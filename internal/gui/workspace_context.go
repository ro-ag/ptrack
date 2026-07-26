package gui

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"time"

	"github.com/ro-ag/ptrack/internal/terminal"
)

var (
	errWorkspaceClosing         = errors.New("workspace is closing")
	errWorkspaceResourceFenced  = errors.New("workspace resource admission is fenced")
	errStaleWorkspaceGeneration = errors.New("stale workspace generation")
)

type workspaceShutdowner interface {
	Shutdown(context.Context) error
}

type workspaceCloseTimeouts struct {
	operations time.Duration
	terminals  time.Duration
	agents     time.Duration
}

type workspaceContextConfig struct {
	generation  uint64
	root        string
	dbPath      string
	name        string
	initialPlan uint64
	terminals   workspaceShutdowner
	agents      workspaceShutdowner
	timeouts    workspaceCloseTimeouts
}

// WorkspaceContext owns every resource associated with one published project
// generation. It never retains an open project store.
type WorkspaceContext struct {
	generation  uint64
	root        string
	dbPath      string
	name        string
	initialPlan uint64
	terminals   workspaceShutdowner
	agents      workspaceShutdowner
	timeouts    workspaceCloseTimeouts

	ctx    context.Context
	cancel context.CancelFunc

	mu                 sync.Mutex
	closing            bool
	operations         int
	resourceOperations int
	operationsDone     chan struct{}
	operationsOnce     sync.Once
	resourceFences     int
	resourceRevision   uint64
	terminalSessions   map[string]TerminalSession

	closeOnce sync.Once
	closeDone chan struct{}
	closeErr  error
}

func newWorkspaceContext(config workspaceContextConfig) *WorkspaceContext {
	ctx, cancel := context.WithCancel(context.Background())
	if config.timeouts.operations <= 0 {
		config.timeouts.operations = 3 * time.Second
	}
	if config.timeouts.terminals <= 0 {
		config.timeouts.terminals = 2500 * time.Millisecond
	}
	if config.timeouts.agents <= 0 {
		config.timeouts.agents = 2 * time.Second
	}
	return &WorkspaceContext{
		generation:       config.generation,
		root:             config.root,
		dbPath:           config.dbPath,
		name:             config.name,
		initialPlan:      config.initialPlan,
		terminals:        config.terminals,
		agents:           config.agents,
		timeouts:         config.timeouts,
		ctx:              ctx,
		cancel:           cancel,
		operationsDone:   make(chan struct{}),
		closeDone:        make(chan struct{}),
		terminalSessions: make(map[string]TerminalSession),
	}
}

func (w *WorkspaceContext) Context() context.Context {
	return w.ctx
}

func (w *WorkspaceContext) Generation() uint64 {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.generation
}

func (w *WorkspaceContext) setGeneration(generation uint64) {
	w.mu.Lock()
	w.generation = generation
	w.mu.Unlock()
}

// beginOperation admits one operation for the expected generation and returns
// an idempotent release function.
func (w *WorkspaceContext) beginOperation(
	expectedGeneration uint64,
	resourceAdmission bool,
) (func(), error) {
	w.mu.Lock()
	defer w.mu.Unlock()
	if expectedGeneration != 0 && expectedGeneration != w.generation {
		return nil, fmt.Errorf(
			"%w: expected %d, active %d",
			errStaleWorkspaceGeneration,
			expectedGeneration,
			w.generation,
		)
	}
	if w.closing {
		return nil, errWorkspaceClosing
	}
	if resourceAdmission && w.resourceFences > 0 {
		return nil, errWorkspaceResourceFenced
	}
	w.operations++
	if resourceAdmission {
		w.resourceOperations++
		w.resourceRevision++
	}
	var once sync.Once
	return func() {
		once.Do(func() { w.finishOperation(resourceAdmission) })
	}, nil
}

func (w *WorkspaceContext) terminalManager() terminalManager {
	manager, _ := w.terminals.(terminalManager)
	return manager
}

func (w *WorkspaceContext) recordTerminal(session TerminalSession) {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.terminalSessions[session.SessionID] = session
	w.resourceRevision++
}

func (w *WorkspaceContext) removeTerminal(sessionID string) {
	w.mu.Lock()
	defer w.mu.Unlock()
	if _, exists := w.terminalSessions[sessionID]; exists {
		delete(w.terminalSessions, sessionID)
		w.resourceRevision++
	}
}

func (w *WorkspaceContext) activeResourceSummary() ActiveResourceSummary {
	w.mu.Lock()
	defer w.mu.Unlock()
	active := 0
	for _, session := range w.terminalSessions {
		switch session.State {
		case terminal.SessionRunning, terminal.SessionStarting:
			active++
		}
	}
	summary := ActiveResourceSummary{
		Terminals:         active,
		PendingAdmissions: w.resourceOperations,
		ResourceRevision:  w.resourceRevision,
	}
	if agents, ok := w.agents.(interface{ ActiveCount() int }); ok {
		summary.AgentRuns = agents.ActiveCount()
	}
	return summary
}

func (w *WorkspaceContext) finishOperation(resourceAdmission bool) {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.operations--
	if resourceAdmission {
		w.resourceOperations--
		w.resourceRevision++
	}
	if w.closing && w.operations == 0 {
		w.operationsOnce.Do(func() { close(w.operationsDone) })
	}
}

// fenceResourceAdmission rejects new terminal and AgentRun admissions until
// the returned idempotent release function is called.
func (w *WorkspaceContext) fenceResourceAdmission() func() {
	w.mu.Lock()
	w.resourceFences++
	w.mu.Unlock()
	var releaseAgents func()
	if agents, ok := w.agents.(interface{ FenceAdmission() func() }); ok {
		releaseAgents = agents.FenceAdmission()
	}
	var once sync.Once
	return func() {
		once.Do(func() {
			w.mu.Lock()
			if w.resourceFences > 0 {
				w.resourceFences--
			}
			w.mu.Unlock()
			if releaseAgents != nil {
				releaseAgents()
			}
		})
	}
}

func (w *WorkspaceContext) bumpResourceRevision() uint64 {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.resourceRevision++
	return w.resourceRevision
}

func (w *WorkspaceContext) resourceRevisionValue() uint64 {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.resourceRevision
}

// Close starts teardown once. Each caller independently waits for completion
// with its own context.
func (w *WorkspaceContext) Close(ctx context.Context) error {
	w.closeOnce.Do(func() {
		w.mu.Lock()
		w.closing = true
		w.cancel()
		if w.operations == 0 {
			w.operationsOnce.Do(func() { close(w.operationsDone) })
		}
		w.mu.Unlock()
		go w.runClose()
	})
	select {
	case <-w.closeDone:
		return w.closeErr
	case <-ctx.Done():
		return ctx.Err()
	}
}

func (w *WorkspaceContext) runClose() {
	var closeErrors []error
	operationsTimer := time.NewTimer(w.timeouts.operations)
	select {
	case <-w.operationsDone:
		if !operationsTimer.Stop() {
			<-operationsTimer.C
		}
	case <-operationsTimer.C:
		closeErrors = append(closeErrors, errors.New("wait for workspace operations: timeout"))
	}

	type resourceResult struct {
		name string
		err  error
	}
	resources := []struct {
		name    string
		timeout time.Duration
		value   workspaceShutdowner
	}{
		{name: "terminal", timeout: w.timeouts.terminals, value: w.terminals},
		{name: "agent", timeout: w.timeouts.agents, value: w.agents},
	}
	results := make(chan resourceResult, len(resources))
	var wait sync.WaitGroup
	for _, resource := range resources {
		if resource.value == nil {
			continue
		}
		wait.Add(1)
		go func() {
			defer wait.Done()
			resourceCtx, cancel := context.WithTimeout(context.Background(), resource.timeout)
			defer cancel()
			shutdownDone := make(chan error, 1)
			go func() {
				shutdownDone <- resource.value.Shutdown(resourceCtx)
			}()
			var shutdownErr error
			select {
			case shutdownErr = <-shutdownDone:
			case <-resourceCtx.Done():
				shutdownErr = resourceCtx.Err()
			}
			results <- resourceResult{
				name: resource.name,
				err:  shutdownErr,
			}
		}()
	}
	wait.Wait()
	close(results)
	for result := range results {
		if result.err != nil {
			closeErrors = append(closeErrors, fmt.Errorf("%s workspace cleanup: %w", result.name, result.err))
		}
	}
	w.closeErr = errors.Join(closeErrors...)
	close(w.closeDone)
}
