package gui

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"time"

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
	SessionID string                `json:"sessionId"`
	State     terminal.SessionState `json:"state"`
}

type TerminalExit struct {
	SessionID string                `json:"sessionId"`
	ExitCode  int                   `json:"exitCode"`
	State     terminal.SessionState `json:"state"`
	Error     string                `json:"error,omitempty"`
}

type managedTerminalSession struct {
	SessionID   string
	ProfileID   string
	CWD         string
	State       terminal.SessionState
	StreamURL   string
	exitResults <-chan terminal.ExitResult
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
	return managedTerminalSession{
		SessionID:   session.ID(),
		ProfileID:   session.ProfileID(),
		CWD:         session.CWD(),
		State:       session.State(),
		StreamURL:   streamURL,
		exitResults: session.ExitResults(),
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
	return &App{
		dbPath:          dbPath,
		initialPlan:     initialPlan,
		projectName:     filepath.Base(canonicalRoot),
		projectRoot:     canonicalRoot,
		terminals:       manager,
		emitTerminal:    emitter,
		startupReady:    make(chan struct{}),
		shutdownStarted: make(chan struct{}),
	}, nil
}

func (a *App) GetTerminalProfiles() ([]terminal.Profile, error) {
	if err := a.beginTerminalOperation(); err != nil {
		return nil, err
	}
	defer a.terminalOps.Done()
	if a.terminals == nil {
		return nil, errors.New("terminal manager is unavailable")
	}
	profiles, err := a.terminals.Profiles()
	if err != nil {
		return nil, err
	}
	copies := make([]terminal.Profile, len(profiles))
	for index, profile := range profiles {
		copies[index] = terminal.Profile{
			ID:   profile.ID,
			Name: profile.Name,
			Kind: profile.Kind,
		}
	}
	return copies, nil
}

func (a *App) CreateTerminal(
	profileID string,
	cwd string,
	rows int,
	columns int,
) (TerminalSession, error) {
	if err := a.beginTerminalOperation(); err != nil {
		return TerminalSession{}, err
	}
	defer a.terminalOps.Done()
	if a.terminals == nil {
		return TerminalSession{}, errors.New("terminal manager is unavailable")
	}
	if cwd == "" {
		cwd = a.projectRoot
	}
	session, err := a.terminals.Create(profileID, cwd, rows, columns)
	if err != nil {
		return TerminalSession{}, err
	}
	result := TerminalSession{
		SessionID: session.SessionID,
		ProfileID: session.ProfileID,
		CWD:       session.CWD,
		State:     session.State,
		StreamURL: session.StreamURL,
	}
	a.publishTerminalStatus(TerminalStatus{SessionID: session.SessionID, State: session.State})
	if session.exitResults != nil {
		a.monitorTerminalExit(session.SessionID, session.exitResults)
	}
	return result, nil
}

func (a *App) ResizeTerminal(sessionID string, rows, columns int) error {
	if err := a.beginTerminalOperation(); err != nil {
		return err
	}
	defer a.terminalOps.Done()
	if a.terminals == nil {
		return errors.New("terminal manager is unavailable")
	}
	return a.terminals.Resize(sessionID, rows, columns)
}

func (a *App) CloseTerminal(sessionID string, force bool) error {
	if err := a.beginTerminalOperation(); err != nil {
		return err
	}
	defer a.terminalOps.Done()
	if a.terminals == nil {
		return errors.New("terminal manager is unavailable")
	}
	if err := a.terminals.Close(sessionID, force); err != nil {
		return err
	}
	a.publishTerminalStatus(TerminalStatus{SessionID: sessionID, State: terminal.SessionClosed})
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
}

func (a *App) onShutdown(ctx context.Context) {
	a.shutdownOnce.Do(func() {
		a.lifecycleMu.Lock()
		a.shuttingDown = true
		a.shutdownSignalOnce.Do(func() {
			close(a.shutdownStarted)
		})
		a.lifecycleMu.Unlock()
		a.terminalOps.Wait()
		if a.terminals != nil {
			shutdownCtx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
			_ = a.terminals.Shutdown(shutdownCtx)
			cancel()
		}
		a.lifecycleMu.Lock()
		if a.monitorCancel != nil {
			a.monitorCancel()
		}
		a.lifecycleMu.Unlock()
		a.monitorWG.Wait()
	})
}

func (a *App) publishTerminalStatus(status TerminalStatus) {
	a.emitTerminalEvent("terminal:status", status)
}

func (a *App) publishTerminalExit(result TerminalExit) {
	a.emitTerminalEvent("terminal:exit", result)
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

func (a *App) monitorTerminalExit(sessionID string, results <-chan terminal.ExitResult) {
	a.lifecycleMu.Lock()
	ctx := a.monitorCtx
	if ctx == nil || a.shuttingDown {
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
			exit := TerminalExit{
				SessionID: sessionID,
				ExitCode:  result.ExitCode,
				State:     result.State,
			}
			if result.Err != nil {
				exit.Error = result.Err.Error()
			}
			a.publishTerminalExit(exit)
		case <-ctx.Done():
		}
	}()
}

func (a *App) beginTerminalOperation() error {
	select {
	case <-a.startupReady:
	case <-a.shutdownStarted:
		return errors.New("terminal lifecycle is shutting down")
	}
	a.lifecycleMu.Lock()
	defer a.lifecycleMu.Unlock()
	if a.shuttingDown {
		return errors.New("terminal lifecycle is shutting down")
	}
	a.terminalOps.Add(1)
	return nil
}

var _ terminalManager = productionTerminalManager{}
