package gui

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/terminal"
)

func TestWorkspaceCoordinatorStartsWelcomeAndPublishesOnOpen(t *testing.T) {
	builder := &fakeWorkspaceBuilder{roots: map[string]string{"alpha": t.TempDir()}}
	app := newWorkspaceCoordinator(builder.Build, nil)

	state := app.GetWorkspaceState()
	if state.Status != WorkspaceWelcome || state.Generation != 0 || state.Project != nil {
		t.Fatalf("initial state = %#v, want welcome generation zero", state)
	}
	result, err := app.OpenProject("alpha", "")
	if err != nil {
		t.Fatalf("OpenProject: %v", err)
	}
	if result.RequiresConfirmation || result.State.Status != WorkspaceOpen {
		t.Fatalf("open result = %#v", result)
	}
	if result.State.Generation != 1 || result.State.Project == nil ||
		result.State.Project.Root != builder.roots["alpha"] {
		t.Fatalf("opened state = %#v", result.State)
	}
}

func TestWorkspaceCoordinatorFailedCandidatePreservesPublishedGeneration(t *testing.T) {
	buildErr := errors.New("candidate failed")
	builder := &fakeWorkspaceBuilder{
		roots:  map[string]string{"alpha": t.TempDir()},
		errors: map[string]error{"broken": buildErr},
	}
	app := newWorkspaceCoordinator(builder.Build, nil)
	if _, err := app.OpenProject("alpha", ""); err != nil {
		t.Fatalf("open alpha: %v", err)
	}
	if _, err := app.OpenProject("broken", ""); !errors.Is(err, buildErr) {
		t.Fatalf("open broken = %v, want build error", err)
	}
	state := app.GetWorkspaceState()
	if state.Status != WorkspaceOpen || state.Generation != 1 ||
		state.Project == nil || state.Project.Root != builder.roots["alpha"] {
		t.Fatalf("state after failed switch = %#v", state)
	}
}

func TestWorkspaceCoordinatorRequiresFencedConfirmationAndCanCancel(t *testing.T) {
	builder := &fakeWorkspaceBuilder{
		roots: map[string]string{"alpha": t.TempDir(), "beta": t.TempDir()},
	}
	app := newWorkspaceCoordinator(builder.Build, nil)
	if _, err := app.OpenProject("alpha", ""); err != nil {
		t.Fatalf("open alpha: %v", err)
	}
	workspace, _ := app.currentWorkspace(1)
	workspace.recordTerminal(TerminalSession{
		SessionID: "active",
		State:     terminal.SessionRunning,
	})

	result, err := app.OpenProject("beta", "")
	if err != nil {
		t.Fatalf("request switch: %v", err)
	}
	if !result.RequiresConfirmation || result.ConfirmationToken == "" ||
		result.ActiveResources.Terminals != 1 {
		t.Fatalf("confirmation result = %#v", result)
	}
	if got := builder.callCount("beta"); got != 0 {
		t.Fatalf("candidate built before confirmation: %d calls", got)
	}
	if release, err := workspace.beginOperation(1, true); !errors.Is(err, errWorkspaceResourceFenced) {
		if release != nil {
			release()
		}
		t.Fatalf("resource admission while confirmation pending = %v", err)
	}
	if err := app.CancelWorkspaceChange(result.ConfirmationToken); err != nil {
		t.Fatalf("CancelWorkspaceChange: %v", err)
	}
	release, err := workspace.beginOperation(1, true)
	if err != nil {
		t.Fatalf("resource admission after cancel: %v", err)
	}
	release()

	result, err = app.OpenProject("beta", "")
	if err != nil {
		t.Fatalf("request second switch: %v", err)
	}
	confirmed, err := app.OpenProject("beta", result.ConfirmationToken)
	if err != nil {
		t.Fatalf("confirm switch: %v", err)
	}
	if confirmed.State.Generation != 2 || confirmed.State.Project.Root != builder.roots["beta"] {
		t.Fatalf("confirmed result = %#v", confirmed)
	}
	select {
	case <-workspace.Context().Done():
	default:
		t.Fatal("old workspace was not cancelled")
	}
}

func TestWorkspaceCoordinatorCloseReturnsToWelcomeWithoutExiting(t *testing.T) {
	builder := &fakeWorkspaceBuilder{roots: map[string]string{"alpha": t.TempDir()}}
	app := newWorkspaceCoordinator(builder.Build, nil)
	if _, err := app.OpenProject("alpha", ""); err != nil {
		t.Fatalf("open alpha: %v", err)
	}
	result, err := app.CloseProject("")
	if err != nil {
		t.Fatalf("CloseProject: %v", err)
	}
	if result.State.Status != WorkspaceClosed || result.State.Generation != 1 ||
		result.State.Project != nil {
		t.Fatalf("closed result = %#v", result)
	}
	state := app.GetWorkspaceState()
	if state.Status != WorkspaceWelcome || state.Generation != 1 {
		t.Fatalf("state after close = %#v", state)
	}
}

func TestWorkspaceCoordinatorRejectsStaleGeneration(t *testing.T) {
	builder := &fakeWorkspaceBuilder{roots: map[string]string{"alpha": t.TempDir()}}
	app := newWorkspaceCoordinator(builder.Build, nil)
	if _, err := app.OpenProject("alpha", ""); err != nil {
		t.Fatalf("open alpha: %v", err)
	}
	if _, err := app.currentWorkspace(2); !errors.Is(err, errStaleWorkspaceGeneration) {
		t.Fatalf("currentWorkspace wrong generation = %v, want stale", err)
	}
}

func TestWorkspaceConfirmationExpiresAndReleasesAdmissionFence(t *testing.T) {
	builder := &fakeWorkspaceBuilder{
		roots: map[string]string{"alpha": t.TempDir(), "beta": t.TempDir()},
	}
	app := newWorkspaceCoordinator(builder.Build, nil)
	app.confirmationTTL = 10 * time.Millisecond
	if _, err := app.OpenProject("alpha", ""); err != nil {
		t.Fatalf("open alpha: %v", err)
	}
	workspace, _ := app.currentWorkspace(1)
	workspace.recordTerminal(TerminalSession{
		SessionID: "active",
		State:     terminal.SessionRunning,
	})
	result, err := app.OpenProject("beta", "")
	if err != nil {
		t.Fatalf("request switch: %v", err)
	}

	deadline := time.Now().Add(time.Second)
	for {
		release, admissionErr := workspace.beginOperation(1, true)
		if admissionErr == nil {
			release()
			break
		}
		if !errors.Is(admissionErr, errWorkspaceResourceFenced) {
			t.Fatalf("resource admission while waiting for expiry = %v", admissionErr)
		}
		if time.Now().After(deadline) {
			t.Fatal("confirmation expiry did not release the admission fence")
		}
		time.Sleep(time.Millisecond)
	}
	if err := app.CancelWorkspaceChange(result.ConfirmationToken); !errors.Is(err, errInvalidConfirmation) {
		t.Fatalf("cancel expired confirmation = %v", err)
	}
}

type fakeWorkspaceBuilder struct {
	mu     sync.Mutex
	roots  map[string]string
	errors map[string]error
	calls  map[string]int
}

func (b *fakeWorkspaceBuilder) Build(path string, initialPlan uint64) (*WorkspaceContext, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.calls == nil {
		b.calls = make(map[string]int)
	}
	b.calls[path]++
	if err := b.errors[path]; err != nil {
		return nil, err
	}
	return newWorkspaceContext(workspaceContextConfig{
		root:        b.roots[path],
		dbPath:      path + ".db",
		name:        path,
		initialPlan: initialPlan,
		terminals:   &countingWorkspaceTerminalManager{},
	}), nil
}

func (b *fakeWorkspaceBuilder) callCount(path string) int {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.calls[path]
}

func TestWorkspaceCoordinatorConcurrentTransitionsRemainSerialized(t *testing.T) {
	builder := &fakeWorkspaceBuilder{
		roots: map[string]string{"alpha": t.TempDir(), "beta": t.TempDir()},
	}
	app := newWorkspaceCoordinator(builder.Build, nil)
	var wait sync.WaitGroup
	for i := range 20 {
		wait.Add(1)
		go func() {
			defer wait.Done()
			path := "alpha"
			if i%2 == 1 {
				path = "beta"
			}
			_, _ = app.OpenProject(path, "")
		}()
	}
	wait.Wait()
	state := app.GetWorkspaceState()
	if state.Status != WorkspaceOpen || state.Generation != 20 || state.Project == nil {
		t.Fatalf("serialized transition state = %#v", state)
	}
	app.onShutdown(context.Background())
}

func TestWorkspaceSwitchCancelsActiveRefreshBeforeTerminalCleanup(t *testing.T) {
	builder := &fakeWorkspaceBuilder{
		roots: map[string]string{"alpha": t.TempDir(), "beta": t.TempDir()},
	}
	app := newWorkspaceCoordinator(builder.Build, nil)
	if _, err := app.OpenProject("alpha", ""); err != nil {
		t.Fatalf("open alpha: %v", err)
	}
	old, _ := app.currentWorkspace(1)
	manager := old.terminals.(*countingWorkspaceTerminalManager)
	releaseRefresh, err := old.beginOperation(1, false)
	if err != nil {
		t.Fatalf("admit refresh: %v", err)
	}
	old.recordTerminal(TerminalSession{
		SessionID: "active-terminal",
		State:     terminal.SessionRunning,
	})
	request, err := app.OpenProject("beta", "")
	if err != nil || !request.RequiresConfirmation {
		t.Fatalf("request switch = %#v, %v", request, err)
	}

	switchDone := make(chan error, 1)
	go func() {
		_, switchErr := app.OpenProject("beta", request.ConfirmationToken)
		switchDone <- switchErr
	}()
	select {
	case <-old.Context().Done():
	case <-time.After(time.Second):
		t.Fatal("old refresh context was not cancelled")
	}
	manager.mu.Lock()
	shutdownBeforeRefreshRelease := manager.shutdownCalls
	manager.mu.Unlock()
	if shutdownBeforeRefreshRelease != 0 {
		t.Fatal("terminal cleanup began before the active refresh drained")
	}
	releaseRefresh()
	if err := <-switchDone; err != nil {
		t.Fatalf("confirm switch: %v", err)
	}
	manager.mu.Lock()
	shutdownCalls := manager.shutdownCalls
	manager.mu.Unlock()
	if shutdownCalls != 1 {
		t.Fatalf("terminal shutdown calls = %d, want 1", shutdownCalls)
	}
	if state := app.GetWorkspaceState(); state.Generation != 2 ||
		state.Project == nil || state.Project.Root != builder.roots["beta"] {
		t.Fatalf("state after switch = %#v", state)
	}
}
