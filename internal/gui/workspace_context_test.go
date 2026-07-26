package gui

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"
)

func TestWorkspaceContextAdmitsOperationsThenRejectsAfterCloseStarts(t *testing.T) {
	workspace := newWorkspaceContext(workspaceContextConfig{
		generation: 7,
		root:       t.TempDir(),
		dbPath:     "test.db",
	})
	release, err := workspace.beginOperation(7, false)
	if err != nil {
		t.Fatalf("beginOperation: %v", err)
	}
	release()

	closeDone := make(chan error, 1)
	go func() {
		closeDone <- workspace.Close(context.Background())
	}()
	select {
	case <-workspace.Context().Done():
	case <-time.After(time.Second):
		t.Fatal("workspace context was not cancelled when close started")
	}
	if _, err := workspace.beginOperation(7, false); !errors.Is(err, errWorkspaceClosing) {
		t.Fatalf("beginOperation after close = %v, want errWorkspaceClosing", err)
	}
	if err := <-closeDone; err != nil {
		t.Fatalf("Close: %v", err)
	}
}

func TestWorkspaceContextCloseIsIdempotentAndWaitsForAdmittedWork(t *testing.T) {
	manager := &countingWorkspaceTerminalManager{}
	workspace := newWorkspaceContext(workspaceContextConfig{
		generation: 3,
		root:       t.TempDir(),
		dbPath:     "test.db",
		terminals:  manager,
	})
	release, err := workspace.beginOperation(3, false)
	if err != nil {
		t.Fatalf("beginOperation: %v", err)
	}

	const callers = 8
	results := make(chan error, callers)
	for range callers {
		go func() {
			results <- workspace.Close(context.Background())
		}()
	}
	select {
	case <-manager.shutdownStarted:
		t.Fatal("terminal shutdown raced admitted workspace work")
	case <-time.After(30 * time.Millisecond):
	}
	release()
	for range callers {
		if err := <-results; err != nil {
			t.Fatalf("Close: %v", err)
		}
	}
	if got := manager.shutdownCount(); got != 1 {
		t.Fatalf("terminal Shutdown calls = %d, want 1", got)
	}
}

func TestWorkspaceContextCloseCallerCanTimeOutThenObserveEventualCleanup(t *testing.T) {
	manager := newBlockingWorkspaceTerminalManager()
	workspace := newWorkspaceContext(workspaceContextConfig{
		generation: 1,
		root:       t.TempDir(),
		dbPath:     "test.db",
		terminals:  manager,
		timeouts: workspaceCloseTimeouts{
			operations: time.Second,
			terminals:  time.Second,
			agents:     time.Second,
		},
	})

	shortCtx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	if err := workspace.Close(shortCtx); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("short Close = %v, want deadline", err)
	}
	select {
	case <-manager.started:
	case <-time.After(time.Second):
		t.Fatal("terminal shutdown did not start")
	}
	close(manager.release)
	if err := workspace.Close(context.Background()); err != nil {
		t.Fatalf("later Close: %v", err)
	}
	if got := manager.shutdownCount(); got != 1 {
		t.Fatalf("terminal Shutdown calls = %d, want 1", got)
	}
}

func TestWorkspaceContextCloseJoinsResourceErrors(t *testing.T) {
	terminalErr := errors.New("terminal cleanup")
	agentErr := errors.New("agent cleanup")
	workspace := newWorkspaceContext(workspaceContextConfig{
		generation: 1,
		root:       t.TempDir(),
		dbPath:     "test.db",
		terminals:  &countingWorkspaceTerminalManager{shutdownErr: terminalErr},
		agents:     fakeWorkspaceAgentResource{shutdownErr: agentErr},
	})
	err := workspace.Close(context.Background())
	if !errors.Is(err, terminalErr) || !errors.Is(err, agentErr) {
		t.Fatalf("Close error = %v, want joined terminal and agent errors", err)
	}
}

func TestWorkspaceContextCloseIsBoundedWhenResourceIgnoresContext(t *testing.T) {
	resource := newIgnoringWorkspaceShutdowner()
	workspace := newWorkspaceContext(workspaceContextConfig{
		generation: 1,
		root:       t.TempDir(),
		dbPath:     "test.db",
		terminals:  resource,
		timeouts: workspaceCloseTimeouts{
			operations: 20 * time.Millisecond,
			terminals:  20 * time.Millisecond,
			agents:     20 * time.Millisecond,
		},
	})
	done := make(chan error, 1)
	go func() {
		done <- workspace.Close(context.Background())
	}()
	select {
	case err := <-done:
		if !errors.Is(err, context.DeadlineExceeded) {
			t.Fatalf("Close error = %v, want resource deadline", err)
		}
	case <-time.After(250 * time.Millisecond):
		t.Fatal("Close remained blocked after its resource deadline")
	}
	close(resource.release)
	select {
	case <-resource.returned:
	case <-time.After(time.Second):
		t.Fatal("isolated resource shutdown did not finish after release")
	}
}

func TestWorkspaceContextGenerationAndResourceFence(t *testing.T) {
	workspace := newWorkspaceContext(workspaceContextConfig{
		generation: 9,
		root:       t.TempDir(),
		dbPath:     "test.db",
	})
	if _, err := workspace.beginOperation(8, false); !errors.Is(err, errStaleWorkspaceGeneration) {
		t.Fatalf("wrong generation = %v, want stale generation", err)
	}
	releaseFence := workspace.fenceResourceAdmission()
	if _, err := workspace.beginOperation(9, true); !errors.Is(err, errWorkspaceResourceFenced) {
		t.Fatalf("resource operation during fence = %v, want fenced", err)
	}
	releaseFence()
	release, err := workspace.beginOperation(9, true)
	if err != nil {
		t.Fatalf("resource operation after fence: %v", err)
	}
	release()
}

type countingWorkspaceTerminalManager struct {
	mu              sync.Mutex
	shutdownCalls   int
	shutdownErr     error
	shutdownStarted chan struct{}
	startOnce       sync.Once
}

func (m *countingWorkspaceTerminalManager) shutdownCount() int {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.shutdownCalls
}

func (m *countingWorkspaceTerminalManager) Shutdown(context.Context) error {
	m.mu.Lock()
	m.shutdownCalls++
	m.mu.Unlock()
	if m.shutdownStarted != nil {
		m.startOnce.Do(func() { close(m.shutdownStarted) })
	}
	return m.shutdownErr
}

type blockingWorkspaceTerminalManager struct {
	countingWorkspaceTerminalManager
	started chan struct{}
	release chan struct{}
}

func newBlockingWorkspaceTerminalManager() *blockingWorkspaceTerminalManager {
	return &blockingWorkspaceTerminalManager{
		started: make(chan struct{}),
		release: make(chan struct{}),
	}
}

func (m *blockingWorkspaceTerminalManager) Shutdown(ctx context.Context) error {
	m.mu.Lock()
	m.shutdownCalls++
	m.mu.Unlock()
	m.startOnce.Do(func() { close(m.started) })
	select {
	case <-m.release:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

type fakeWorkspaceAgentResource struct {
	shutdownErr error
}

func (f fakeWorkspaceAgentResource) Shutdown(context.Context) error {
	return f.shutdownErr
}

type ignoringWorkspaceShutdowner struct {
	release  chan struct{}
	returned chan struct{}
}

func newIgnoringWorkspaceShutdowner() *ignoringWorkspaceShutdowner {
	return &ignoringWorkspaceShutdowner{
		release:  make(chan struct{}),
		returned: make(chan struct{}),
	}
}

func (s *ignoringWorkspaceShutdowner) Shutdown(context.Context) error {
	<-s.release
	close(s.returned)
	return nil
}
