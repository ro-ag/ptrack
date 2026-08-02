package gui

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"sync"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/terminal"
	wailsruntime "github.com/wailsapp/wails/v2/pkg/runtime"
)

type TerminalSession struct {
	SessionID string                `json:"sessionId"`
	ProfileID string                `json:"profileId"`
	CWD       string                `json:"cwd"`
	State     terminal.SessionState `json:"state"`
	StreamURL string                `json:"streamUrl"`
}

type TerminalStatus struct {
	Generation uint64                `json:"generation"`
	SessionID  string                `json:"sessionId"`
	State      terminal.SessionState `json:"state"`
}

type TerminalExit struct {
	Generation uint64                `json:"generation"`
	SessionID  string                `json:"sessionId"`
	ExitCode   int                   `json:"exitCode"`
	State      terminal.SessionState `json:"state"`
	Error      string                `json:"error,omitempty"`
}

type TerminalProfilesV2 struct {
	Generation uint64             `json:"generation"`
	Profiles   []terminal.Profile `json:"profiles"`
}

type TerminalSessionV2 struct {
	Generation uint64                `json:"generation"`
	SessionID  string                `json:"sessionId"`
	ProfileID  string                `json:"profileId"`
	CWD        string                `json:"cwd"`
	State      terminal.SessionState `json:"state"`
	StreamURL  string                `json:"streamUrl"`
}

type managedTerminalSession struct {
	SessionID      string
	ProfileID      string
	CWD            string
	State          terminal.SessionState
	StreamURL      string
	ProfileKind    terminal.ProfileKind
	Provider       string
	PID            int
	StartedAt      time.Time
	LastActivityAt time.Time
	exitResults    <-chan terminal.ExitResult
}

type terminalManager interface {
	Profiles() ([]terminal.Profile, error)
	Create(profileID, cwd string, rows, columns int) (managedTerminalSession, error)
	Resize(sessionID string, rows, columns int) error
	Close(sessionID string, force bool) error
	Shutdown(context.Context) error
}

type terminalEventEmitter func(context.Context, string, any)

type productionTerminalManager struct {
	manager *terminal.Manager
}

func (m productionTerminalManager) Profiles() ([]terminal.Profile, error) {
	return m.manager.Profiles(), nil
}

func (m productionTerminalManager) Create(
	profileID string,
	cwd string,
	rows int,
	columns int,
) (managedTerminalSession, error) {
	session, err := m.manager.Create(profileID, cwd, rows, columns)
	if err != nil {
		return managedTerminalSession{}, err
	}
	streamURL, err := m.manager.StreamURL(session.ID())
	if err != nil {
		return managedTerminalSession{}, errors.Join(
			err,
			m.manager.CloseSession(session.ID(), true),
		)
	}
	info := session.Info()
	return managedTerminalSession{
		SessionID:      session.ID(),
		ProfileID:      session.ProfileID(),
		CWD:            session.CWD(),
		State:          session.State(),
		StreamURL:      streamURL,
		ProfileKind:    info.ProfileKind,
		Provider:       info.Provider,
		PID:            info.PID,
		StartedAt:      info.StartedAt,
		LastActivityAt: info.LastActivityAt,
		exitResults:    session.ExitResults(),
	}, nil
}

func (m productionTerminalManager) Resize(sessionID string, rows, columns int) error {
	return m.manager.Resize(sessionID, rows, columns)
}

func (m productionTerminalManager) Close(sessionID string, force bool) error {
	return m.manager.CloseSession(sessionID, force)
}

func (m productionTerminalManager) Shutdown(ctx context.Context) error {
	return m.manager.Shutdown(ctx)
}

func (m productionTerminalManager) SessionSnapshot(limit int) []terminal.SessionInfo {
	return m.manager.SessionSnapshot(limit)
}

func newAppWithTerminal(
	dbPath string,
	initialPlan uint64,
	manager terminalManager,
	emitter terminalEventEmitter,
) (*App, error) {
	root := filepath.Dir(filepath.Dir(dbPath))
	absoluteRoot, err := filepath.Abs(root)
	if err != nil {
		return nil, fmt.Errorf("resolve GUI project root: %w", err)
	}
	canonicalRoot, err := filepath.EvalSymlinks(absoluteRoot)
	if err != nil {
		return nil, fmt.Errorf("canonicalize GUI project root: %w", err)
	}
	if emitter == nil {
		emitter = func(ctx context.Context, name string, payload any) {
			wailsruntime.EventsEmit(ctx, name, payload)
		}
	}
	app := newWorkspaceCoordinator(nil, emitter)
	workspace := newWorkspaceContext(workspaceContextConfig{
		generation:  1,
		root:        canonicalRoot,
		dbPath:      dbPath,
		name:        filepath.Base(canonicalRoot),
		initialPlan: initialPlan,
		terminals:   manager,
	})
	app.workspace = workspace
	app.workspaceStatus = WorkspaceOpen
	app.lastGeneration = 1
	app.syncLegacyWorkspaceFieldsLocked(workspace)
	return app, nil
}

func (a *App) GetTerminalProfiles() ([]terminal.Profile, error) {
	result, err := a.GetTerminalProfilesV2(0)
	return result.Profiles, err
}

func (a *App) GetTerminalProfilesV2(generation uint64) (TerminalProfilesV2, error) {
	workspace, manager, release, err := a.beginTerminalOperation(generation, false)
	if err != nil {
		return TerminalProfilesV2{}, err
	}
	defer release()
	if manager == nil {
		return TerminalProfilesV2{}, errors.New("terminal manager is unavailable")
	}
	profiles, err := manager.Profiles()
	if err != nil {
		return TerminalProfilesV2{}, err
	}
	copies := safeTerminalProfiles(profiles)
	return TerminalProfilesV2{
		Generation: workspace.Generation(),
		Profiles:   copies,
	}, nil
}

func safeTerminalProfiles(profiles []terminal.Profile) []terminal.Profile {
	copies := make([]terminal.Profile, len(profiles))
	for index, profile := range profiles {
		copies[index] = terminal.Profile{
			ID:       profile.ID,
			Name:     profile.Name,
			Kind:     profile.Kind,
			Provider: profile.Provider,
		}
	}
	return copies
}

func (a *App) CreateTerminal(
	profileID string,
	cwd string,
	rows int,
	columns int,
) (TerminalSession, error) {
	result, err := a.CreateTerminalV2(0, profileID, cwd, rows, columns)
	if err != nil {
		return TerminalSession{}, err
	}
	return TerminalSession{
		SessionID: result.SessionID,
		ProfileID: result.ProfileID,
		CWD:       result.CWD,
		State:     result.State,
		StreamURL: result.StreamURL,
	}, nil
}

func (a *App) CreateTerminalV2(
	generation uint64,
	profileID string,
	cwd string,
	rows int,
	columns int,
) (TerminalSessionV2, error) {
	workspace, manager, release, err := a.beginTerminalOperation(generation, true)
	if err != nil {
		return TerminalSessionV2{}, err
	}
	defer release()
	if manager == nil {
		return TerminalSessionV2{}, errors.New("terminal manager is unavailable")
	}
	if cwd == "" {
		cwd = workspace.root
	}
	session, err := manager.Create(profileID, cwd, rows, columns)
	if err != nil {
		return TerminalSessionV2{}, err
	}
	result := TerminalSessionV2{
		Generation: workspace.Generation(),
		SessionID:  session.SessionID,
		ProfileID:  session.ProfileID,
		CWD:        session.CWD,
		State:      session.State,
		StreamURL:  session.StreamURL,
	}
	workspace.recordTerminal(TerminalSession{
		SessionID: session.SessionID,
		ProfileID: session.ProfileID,
		CWD:       session.CWD,
		State:     session.State,
		StreamURL: session.StreamURL,
	})
	if session.ProfileKind == terminal.ProfileAgent {
		if registry := workspace.agentRegistry(); registry != nil {
			if _, registerErr := registry.RegisterLaunched(agentrun.Registration{
				Profile:    session.ProfileID,
				Provider:   session.Provider,
				PID:        session.PID,
				TerminalID: session.SessionID,
				CWD:        session.CWD,
			}); registerErr != nil {
				workspace.removeTerminal(session.SessionID)
				closeErr := manager.Close(session.SessionID, true)
				return TerminalSessionV2{}, errors.Join(registerErr, closeErr)
			}
		}
	}
	a.publishWorkspaceTerminalStatus(workspace, TerminalStatus{
		Generation: workspace.Generation(),
		SessionID:  session.SessionID,
		State:      session.State,
	})
	if session.exitResults != nil {
		a.monitorTerminalExit(workspace, session.SessionID, session.exitResults)
	}
	return result, nil
}

func (a *App) ResizeTerminal(sessionID string, rows, columns int) error {
	return a.ResizeTerminalV2(0, sessionID, rows, columns)
}

func (a *App) ResizeTerminalV2(
	generation uint64,
	sessionID string,
	rows int,
	columns int,
) error {
	_, manager, release, err := a.beginTerminalOperation(generation, false)
	if err != nil {
		return err
	}
	defer release()
	if manager == nil {
		return errors.New("terminal manager is unavailable")
	}
	return manager.Resize(sessionID, rows, columns)
}

func (a *App) CloseTerminal(sessionID string, force bool) error {
	return a.CloseTerminalV2(0, sessionID, force)
}

func (a *App) CloseTerminalV2(
	generation uint64,
	sessionID string,
	force bool,
) error {
	workspace, manager, release, err := a.beginTerminalOperation(generation, false)
	if err != nil {
		return err
	}
	defer release()
	if manager == nil {
		return errors.New("terminal manager is unavailable")
	}
	if err := manager.Close(sessionID, force); err != nil {
		return err
	}
	workspace.removeTerminal(sessionID)
	a.publishWorkspaceTerminalStatus(workspace, TerminalStatus{
		Generation: workspace.Generation(),
		SessionID:  sessionID,
		State:      terminal.SessionClosed,
	})
	return nil
}

func (a *App) onStartup(ctx context.Context) {
	a.lifecycleMu.Lock()
	if a.shuttingDown {
		a.lifecycleMu.Unlock()
		return
	}
	if a.monitorCancel != nil {
		a.monitorCancel()
	}
	a.wailsContext = ctx
	a.monitorCtx, a.monitorCancel = context.WithCancel(ctx)
	a.lifecycleMu.Unlock()
	a.startupOnce.Do(func() {
		close(a.startupReady)
	})
	a.workspaceMu.RLock()
	workspace := a.workspace
	a.workspaceMu.RUnlock()
	if workspace != nil {
		a.startWorkspaceWatcher(workspace)
	}
}

func (a *App) onShutdown(ctx context.Context) {
	a.shutdownOnce.Do(func() {
		a.lifecycleMu.Lock()
		a.shuttingDown = true
		a.shutdownSignalOnce.Do(func() {
			close(a.shutdownStarted)
		})
		a.lifecycleMu.Unlock()
		a.transitionMu.Lock()
		a.clearConfirmationLocked()
		a.workspaceMu.Lock()
		workspace := a.workspace
		a.workspace = nil
		a.syncLegacyWorkspaceFieldsLocked(nil)
		a.workspaceMu.Unlock()
		a.transitionMu.Unlock()
		a.lifecycleMu.Lock()
		monitorCancel := a.monitorCancel
		a.lifecycleMu.Unlock()
		if monitorCancel != nil {
			monitorCancel()
		}
		_ = closeWorkspaceWithTimeout(workspace)
		a.stopWorkspaceWatcher()
		waitTimeout := a.shutdownWaitTimeout
		if waitTimeout <= 0 {
			waitTimeout = 3 * time.Second
		}
		waitForLifecycleGroup(&a.terminalOps, waitTimeout)
		waitForLifecycleGroup(&a.monitorWG, waitTimeout)
		a.lifecycleMu.Lock()
		if a.monitorCancel != nil {
			a.monitorCancel()
		}
		a.lifecycleMu.Unlock()
	})
}

func waitForLifecycleGroup(group *lifecycleGroup, timeout time.Duration) bool {
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	return group.WaitContext(ctx) == nil
}

func (a *App) publishTerminalStatus(status TerminalStatus) {
	a.emitTerminalEvent("terminal:status", status)
}

func (a *App) publishTerminalExit(result TerminalExit) {
	a.emitTerminalEvent("terminal:exit", result)
}

func (a *App) publishWorkspaceTerminalStatus(
	workspace *WorkspaceContext,
	status TerminalStatus,
) {
	if a.workspaceIsPublished(workspace) {
		a.publishTerminalStatus(status)
	}
}

func (a *App) publishWorkspaceTerminalExit(
	workspace *WorkspaceContext,
	result TerminalExit,
) {
	if a.workspaceIsPublished(workspace) {
		a.publishTerminalExit(result)
	}
}

func (a *App) workspaceIsPublished(workspace *WorkspaceContext) bool {
	a.workspaceMu.RLock()
	defer a.workspaceMu.RUnlock()
	return workspace != nil && a.workspace == workspace &&
		a.workspace.Generation() == workspace.Generation()
}

func (a *App) emitTerminalEvent(name string, payload any) {
	a.lifecycleMu.Lock()
	ctx := a.wailsContext
	emitter := a.emitTerminal
	a.lifecycleMu.Unlock()
	if ctx == nil || emitter == nil {
		return
	}
	emitter(ctx, name, payload)
}

func (a *App) monitorTerminalExit(
	workspace *WorkspaceContext,
	sessionID string,
	results <-chan terminal.ExitResult,
) {
	a.lifecycleMu.Lock()
	if a.wailsContext == nil || a.shuttingDown {
		a.lifecycleMu.Unlock()
		return
	}
	a.monitorWG.Add(1)
	a.lifecycleMu.Unlock()
	go func() {
		defer a.monitorWG.Done()
		select {
		case result, ok := <-results:
			if !ok {
				return
			}
			workspace.removeTerminal(sessionID)
			exit := TerminalExit{
				Generation: workspace.Generation(),
				SessionID:  sessionID,
				ExitCode:   result.ExitCode,
				State:      result.State,
			}
			if result.Err != nil {
				exit.Error = result.Err.Error()
			}
			if registry := workspace.agentRegistry(); registry != nil {
				resultText := "exited"
				if result.Err != nil {
					resultText = "failed"
				}
				registry.RecordTerminalExit(sessionID, result.ExitCode, resultText)
			}
			a.publishWorkspaceTerminalExit(workspace, exit)
		case <-workspace.Context().Done():
		}
	}()
}

func (a *App) beginTerminalOperation(
	expectedGeneration uint64,
	resourceAdmission bool,
) (*WorkspaceContext, terminalManager, func(), error) {
	select {
	case <-a.startupReady:
	case <-a.shutdownStarted:
		return nil, nil, nil, errors.New("terminal lifecycle is shutting down")
	}
	a.lifecycleMu.Lock()
	if a.shuttingDown {
		a.lifecycleMu.Unlock()
		return nil, nil, nil, errors.New("terminal lifecycle is shutting down")
	}
	a.terminalOps.Add(1)
	a.lifecycleMu.Unlock()

	workspace, err := a.currentWorkspace(expectedGeneration)
	if err != nil {
		a.terminalOps.Done()
		return nil, nil, nil, err
	}
	releaseWorkspace, err := workspace.beginOperation(expectedGeneration, resourceAdmission)
	if err != nil {
		a.terminalOps.Done()
		return nil, nil, nil, err
	}
	var once sync.Once
	release := func() {
		once.Do(func() {
			releaseWorkspace()
			a.terminalOps.Done()
		})
	}
	return workspace, workspace.terminalManager(), release, nil
}

var _ terminalManager = productionTerminalManager{}
