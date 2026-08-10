package terminal

import (
	"bytes"
	"errors"
	"io"
	"reflect"
	"sync"
	"testing"
	"time"
)

func TestSessionStateValues(t *testing.T) {
	states := map[SessionState]string{
		SessionStarting: "starting",
		SessionRunning:  "running",
		SessionExited:   "exited",
		SessionClosing:  "closing",
		SessionClosed:   "closed",
		SessionFailed:   "failed",
	}
	for state, want := range states {
		if string(state) != want {
			t.Errorf("state %q = %q, want %q", want, state, want)
		}
	}
}

func TestSessionTransitionsStartingRunningExitedClosed(t *testing.T) {
	process := newControlledPTYProcess()
	process.setExit(23, nil)
	factory := &sessionTestFactory{process: process}
	session := newSession(StartRequest{
		Executable: "/test/shell",
		CWD:        "/test/project",
		Rows:       24,
		Columns:    80,
	}, testSessionDependencies(factory))

	if got := session.State(); got != SessionStarting {
		t.Fatalf("new session state = %q, want %q", got, SessionStarting)
	}
	exitResults := session.ExitResults()
	if err := session.start(); err != nil {
		t.Fatalf("start session: %v", err)
	}
	process.waitUntilWaiting(t)
	if got := session.State(); got != SessionRunning {
		t.Fatalf("started session state = %q, want %q", got, SessionRunning)
	}

	process.releaseWait()
	result := receiveExitResult(t, exitResults)
	if result.ExitCode != 23 || result.State != SessionExited || result.Err != nil {
		t.Fatalf("exit result = %#v, want code 23, exited state, and nil error", result)
	}
	if got := session.State(); got != SessionExited {
		t.Fatalf("exited session state = %q, want %q", got, SessionExited)
	}
	assertExitResultsClosed(t, exitResults)

	if err := session.Close(false); err != nil {
		t.Fatalf("close exited session: %v", err)
	}
	if err := session.Close(false); err != nil {
		t.Fatalf("close exited session again: %v", err)
	}
	if got := session.State(); got != SessionClosed {
		t.Fatalf("closed session state = %q, want %q", got, SessionClosed)
	}
	waitCalls, _, _, closeCalls := process.callCounts()
	if waitCalls != 1 {
		t.Fatalf("PTY Wait calls = %d, want exactly 1", waitCalls)
	}
	if closeCalls != 1 {
		t.Fatalf("PTY Close calls = %d, want exactly 1", closeCalls)
	}
}

func TestSessionWaitsForFinalOutputBeforeClosingPTY(t *testing.T) {
	process := newDelayedDrainPTYProcess()
	session := newSession(StartRequest{
		Executable: "/test/shell",
		CWD:        "/test/project",
		Rows:       24,
		Columns:    80,
	}, testSessionDependencies(&sessionTestFactory{process: process}))
	if err := session.start(); err != nil {
		t.Fatalf("start session: %v", err)
	}
	_, output, err := session.attachOutput()
	if err != nil {
		t.Fatalf("attach output: %v", err)
	}

	<-process.waitReturned
	time.Sleep(25 * time.Millisecond)
	close(process.readRelease)

	var collected []byte
	for chunk := range output {
		collected = append(collected, chunk...)
	}
	if string(collected) != "FINAL" {
		t.Fatalf("terminal output = %q, want final process bytes", collected)
	}
	result := receiveExitResult(t, session.ExitResults())
	if result.Err != nil || result.State != SessionExited {
		t.Fatalf("exit result = %#v", result)
	}
}

func TestSessionBoundsNaturalExitDrainBeforeClosingPTY(t *testing.T) {
	process := newUndrainablePTYProcess()
	dependencies := testSessionDependencies(&sessionTestFactory{process: process})
	dependencies.outputDrainTimeout = 10 * time.Millisecond
	session := newSession(StartRequest{
		Executable: "/test/shell",
		CWD:        "/test/project",
		Rows:       24,
		Columns:    80,
	}, dependencies)
	if err := session.start(); err != nil {
		t.Fatalf("start session: %v", err)
	}
	if _, _, err := session.attachOutput(); err != nil {
		t.Fatalf("attach output: %v", err)
	}

	result := receiveExitResult(t, session.ExitResults())
	if result.Err != nil || result.State != SessionExited {
		t.Fatalf("exit result = %#v", result)
	}
	process.mu.Lock()
	closeCalls := process.closeCalls
	process.mu.Unlock()
	if closeCalls != 1 {
		t.Fatalf("PTY Close calls = %d, want bounded drain fallback to close once", closeCalls)
	}
}

func TestSessionStartFailureTransitionsToFailed(t *testing.T) {
	startErr := errors.New("PTY unavailable")
	factory := &sessionTestFactory{err: startErr}
	session := newSession(StartRequest{
		Executable: "/test/shell",
		CWD:        "/test/project",
		Rows:       24,
		Columns:    80,
	}, testSessionDependencies(factory))

	err := session.start()
	if !errors.Is(err, startErr) {
		t.Fatalf("start error = %v, want %v", err, startErr)
	}
	if got := session.State(); got != SessionFailed {
		t.Fatalf("failed session state = %q, want %q", got, SessionFailed)
	}
}

func TestSessionAttachmentLeaseAttachAndExpiryHaveOneWinner(t *testing.T) {
	t.Run("attachment cancels expiry", func(t *testing.T) {
		session := newSession(StartRequest{}, testSessionDependencies(&sessionTestFactory{}))
		if _, _, err := session.attachOutput(); err != nil {
			t.Fatalf("attach output: %v", err)
		}
		select {
		case <-session.AttachmentSignal():
		default:
			t.Fatal("attachment signal was not closed")
		}
		if session.ExpireUnattached() {
			t.Fatal("expiry won after attachment")
		}
	})

	t.Run("expiry rejects later attachment", func(t *testing.T) {
		session := newSession(StartRequest{}, testSessionDependencies(&sessionTestFactory{}))
		if !session.ExpireUnattached() {
			t.Fatal("first expiry did not claim session")
		}
		if session.ExpireUnattached() {
			t.Fatal("second expiry claimed session again")
		}
		if _, _, err := session.attachOutput(); err == nil {
			t.Fatal("attachment succeeded after lease expiry")
		}
		select {
		case <-session.AttachmentSignal():
			t.Fatal("expiry incorrectly reported a stream attachment")
		default:
		}
	})

	t.Run("concurrent race has exactly one winner", func(t *testing.T) {
		for range 100 {
			session := newSession(StartRequest{}, testSessionDependencies(&sessionTestFactory{}))
			start := make(chan struct{})
			results := make(chan bool, 2)
			go func() {
				<-start
				_, _, err := session.attachOutput()
				results <- err == nil
			}()
			go func() {
				<-start
				results <- session.ExpireUnattached()
			}()
			close(start)
			winners := 0
			if <-results {
				winners++
			}
			if <-results {
				winners++
			}
			if winners != 1 {
				t.Fatalf("attachment/expiry winners = %d, want exactly 1", winners)
			}
		}
	})
}

func TestSessionPassesRequestedStartDataToPTY(t *testing.T) {
	process := newControlledPTYProcess()
	factory := &sessionTestFactory{process: process}
	request := StartRequest{
		Executable: "/test/agent",
		Args:       []string{"--interactive", "--color=true"},
		Env:        []string{"PATH=/test/bin", "TERM=xterm-256color"},
		CWD:        "/test/project",
		Rows:       37,
		Columns:    119,
	}
	session := newSession(request, testSessionDependencies(factory))

	if err := session.start(); err != nil {
		t.Fatalf("start session: %v", err)
	}
	process.waitUntilWaiting(t)
	if got := factory.lastStart(); !reflect.DeepEqual(got, request) {
		t.Fatalf("PTY start request:\ngot:  %#v\nwant: %#v", got, request)
	}

	closeSessionForTest(t, session)
}

func TestSessionRetainsBoundedStartupOutputAndForwardsLiveOutputAfterAttach(t *testing.T) {
	const startupLimit = 8
	process := newControlledPTYProcess()
	factory := &sessionTestFactory{process: process}
	dependencies := testSessionDependencies(factory)
	dependencies.startupBufferBytes = startupLimit
	session := newSession(StartRequest{
		Executable: "/test/shell",
		CWD:        "/test/project",
		Rows:       24,
		Columns:    80,
	}, dependencies)

	if err := session.start(); err != nil {
		t.Fatalf("start session: %v", err)
	}
	process.waitUntilWaiting(t)
	process.sendOutput(t, []byte("0123456789abcdef"))
	process.waitForRead(t)

	startup, live, err := session.attachOutput()
	if err != nil {
		t.Fatalf("attach output: %v", err)
	}
	if !bytes.Equal(startup, []byte("01234567")) {
		t.Fatalf("startup output = %q, want bounded prefix %q", startup, "01234567")
	}
	select {
	case got := <-live:
		if !bytes.Equal(got, []byte("89abcdef")) {
			t.Fatalf("overflow output = %q, want preserved suffix %q", got, "89abcdef")
		}
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for preserved startup overflow")
	}

	process.sendOutput(t, []byte("live-output"))
	process.waitForRead(t)
	select {
	case got := <-live:
		if !bytes.Equal(got, []byte("live-output")) {
			t.Fatalf("live output = %q, want %q", got, "live-output")
		}
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for live output")
	}

	closeSessionForTest(t, session)
}

func TestSessionResizeClampsDimensionsAndSkipsIdenticalRequests(t *testing.T) {
	process := newControlledPTYProcess()
	factory := &sessionTestFactory{process: process}
	session := newSession(StartRequest{
		Executable: "/test/shell",
		CWD:        "/test/project",
		Rows:       24,
		Columns:    80,
	}, testSessionDependencies(factory))
	if err := session.start(); err != nil {
		t.Fatalf("start session: %v", err)
	}
	process.waitUntilWaiting(t)

	if err := session.Resize(24, 80); err != nil {
		t.Fatalf("resize to initial dimensions: %v", err)
	}
	if err := session.Resize(30, 100); err != nil {
		t.Fatalf("resize: %v", err)
	}
	if err := session.Resize(30, 100); err != nil {
		t.Fatalf("repeat resize: %v", err)
	}
	if got := process.resizeCalls(); !reflect.DeepEqual(got, [][2]int{{30, 100}}) {
		t.Fatalf("resize calls = %#v, want one changed resize", got)
	}

	if err := session.Resize(-10, 1_000_000); err != nil {
		t.Fatalf("resize with out-of-range dimensions: %v", err)
	}
	got := process.resizeCalls()
	if len(got) != 2 {
		t.Fatalf("resize call count = %d, want 2: %#v", len(got), got)
	}
	clamped := got[1]
	if clamped[0] <= 0 || clamped[1] <= 0 {
		t.Fatalf("clamped dimensions are not positive: rows=%d columns=%d", clamped[0], clamped[1])
	}
	if clamped[1] >= 1_000_000 {
		t.Fatalf("columns were not clamped to a sensible maximum: %d", clamped[1])
	}

	closeSessionForTest(t, session)
}

func TestSessionClampsInitialDimensionsBeforePTYStart(t *testing.T) {
	process := newControlledPTYProcess()
	factory := &sessionTestFactory{process: process}
	session := newSession(StartRequest{
		Executable: "/test/shell",
		CWD:        "/test/project",
		Rows:       0,
		Columns:    1_000_000,
	}, testSessionDependencies(factory))

	if err := session.start(); err != nil {
		t.Fatalf("start session: %v", err)
	}
	process.waitUntilWaiting(t)
	got := factory.lastStart()
	if got.Rows <= 0 || got.Columns <= 0 {
		t.Fatalf("initial dimensions are not positive: rows=%d columns=%d", got.Rows, got.Columns)
	}
	if got.Columns >= 1_000_000 {
		t.Fatalf("initial columns were not clamped to a sensible maximum: %d", got.Columns)
	}

	closeSessionForTest(t, session)
}

func TestSessionGracefulCloseTerminatesWithoutKillWhenProcessExits(t *testing.T) {
	process := newControlledPTYProcess()
	process.terminateExits = true
	factory := &sessionTestFactory{process: process}
	session := newSession(StartRequest{
		Executable: "/test/shell",
		CWD:        "/test/project",
		Rows:       24,
		Columns:    80,
	}, testSessionDependencies(factory))
	if err := session.start(); err != nil {
		t.Fatalf("start session: %v", err)
	}
	process.waitUntilWaiting(t)

	if err := session.Close(false); err != nil {
		t.Fatalf("graceful close: %v", err)
	}
	if got := session.State(); got != SessionClosed {
		t.Fatalf("state after graceful close = %q, want %q", got, SessionClosed)
	}
	waitCalls, terminateCalls, killCalls, closeCalls := process.callCounts()
	if waitCalls != 1 || terminateCalls != 1 || killCalls != 0 || closeCalls != 1 {
		t.Fatalf(
			"calls after graceful close: Wait=%d Terminate=%d Kill=%d Close=%d; want 1,1,0,1",
			waitCalls,
			terminateCalls,
			killCalls,
			closeCalls,
		)
	}
}

func TestSessionGracefulCloseTransitionsClosingThenKillsAfterInjectedTimeout(t *testing.T) {
	process := newControlledPTYProcess()
	factory := &sessionTestFactory{process: process}
	timeout := make(chan time.Time, 1)
	afterCalled := make(chan time.Duration, 1)
	dependencies := testSessionDependencies(factory)
	dependencies.after = func(duration time.Duration) <-chan time.Time {
		afterCalled <- duration
		return timeout
	}
	session := newSession(StartRequest{
		Executable: "/test/shell",
		CWD:        "/test/project",
		Rows:       24,
		Columns:    80,
	}, dependencies)
	if err := session.start(); err != nil {
		t.Fatalf("start session: %v", err)
	}
	process.waitUntilWaiting(t)

	closeDone := make(chan error, 1)
	go func() {
		closeDone <- session.Close(false)
	}()
	select {
	case duration := <-afterCalled:
		if duration <= 0 {
			t.Errorf("graceful close timeout = %v, want positive bounded duration", duration)
		}
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for graceful close timeout")
	}
	if got := session.State(); got != SessionClosing {
		t.Fatalf("state while waiting for graceful close = %q, want %q", got, SessionClosing)
	}
	_, terminateCalls, killCalls, _ := process.callCounts()
	if terminateCalls != 1 || killCalls != 0 {
		t.Fatalf("calls before timeout: Terminate=%d Kill=%d, want 1 and 0", terminateCalls, killCalls)
	}

	timeout <- time.Now()
	select {
	case err := <-closeDone:
		if err != nil {
			t.Fatalf("close after timeout: %v", err)
		}
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for forced close after timeout")
	}
	if got := session.State(); got != SessionClosed {
		t.Fatalf("state after timeout close = %q, want %q", got, SessionClosed)
	}
	waitCalls, terminateCalls, killCalls, closeCalls := process.callCounts()
	if waitCalls != 1 || terminateCalls != 1 || killCalls != 1 || closeCalls != 1 {
		t.Fatalf(
			"calls after timeout close: Wait=%d Terminate=%d Kill=%d Close=%d; want 1,1,1,1",
			waitCalls,
			terminateCalls,
			killCalls,
			closeCalls,
		)
	}
}

func TestSessionForceCloseKillsImmediatelyWithoutGracefulTimeout(t *testing.T) {
	process := newControlledPTYProcess()
	factory := &sessionTestFactory{process: process}
	afterCalled := make(chan struct{}, 1)
	dependencies := testSessionDependencies(factory)
	dependencies.after = func(time.Duration) <-chan time.Time {
		afterCalled <- struct{}{}
		return make(chan time.Time)
	}
	session := newSession(StartRequest{
		Executable: "/test/shell",
		CWD:        "/test/project",
		Rows:       24,
		Columns:    80,
	}, dependencies)
	if err := session.start(); err != nil {
		t.Fatalf("start session: %v", err)
	}
	process.waitUntilWaiting(t)

	if err := session.Close(true); err != nil {
		t.Fatalf("force close: %v", err)
	}
	if got := session.State(); got != SessionClosed {
		t.Fatalf("state after force close = %q, want %q", got, SessionClosed)
	}
	select {
	case <-afterCalled:
		t.Fatal("force close used graceful timeout")
	default:
	}
	waitCalls, terminateCalls, killCalls, closeCalls := process.callCounts()
	if waitCalls != 1 || terminateCalls != 0 || killCalls != 1 || closeCalls != 1 {
		t.Fatalf(
			"calls after force close: Wait=%d Terminate=%d Kill=%d Close=%d; want 1,0,1,1",
			waitCalls,
			terminateCalls,
			killCalls,
			closeCalls,
		)
	}
}

type sessionTestFactory struct {
	mu      sync.Mutex
	starts  []StartRequest
	process PTYProcess
	err     error
}

func (f *sessionTestFactory) Start(request StartRequest) (PTYProcess, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.starts = append(f.starts, cloneStartRequest(request))
	if f.err != nil {
		return nil, f.err
	}
	return f.process, nil
}

func (f *sessionTestFactory) lastStart() StartRequest {
	f.mu.Lock()
	defer f.mu.Unlock()
	if len(f.starts) == 0 {
		return StartRequest{}
	}
	return cloneStartRequest(f.starts[len(f.starts)-1])
}

type delayedDrainPTYProcess struct {
	*fakePTYProcess
	waitReturned chan struct{}
	readRelease  chan struct{}
	readOnce     sync.Once
}

func newDelayedDrainPTYProcess() *delayedDrainPTYProcess {
	return &delayedDrainPTYProcess{
		fakePTYProcess: newFakePTYProcess(),
		waitReturned:   make(chan struct{}),
		readRelease:    make(chan struct{}),
	}
}

func (p *delayedDrainPTYProcess) Read(buffer []byte) (int, error) {
	<-p.readRelease
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.closed {
		return 0, io.EOF
	}
	read := false
	p.readOnce.Do(func() {
		read = true
	})
	if !read {
		return 0, io.EOF
	}
	return copy(buffer, []byte("FINAL")), nil
}

func (p *delayedDrainPTYProcess) Wait() (int, error) {
	close(p.waitReturned)
	return 0, nil
}

type undrainablePTYProcess struct {
	*fakePTYProcess
	closedRead chan struct{}
	closeOnce  sync.Once
}

func newUndrainablePTYProcess() *undrainablePTYProcess {
	return &undrainablePTYProcess{
		fakePTYProcess: newFakePTYProcess(),
		closedRead:     make(chan struct{}),
	}
}

func (p *undrainablePTYProcess) Read([]byte) (int, error) {
	<-p.closedRead
	return 0, io.EOF
}

func (p *undrainablePTYProcess) Wait() (int, error) {
	return 0, nil
}

func (p *undrainablePTYProcess) Close() error {
	err := p.fakePTYProcess.Close()
	p.closeOnce.Do(func() {
		close(p.closedRead)
	})
	return err
}

type controlledPTYProcess struct {
	*fakePTYProcess

	output          chan []byte
	readObserved    chan []byte
	pendingOutput   []byte
	waitStarted     chan struct{}
	waitRelease     chan struct{}
	waitStartedOnce sync.Once
	waitReleaseOnce sync.Once
	outputCloseOnce sync.Once
	terminateExits  bool
}

func newControlledPTYProcess() *controlledPTYProcess {
	return &controlledPTYProcess{
		fakePTYProcess: newFakePTYProcess(),
		output:         make(chan []byte, 8),
		readObserved:   make(chan []byte, 8),
		waitStarted:    make(chan struct{}),
		waitRelease:    make(chan struct{}),
	}
}

func (p *controlledPTYProcess) Read(buffer []byte) (int, error) {
	if len(p.pendingOutput) == 0 {
		chunk, ok := <-p.output
		if !ok {
			return 0, io.EOF
		}
		p.pendingOutput = chunk
	}
	n := copy(buffer, p.pendingOutput)
	p.pendingOutput = p.pendingOutput[n:]
	p.readObserved <- append([]byte(nil), buffer[:n]...)
	return n, nil
}

func (p *controlledPTYProcess) Wait() (int, error) {
	p.waitStartedOnce.Do(func() {
		close(p.waitStarted)
	})
	<-p.waitRelease
	return p.fakePTYProcess.Wait()
}

func (p *controlledPTYProcess) Terminate() error {
	err := p.fakePTYProcess.Terminate()
	if p.terminateExits {
		p.releaseWait()
	}
	return err
}

func (p *controlledPTYProcess) Kill() error {
	err := p.fakePTYProcess.Kill()
	p.releaseWait()
	return err
}

func (p *controlledPTYProcess) Close() error {
	err := p.fakePTYProcess.Close()
	p.outputCloseOnce.Do(func() {
		close(p.output)
	})
	return err
}

func (p *controlledPTYProcess) setExit(code int, err error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.waitCode = code
	p.waitErr = err
}

func (p *controlledPTYProcess) releaseWait() {
	p.waitReleaseOnce.Do(func() {
		close(p.waitRelease)
	})
	p.outputCloseOnce.Do(func() {
		close(p.output)
	})
}

func (p *controlledPTYProcess) waitUntilWaiting(t *testing.T) {
	t.Helper()
	select {
	case <-p.waitStarted:
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for PTY Wait")
	}
}

func (p *controlledPTYProcess) sendOutput(t *testing.T, output []byte) {
	t.Helper()
	select {
	case p.output <- append([]byte(nil), output...):
	case <-time.After(time.Second):
		t.Fatal("timed out sending controlled PTY output")
	}
}

func (p *controlledPTYProcess) waitForRead(t *testing.T) []byte {
	t.Helper()
	select {
	case output := <-p.readObserved:
		return output
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for controlled PTY read")
		return nil
	}
}

func (p *controlledPTYProcess) callCounts() (wait, terminate, kill, close int) {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.waitCalls, p.terminateCalls, p.killCalls, p.closeCalls
}

func (p *controlledPTYProcess) resizeCalls() [][2]int {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([][2]int(nil), p.resizes...)
}

func testSessionDependencies(factory PTYFactory) sessionDependencies {
	return sessionDependencies{
		factory:            factory,
		startupBufferBytes: 64 * 1024,
		gracefulTimeout:    250 * time.Millisecond,
		after:              time.After,
	}
}

func closeSessionForTest(t *testing.T, session *Session) {
	t.Helper()
	if err := session.Close(true); err != nil {
		t.Fatalf("close session: %v", err)
	}
}

func receiveExitResult(t *testing.T, results <-chan ExitResult) ExitResult {
	t.Helper()
	select {
	case result, ok := <-results:
		if !ok {
			t.Fatal("exit results closed without a result")
		}
		return result
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for exit result")
		return ExitResult{}
	}
}

func assertExitResultsClosed(t *testing.T, results <-chan ExitResult) {
	t.Helper()
	select {
	case result, ok := <-results:
		if ok {
			t.Fatalf("received duplicate exit result %#v", result)
		}
	case <-time.After(time.Second):
		t.Fatal("exit results did not close after one result")
	}
}
