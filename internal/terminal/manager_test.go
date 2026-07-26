package terminal

import (
	"context"
	"errors"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"sync"
	"testing"
	"time"
)

func TestManagerOwnsSessionsAndStartsWithCopiedProfileParameters(t *testing.T) {
	projectRoot := t.TempDir()
	requestedCWD := t.TempDir()
	executable := filepath.Join(projectRoot, "agent")
	arguments := []string{"--mode", "interactive"}
	overrides := map[string]string{"MANAGER_TEST": "original"}
	profiles := []Profile{{
		ID:         "agent",
		Name:       "Agent",
		Kind:       ProfileAgent,
		Executable: executable,
		Args:       arguments,
		Env:        overrides,
	}}
	firstProcess := newManagerFakeProcess()
	secondProcess := newManagerFakeProcess()
	factory := newManagerFakeFactory(
		managerStartOutcome{process: firstProcess},
		managerStartOutcome{process: secondProcess},
	)

	manager, err := NewManager(projectRoot, profiles, factory)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	cleanupManager(t, manager, firstProcess, secondProcess)

	profiles[0].Executable = filepath.Join(projectRoot, "mutated-agent")
	arguments[0] = "--mutated"
	overrides["MANAGER_TEST"] = "mutated"

	wantEnvironment, err := buildEnvironment(os.Environ(), map[string]string{
		"MANAGER_TEST": "original",
	})
	if err != nil {
		t.Fatalf("build expected environment: %v", err)
	}

	first, err := manager.Create("agent", requestedCWD, 31, 107)
	if err != nil {
		t.Fatalf("Create first session: %v", err)
	}
	second, err := manager.Create("agent", "", 24, 80)
	if err != nil {
		t.Fatalf("Create second session: %v", err)
	}

	wantFirstStart := StartRequest{
		Executable: executable,
		Args:       []string{"--mode", "interactive"},
		Env:        wantEnvironment,
		CWD:        requestedCWD,
		Rows:       31,
		Columns:    107,
	}
	starts := factory.recordedStarts()
	if len(starts) != 2 {
		t.Fatalf("factory starts = %d, want 2", len(starts))
	}
	if !reflect.DeepEqual(starts[0], wantFirstStart) {
		t.Fatalf("first start request:\ngot:  %#v\nwant: %#v", starts[0], wantFirstStart)
	}
	if starts[1].CWD != projectRoot || starts[1].Rows != 24 || starts[1].Columns != 80 {
		t.Fatalf("default-root start request = %#v", starts[1])
	}

	identities := []string{
		first.ID(),
		first.StreamToken(),
		second.ID(),
		second.StreamToken(),
	}
	for index, identity := range identities {
		if identity == "" {
			t.Fatalf("opaque identity %d is empty", index)
		}
		if identity == "agent" {
			t.Fatalf("opaque identity exposes profile ID: %q", identity)
		}
		for previous := 0; previous < index; previous++ {
			if identities[previous] == identity {
				t.Fatalf("session IDs and stream tokens are not separate and unique: %#v", identities)
			}
		}
	}

	gotFirst, err := manager.Get(first.ID())
	if err != nil {
		t.Fatalf("Get first session: %v", err)
	}
	if gotFirst != first {
		t.Fatal("Get returned a different first session")
	}
	gotSecond, err := manager.Get(second.ID())
	if err != nil {
		t.Fatalf("Get second session: %v", err)
	}
	if gotSecond != second {
		t.Fatal("Get returned a different second session")
	}
}

func TestManagerSessionSnapshotIncludesOwnedProcessMetadata(t *testing.T) {
	root := t.TempDir()
	process := newManagerFakeProcess()
	manager, err := NewManager(root, []Profile{{
		ID:         "agent-codex",
		Name:       "Codex",
		Kind:       ProfileAgent,
		Provider:   "codex",
		Executable: filepath.Join(root, "codex"),
	}}, newManagerFakeFactory(managerStartOutcome{process: process}))
	if err != nil {
		t.Fatal(err)
	}
	cleanupManager(t, manager, process)
	session, err := manager.Create("agent-codex", "", 24, 80)
	if err != nil {
		t.Fatal(err)
	}
	if session.PID() != 31337 {
		t.Fatalf("session PID = %d want 31337", session.PID())
	}
	snapshot := manager.SessionSnapshot(64)
	if len(snapshot) != 1 || snapshot[0].ID != session.ID() ||
		snapshot[0].ProfileID != "agent-codex" ||
		snapshot[0].ProfileKind != ProfileAgent ||
		snapshot[0].Provider != "codex" || snapshot[0].PID != 31337 ||
		snapshot[0].State != SessionRunning || snapshot[0].CWD != root {
		t.Fatalf("session snapshot = %#v", snapshot)
	}
}

func TestManagerCreateFailureIsCleanedUpAndNotRetained(t *testing.T) {
	projectRoot := t.TempDir()
	startError := errors.New("start failed")
	partialProcess := newManagerFakeProcess()
	runningProcess := newManagerFakeProcess()
	factory := newManagerFakeFactory(
		managerStartOutcome{process: partialProcess, err: startError},
		managerStartOutcome{process: runningProcess},
	)
	manager := newManagerForTest(t, projectRoot, factory)
	cleanupManager(t, manager, partialProcess, runningProcess)

	if _, err := manager.Create("agent", "", 24, 80); !errors.Is(err, startError) {
		t.Fatalf("Create error = %v, want %v", err, startError)
	}
	partialSnapshot := partialProcess.snapshot()
	if partialSnapshot.closeCalls != 1 {
		t.Fatalf("partial process close calls = %d, want 1", partialSnapshot.closeCalls)
	}
	if partialSnapshot.terminateCalls != 0 || partialSnapshot.killCalls != 0 {
		t.Fatalf("failed process retained for lifecycle calls: %#v", partialSnapshot)
	}

	running, err := manager.Create("agent", "", 24, 80)
	if err != nil {
		t.Fatalf("Create after failed create: %v", err)
	}
	if _, err := manager.Get(running.ID()); err != nil {
		t.Fatalf("Get successful session after failed create: %v", err)
	}

	if err := shutdownForTest(manager); err != nil {
		t.Fatalf("Shutdown: %v", err)
	}
	partialSnapshot = partialProcess.snapshot()
	if partialSnapshot.closeCalls != 1 ||
		partialSnapshot.terminateCalls != 0 ||
		partialSnapshot.killCalls != 0 {
		t.Fatalf("failed process was retained by manager: %#v", partialSnapshot)
	}
}

func TestManagerUnknownSessionOperationsReturnErrors(t *testing.T) {
	manager := newManagerForTest(t, t.TempDir(), newManagerFakeFactory())
	cleanupManager(t, manager)

	if _, err := manager.Get("missing"); err == nil {
		t.Fatal("Get unknown session succeeded")
	}
	if err := manager.Resize("missing", 24, 80); err == nil {
		t.Fatal("Resize unknown session succeeded")
	}
	if err := manager.CloseSession("missing", false); err == nil {
		t.Fatal("CloseSession unknown session succeeded")
	}
}

func TestManagerDelegatesResizeAndGracefulOrForcedClose(t *testing.T) {
	gracefulProcess := newManagerFakeProcess()
	forcedProcess := newManagerFakeProcess()
	factory := newManagerFakeFactory(
		managerStartOutcome{process: gracefulProcess},
		managerStartOutcome{process: forcedProcess},
	)
	manager := newManagerForTest(t, t.TempDir(), factory)
	cleanupManager(t, manager, gracefulProcess, forcedProcess)

	graceful, err := manager.Create("agent", "", 24, 80)
	if err != nil {
		t.Fatalf("Create graceful session: %v", err)
	}
	forced, err := manager.Create("agent", "", 24, 80)
	if err != nil {
		t.Fatalf("Create forced session: %v", err)
	}

	if err := manager.Resize(graceful.ID(), 42, 132); err != nil {
		t.Fatalf("Resize: %v", err)
	}
	if err := manager.CloseSession(graceful.ID(), false); err != nil {
		t.Fatalf("graceful CloseSession: %v", err)
	}
	if err := manager.CloseSession(forced.ID(), true); err != nil {
		t.Fatalf("forced CloseSession: %v", err)
	}

	gracefulSnapshot := gracefulProcess.snapshot()
	if !reflect.DeepEqual(gracefulSnapshot.resizes, [][2]int{{42, 132}}) {
		t.Fatalf("resize delegation = %#v, want rows 42 columns 132", gracefulSnapshot.resizes)
	}
	if gracefulSnapshot.terminateCalls != 1 ||
		gracefulSnapshot.killCalls != 0 ||
		gracefulSnapshot.closeCalls != 1 {
		t.Fatalf("graceful close delegation = %#v", gracefulSnapshot)
	}
	forcedSnapshot := forcedProcess.snapshot()
	if forcedSnapshot.terminateCalls != 0 ||
		forcedSnapshot.killCalls != 1 ||
		forcedSnapshot.closeCalls != 1 {
		t.Fatalf("forced close delegation = %#v", forcedSnapshot)
	}

	if _, err := manager.Get(graceful.ID()); err == nil {
		t.Fatal("gracefully closed session remains registered")
	}
	if _, err := manager.Get(forced.ID()); err == nil {
		t.Fatal("force-closed session remains registered")
	}
}

func TestManagerShutdownIsParallelIdempotentAndWaitsForOwnedGoroutines(t *testing.T) {
	processes := []*managerFakeProcess{
		newManagerFakeProcess(),
		newManagerFakeProcess(),
		newManagerFakeProcess(),
	}
	outcomes := make([]managerStartOutcome, 0, len(processes))
	for _, process := range processes {
		process.releaseOnTerminate = false
		process.releaseOnKill = false
		outcomes = append(outcomes, managerStartOutcome{process: process})
	}
	factory := newManagerFakeFactory(outcomes...)
	manager := newManagerForTest(t, t.TempDir(), factory)
	cleanupManager(t, manager, processes...)

	for index := range processes {
		if _, err := manager.Create("agent", "", 24+index, 80+index); err != nil {
			t.Fatalf("Create session %d: %v", index, err)
		}
		awaitSignal(t, processes[index].waitStarted, "process Wait start")
		awaitSignal(t, processes[index].readStarted, "process Read start")
	}

	shutdownContext, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	shutdownDone := make(chan error, 1)
	go func() {
		shutdownDone <- manager.Shutdown(shutdownContext)
	}()

	for _, process := range processes {
		awaitSignal(t, process.terminateStarted, "parallel terminate")
	}
	select {
	case err := <-shutdownDone:
		t.Fatalf("Shutdown returned before owned Wait/Read goroutines completed: %v", err)
	default:
	}

	for _, process := range processes {
		process.exit(0, nil)
	}
	if err := awaitError(t, shutdownDone, "Shutdown completion"); err != nil {
		t.Fatalf("Shutdown: %v", err)
	}

	beforeSecondShutdown := make([]managerProcessSnapshot, len(processes))
	for index, process := range processes {
		beforeSecondShutdown[index] = process.snapshot()
		if beforeSecondShutdown[index].terminateCalls != 1 ||
			beforeSecondShutdown[index].killCalls != 0 ||
			beforeSecondShutdown[index].closeCalls != 1 ||
			beforeSecondShutdown[index].waitCalls != 1 {
			t.Errorf("process %d shutdown calls = %#v", index, beforeSecondShutdown[index])
		}
	}
	if err := manager.Shutdown(context.Background()); err != nil {
		t.Fatalf("second Shutdown: %v", err)
	}
	for index, process := range processes {
		if got := process.snapshot(); !reflect.DeepEqual(got, beforeSecondShutdown[index]) {
			t.Errorf("process %d changed after idempotent Shutdown:\ngot:  %#v\nwant: %#v",
				index, got, beforeSecondShutdown[index])
		}
	}

	startsBeforeRejectedCreate := len(factory.recordedStarts())
	if _, err := manager.Create("agent", "", 24, 80); err == nil {
		t.Fatal("Create succeeded after Shutdown")
	}
	if startsAfter := len(factory.recordedStarts()); startsAfter != startsBeforeRejectedCreate {
		t.Fatalf("factory called after Shutdown: starts %d -> %d",
			startsBeforeRejectedCreate, startsAfter)
	}
}

func TestManagerShutdownAggregatesErrorsAcrossSessions(t *testing.T) {
	firstError := errors.New("first close failed")
	secondError := errors.New("second close failed")
	firstProcess := newManagerFakeProcess()
	firstProcess.closeErr = firstError
	secondProcess := newManagerFakeProcess()
	secondProcess.closeErr = secondError
	factory := newManagerFakeFactory(
		managerStartOutcome{process: firstProcess},
		managerStartOutcome{process: secondProcess},
	)
	manager := newManagerForTest(t, t.TempDir(), factory)
	cleanupManager(t, manager, firstProcess, secondProcess)

	if _, err := manager.Create("agent", "", 24, 80); err != nil {
		t.Fatalf("Create first session: %v", err)
	}
	if _, err := manager.Create("agent", "", 24, 80); err != nil {
		t.Fatalf("Create second session: %v", err)
	}

	shutdownError := shutdownForTest(manager)
	if !errors.Is(shutdownError, firstError) || !errors.Is(shutdownError, secondError) {
		t.Fatalf("Shutdown error = %v, want both %v and %v",
			shutdownError, firstError, secondError)
	}
	repeatedError := manager.Shutdown(context.Background())
	if !errors.Is(repeatedError, firstError) || !errors.Is(repeatedError, secondError) {
		t.Fatalf("repeated Shutdown error = %v, want cached aggregate", repeatedError)
	}
}

func newManagerForTest(
	t *testing.T,
	projectRoot string,
	factory PTYFactory,
) *Manager {
	t.Helper()
	manager, err := NewManager(projectRoot, []Profile{{
		ID:         "agent",
		Name:       "Agent",
		Kind:       ProfileAgent,
		Executable: filepath.Join(projectRoot, "agent"),
		Args:       []string{"--interactive"},
		Env:        map[string]string{"MANAGER_TEST": "value"},
	}}, factory)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	return manager
}

func cleanupManager(t *testing.T, manager *Manager, processes ...*managerFakeProcess) {
	t.Helper()
	t.Cleanup(func() {
		for _, process := range processes {
			process.exit(0, nil)
		}
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		_ = manager.Shutdown(ctx)
	})
}

func shutdownForTest(manager *Manager) error {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	return manager.Shutdown(ctx)
}

func awaitSignal(t *testing.T, signal <-chan struct{}, description string) {
	t.Helper()
	select {
	case <-signal:
	case <-time.After(2 * time.Second):
		t.Fatalf("timed out waiting for %s", description)
	}
}

func awaitError(t *testing.T, result <-chan error, description string) error {
	t.Helper()
	select {
	case err := <-result:
		return err
	case <-time.After(2 * time.Second):
		t.Fatalf("timed out waiting for %s", description)
		return nil
	}
}

type managerStartOutcome struct {
	process PTYProcess
	err     error
}

type managerFakeFactory struct {
	mu       sync.Mutex
	outcomes []managerStartOutcome
	starts   []StartRequest
}

func newManagerFakeFactory(outcomes ...managerStartOutcome) *managerFakeFactory {
	return &managerFakeFactory{
		outcomes: append([]managerStartOutcome(nil), outcomes...),
	}
}

func (f *managerFakeFactory) Start(request StartRequest) (PTYProcess, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.starts = append(f.starts, cloneStartRequest(request))
	if len(f.outcomes) == 0 {
		return nil, errors.New("manager fake factory exhausted")
	}
	outcome := f.outcomes[0]
	f.outcomes = f.outcomes[1:]
	return outcome.process, outcome.err
}

func (f *managerFakeFactory) recordedStarts() []StartRequest {
	f.mu.Lock()
	defer f.mu.Unlock()
	starts := make([]StartRequest, len(f.starts))
	for index, start := range f.starts {
		starts[index] = cloneStartRequest(start)
	}
	return starts
}

type managerWaitResult struct {
	code int
	err  error
}

type managerFakeProcess struct {
	*fakePTYProcess

	waitStarted      chan struct{}
	readStarted      chan struct{}
	terminateStarted chan struct{}
	killStarted      chan struct{}
	exitResult       chan managerWaitResult
	readClosed       chan struct{}

	waitStartOnce      sync.Once
	readStartOnce      sync.Once
	terminateStartOnce sync.Once
	killStartOnce      sync.Once
	exitOnce           sync.Once
	readCloseOnce      sync.Once

	releaseOnTerminate bool
	releaseOnKill      bool
	resizeErr          error
	terminateErr       error
	killErr            error
	closeErr           error
}

func newManagerFakeProcess() *managerFakeProcess {
	return &managerFakeProcess{
		fakePTYProcess:     newFakePTYProcess(),
		waitStarted:        make(chan struct{}),
		readStarted:        make(chan struct{}),
		terminateStarted:   make(chan struct{}),
		killStarted:        make(chan struct{}),
		exitResult:         make(chan managerWaitResult, 1),
		readClosed:         make(chan struct{}),
		releaseOnTerminate: true,
		releaseOnKill:      true,
	}
}

func (p *managerFakeProcess) Read([]byte) (int, error) {
	p.readStartOnce.Do(func() {
		close(p.readStarted)
	})
	<-p.readClosed
	return 0, io.EOF
}

func (p *managerFakeProcess) Resize(rows, columns int) error {
	p.fakePTYProcess.mu.Lock()
	p.fakePTYProcess.resizes = append(p.fakePTYProcess.resizes, [2]int{rows, columns})
	p.fakePTYProcess.mu.Unlock()
	return p.resizeErr
}

func (p *managerFakeProcess) Wait() (int, error) {
	p.fakePTYProcess.mu.Lock()
	p.fakePTYProcess.waitCalls++
	p.fakePTYProcess.mu.Unlock()
	p.waitStartOnce.Do(func() {
		close(p.waitStarted)
	})
	result := <-p.exitResult
	return result.code, result.err
}

func (p *managerFakeProcess) Terminate() error {
	p.fakePTYProcess.mu.Lock()
	p.fakePTYProcess.terminateCalls++
	p.fakePTYProcess.mu.Unlock()
	p.terminateStartOnce.Do(func() {
		close(p.terminateStarted)
	})
	if p.releaseOnTerminate {
		p.exit(0, nil)
	}
	return p.terminateErr
}

func (p *managerFakeProcess) Kill() error {
	p.fakePTYProcess.mu.Lock()
	p.fakePTYProcess.killCalls++
	p.fakePTYProcess.mu.Unlock()
	p.killStartOnce.Do(func() {
		close(p.killStarted)
	})
	if p.releaseOnKill {
		p.exit(-1, nil)
	}
	return p.killErr
}

func (p *managerFakeProcess) Close() error {
	p.fakePTYProcess.mu.Lock()
	p.fakePTYProcess.closeCalls++
	p.fakePTYProcess.closed = true
	p.fakePTYProcess.mu.Unlock()
	p.readCloseOnce.Do(func() {
		close(p.readClosed)
	})
	return p.closeErr
}

func (p *managerFakeProcess) exit(code int, err error) {
	p.exitOnce.Do(func() {
		p.exitResult <- managerWaitResult{code: code, err: err}
	})
	p.readCloseOnce.Do(func() {
		close(p.readClosed)
	})
}

func (p *managerFakeProcess) snapshot() managerProcessSnapshot {
	p.fakePTYProcess.mu.Lock()
	defer p.fakePTYProcess.mu.Unlock()
	return managerProcessSnapshot{
		resizes:        append([][2]int(nil), p.fakePTYProcess.resizes...),
		waitCalls:      p.fakePTYProcess.waitCalls,
		terminateCalls: p.fakePTYProcess.terminateCalls,
		killCalls:      p.fakePTYProcess.killCalls,
		closeCalls:     p.fakePTYProcess.closeCalls,
		closed:         p.fakePTYProcess.closed,
	}
}

type managerProcessSnapshot struct {
	resizes        [][2]int
	waitCalls      int
	terminateCalls int
	killCalls      int
	closeCalls     int
	closed         bool
}
