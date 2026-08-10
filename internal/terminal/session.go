package terminal

import (
	"errors"
	"fmt"
	"io"
	"os"
	"sync"
	"time"
)

type SessionState string

const (
	SessionStarting SessionState = "starting"
	SessionRunning  SessionState = "running"
	SessionExited   SessionState = "exited"
	SessionClosing  SessionState = "closing"
	SessionClosed   SessionState = "closed"
	SessionFailed   SessionState = "failed"
)

const (
	defaultStartupBufferBytes = 64 * 1024
	defaultGracefulTimeout    = 750 * time.Millisecond
	defaultOutputDrainTimeout = 250 * time.Millisecond
	maxTerminalRows           = 1_000
	maxTerminalColumns        = 1_000
)

type ExitResult struct {
	ExitCode int
	State    SessionState
	Err      error
}

type sessionDependencies struct {
	factory            PTYFactory
	startupBufferBytes int
	gracefulTimeout    time.Duration
	outputDrainTimeout time.Duration
	after              func(time.Duration) <-chan time.Time
	drainAfter         func(time.Duration) <-chan time.Time
}

type Session struct {
	request      StartRequest
	dependencies sessionDependencies

	mu             sync.Mutex
	id             string
	token          string
	profile        string
	profileKind    ProfileKind
	provider       string
	cwd            string
	state          SessionState
	streamErr      error
	process        PTYProcess
	pid            int
	startedAt      time.Time
	lastActivityAt time.Time
	rows           int
	columns        int

	startupOutput []byte
	attached      bool
	attachExpired bool
	attachSignal  chan struct{}
	liveOutput    chan []byte
	closingSignal chan struct{}
	closingOnce   sync.Once

	exitResults chan ExitResult
	exitDone    chan struct{}
	outputDone  chan struct{}

	workerMu        sync.Mutex
	workers         int
	workersDone     chan struct{}
	workersDoneOnce sync.Once

	processCloseOnce sync.Once
	processCloseErr  error

	closeOnce sync.Once
	closeDone chan struct{}
	closeErr  error
}

func newSession(request StartRequest, dependencies sessionDependencies) *Session {
	if dependencies.startupBufferBytes <= 0 {
		dependencies.startupBufferBytes = defaultStartupBufferBytes
	}
	if dependencies.gracefulTimeout <= 0 {
		dependencies.gracefulTimeout = defaultGracefulTimeout
	}
	if dependencies.outputDrainTimeout <= 0 {
		dependencies.outputDrainTimeout = defaultOutputDrainTimeout
	}
	if dependencies.after == nil {
		dependencies.after = time.After
	}
	if dependencies.drainAfter == nil {
		dependencies.drainAfter = time.After
	}
	request.Rows, request.Columns = clampDimensions(request.Rows, request.Columns)
	return &Session{
		request:       cloneStartRequest(request),
		dependencies:  dependencies,
		state:         SessionStarting,
		rows:          request.Rows,
		columns:       request.Columns,
		attachSignal:  make(chan struct{}),
		liveOutput:    make(chan []byte),
		closingSignal: make(chan struct{}),
		exitResults:   make(chan ExitResult, 1),
		exitDone:      make(chan struct{}),
		outputDone:    make(chan struct{}),
		workersDone:   make(chan struct{}),
		closeDone:     make(chan struct{}),
	}
}

func (s *Session) start() error {
	s.mu.Lock()
	if s.state != SessionStarting {
		s.mu.Unlock()
		return fmt.Errorf("start terminal session in state %q", s.state)
	}
	s.mu.Unlock()

	process, err := s.dependencies.factory.Start(cloneStartRequest(s.request))
	if err != nil {
		if process != nil {
			err = errors.Join(err, process.Close())
		}
		s.mu.Lock()
		s.state = SessionFailed
		s.mu.Unlock()
		s.finishWithoutWorkers()
		return err
	}

	s.mu.Lock()
	s.process = process
	if identified, ok := process.(interface{ PID() int }); ok {
		s.pid = identified.PID()
	}
	s.startedAt = time.Now()
	s.lastActivityAt = s.startedAt
	s.state = SessionRunning
	s.mu.Unlock()

	s.workerMu.Lock()
	s.workers = 2
	s.workerMu.Unlock()
	go func() {
		defer s.workerDone()
		defer close(s.outputDone)
		s.readOutput(process)
	}()
	go func() {
		defer s.workerDone()
		s.waitForExit(process)
	}()
	return nil
}

func (s *Session) ID() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.id
}

func (s *Session) StreamToken() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.token
}

func (s *Session) ProfileID() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.profile
}

func (s *Session) CWD() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.cwd
}

func (s *Session) setMetadata(
	id, token, profileID string,
	profileKind ProfileKind,
	provider, cwd string,
) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.id = id
	s.token = token
	s.profile = profileID
	s.profileKind = profileKind
	s.provider = provider
	s.cwd = cwd
}

func (s *Session) PID() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.pid
}

func (s *Session) State() SessionState {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.state
}

func (s *Session) ExitResults() <-chan ExitResult {
	return s.exitResults
}

// AttachmentSignal is closed when the terminal stream claims this session.
// It is intentionally read-only so lifecycle owners can bound how long an
// unattached process is allowed to retain resources.
func (s *Session) AttachmentSignal() <-chan struct{} {
	return s.attachSignal
}

// ExpireUnattached atomically competes with the first stream attachment. The
// caller that receives true owns cleanup of the unclaimed session.
func (s *Session) ExpireUnattached() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.attached || s.attachExpired || s.state == SessionClosing || s.state == SessionClosed {
		return false
	}
	s.attachExpired = true
	return true
}

func (s *Session) attachOutput() ([]byte, <-chan []byte, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.attached {
		return nil, nil, errors.New("terminal output is already attached")
	}
	if s.attachExpired {
		return nil, nil, errors.New("terminal output attachment lease expired")
	}
	if s.state == SessionFailed || s.state == SessionClosed {
		return nil, nil, fmt.Errorf("attach terminal output in state %q", s.state)
	}
	s.attached = true
	startup := append([]byte(nil), s.startupOutput...)
	s.startupOutput = nil
	close(s.attachSignal)
	return startup, s.liveOutput, nil
}

func (s *Session) Resize(rows, columns int) error {
	rows, columns = clampDimensions(rows, columns)

	s.mu.Lock()
	defer s.mu.Unlock()
	if s.state != SessionRunning {
		return fmt.Errorf("resize terminal session in state %q", s.state)
	}
	if rows == s.rows && columns == s.columns {
		return nil
	}
	if err := s.process.Resize(rows, columns); err != nil {
		return err
	}
	s.rows = rows
	s.columns = columns
	return nil
}

func (s *Session) WriteInput(input []byte) error {
	s.mu.Lock()
	if s.state != SessionRunning {
		state := s.state
		s.mu.Unlock()
		return fmt.Errorf("write terminal input in state %q", state)
	}
	process := s.process
	s.lastActivityAt = time.Now()
	s.mu.Unlock()
	for len(input) > 0 {
		written, err := process.Write(input)
		if err != nil {
			return err
		}
		if written <= 0 {
			return io.ErrShortWrite
		}
		input = input[written:]
	}
	return nil
}

func (s *Session) Close(force bool) error {
	s.closeOnce.Do(func() {
		s.closeErr = s.close(force)
		close(s.closeDone)
	})
	<-s.closeDone
	return s.closeErr
}

func (s *Session) close(force bool) error {
	s.mu.Lock()
	state := s.state
	process := s.process
	if state == SessionClosed {
		s.mu.Unlock()
		return nil
	}
	if state == SessionStarting || (state == SessionFailed && process == nil) {
		s.state = SessionClosed
		s.mu.Unlock()
		s.beginClosingOutput()
		s.finishWithoutWorkers()
		return nil
	}
	if state == SessionExited {
		s.state = SessionClosed
		s.mu.Unlock()
		s.beginClosingOutput()
		closeErr := s.closeProcess(process)
		<-s.workersDone
		return closeErr
	}
	s.state = SessionClosing
	s.mu.Unlock()
	s.beginClosingOutput()

	var closeErrors []error
	if force {
		if err := ignoreProcessDone(process.Kill()); err != nil {
			closeErrors = append(closeErrors, fmt.Errorf("kill terminal process: %w", err))
		}
	} else {
		if err := ignoreProcessDone(process.Terminate()); err != nil {
			closeErrors = append(closeErrors, fmt.Errorf("terminate terminal process: %w", err))
		}
		select {
		case <-s.exitDone:
		case <-s.dependencies.after(s.dependencies.gracefulTimeout):
			if err := ignoreProcessDone(process.Kill()); err != nil {
				closeErrors = append(closeErrors, fmt.Errorf("kill terminal process after timeout: %w", err))
			}
		}
	}

	<-s.exitDone
	if err := s.closeProcess(process); err != nil {
		closeErrors = append(closeErrors, err)
	}
	<-s.workersDone
	s.mu.Lock()
	s.state = SessionClosed
	s.mu.Unlock()
	return errors.Join(closeErrors...)
}

func (s *Session) waitForExit(process PTYProcess) {
	exitCode, waitErr := process.Wait()

	select {
	case <-s.outputDone:
	case <-s.closingSignal:
		_ = s.closeProcess(process)
		<-s.outputDone
	case <-s.dependencies.drainAfter(s.dependencies.outputDrainTimeout):
		s.beginClosingOutput()
		_ = s.closeProcess(process)
		<-s.outputDone
	}

	s.mu.Lock()
	resultState := SessionExited
	if s.streamErr != nil {
		waitErr = errors.Join(waitErr, s.streamErr)
	}
	if waitErr != nil {
		resultState = SessionFailed
	}
	if s.state == SessionRunning {
		s.state = resultState
	}
	s.mu.Unlock()

	closeErr := s.closeProcess(process)
	if waitErr == nil && closeErr != nil {
		waitErr = closeErr
		resultState = SessionFailed
		s.mu.Lock()
		if s.state == SessionExited {
			s.state = SessionFailed
		}
		s.mu.Unlock()
	}
	s.exitResults <- ExitResult{ExitCode: exitCode, State: resultState, Err: waitErr}
	close(s.exitResults)
	close(s.exitDone)
}

func (s *Session) readOutput(process PTYProcess) {
	defer close(s.liveOutput)
	buffer := make([]byte, outputChunkBytes)
	for {
		readSize := s.nextOutputReadSize()
		n, err := process.Read(buffer[:readSize])
		if n > 0 {
			s.deliverOutput(buffer[:n])
		}
		if err != nil {
			if !errors.Is(err, io.EOF) && !errors.Is(err, os.ErrClosed) {
				s.failOutput(process, err)
			}
			return
		}
	}
}

func (s *Session) deliverOutput(output []byte) {
	chunk := append([]byte(nil), output...)

	s.mu.Lock()
	s.lastActivityAt = time.Now()
	if !s.attached {
		remaining := s.dependencies.startupBufferBytes - len(s.startupOutput)
		if remaining > 0 {
			s.startupOutput = append(s.startupOutput, chunk...)
		}
		full := len(s.startupOutput) >= s.dependencies.startupBufferBytes
		s.mu.Unlock()
		if full {
			select {
			case <-s.attachSignal:
			case <-s.closingSignal:
			}
		}
		return
	}
	s.mu.Unlock()

	select {
	case s.liveOutput <- chunk:
	case <-s.closingSignal:
	}
}

func (s *Session) nextOutputReadSize() int {
	for {
		s.mu.Lock()
		if s.attached || s.state == SessionClosing || s.state == SessionClosed {
			s.mu.Unlock()
			return outputChunkBytes
		}
		remaining := s.dependencies.startupBufferBytes - len(s.startupOutput)
		s.mu.Unlock()
		if remaining > 0 {
			return min(remaining, outputChunkBytes)
		}
		select {
		case <-s.attachSignal:
		case <-s.closingSignal:
		}
	}
}

func (s *Session) failOutput(process PTYProcess, readErr error) {
	s.mu.Lock()
	s.streamErr = fmt.Errorf("read terminal output: %w", readErr)
	if s.state == SessionRunning {
		s.state = SessionFailed
	}
	s.mu.Unlock()
	if err := ignoreProcessDone(process.Kill()); err != nil {
		s.mu.Lock()
		s.streamErr = errors.Join(s.streamErr, fmt.Errorf("kill after terminal read failure: %w", err))
		s.mu.Unlock()
	}
}

func (s *Session) beginClosingOutput() {
	s.closingOnce.Do(func() {
		close(s.closingSignal)
	})
}

func (s *Session) closeProcess(process PTYProcess) error {
	if process == nil {
		return nil
	}
	s.processCloseOnce.Do(func() {
		s.processCloseErr = process.Close()
	})
	return s.processCloseErr
}

func (s *Session) workerDone() {
	s.workerMu.Lock()
	s.workers--
	done := s.workers == 0
	s.workerMu.Unlock()
	if done {
		s.workersDoneOnce.Do(func() {
			close(s.workersDone)
		})
	}
}

func (s *Session) finishWithoutWorkers() {
	s.workersDoneOnce.Do(func() {
		close(s.workersDone)
	})
}

func clampDimensions(rows, columns int) (int, int) {
	rows = max(1, min(rows, maxTerminalRows))
	columns = max(1, min(columns, maxTerminalColumns))
	return rows, columns
}

func ignoreProcessDone(err error) error {
	if errors.Is(err, os.ErrProcessDone) {
		return nil
	}
	return err
}
