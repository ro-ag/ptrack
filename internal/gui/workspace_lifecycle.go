package gui

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"errors"
	"fmt"
	"time"

	"github.com/ro-ag/ptrack/internal/gitinfo"
	"github.com/ro-ag/ptrack/internal/store"
)

type WorkspaceStatus string

const (
	WorkspaceWelcome WorkspaceStatus = "welcome"
	WorkspaceLoading WorkspaceStatus = "loading"
	WorkspaceOpen    WorkspaceStatus = "open"
	WorkspaceError   WorkspaceStatus = "error"
	WorkspaceClosed  WorkspaceStatus = "closed"
)

var (
	errNoWorkspace         = errors.New("no project workspace is open")
	errInvalidConfirmation = errors.New("invalid or expired workspace confirmation")
)

type WorkspaceProject struct {
	Name   string `json:"name"`
	Root   string `json:"root"`
	DBPath string `json:"dbPath"`
}

type WorkspaceState struct {
	Status     WorkspaceStatus   `json:"status"`
	Generation uint64            `json:"generation"`
	Version    string            `json:"version"`
	Project    *WorkspaceProject `json:"project,omitempty"`
	Error      string            `json:"error,omitempty"`
}

type ActiveResourceSummary struct {
	Terminals         int    `json:"terminals"`
	AgentRuns         int    `json:"agentRuns"`
	PendingAdmissions int    `json:"pendingAdmissions"`
	ResourceRevision  uint64 `json:"resourceRevision"`
}

type WorkspaceChangeResult struct {
	State                WorkspaceState        `json:"state"`
	RequiresConfirmation bool                  `json:"requiresConfirmation"`
	ConfirmationToken    string                `json:"confirmationToken,omitempty"`
	ActiveResources      ActiveResourceSummary `json:"activeResources"`
	Warning              string                `json:"warning,omitempty"`
}

type workspaceBuilder func(path string, initialPlan uint64) (*WorkspaceContext, error)

type workspaceConfirmation struct {
	token      string
	action     string
	path       string
	generation uint64
	revision   uint64
	expiresAt  time.Time
	release    func()
}

func newWorkspaceCoordinator(
	builder workspaceBuilder,
	emitter terminalEventEmitter,
) *App {
	if emitter == nil {
		emitter = func(context.Context, string, any) {}
	}
	return &App{
		buildWorkspace:      builder,
		workspaceStatus:     WorkspaceWelcome,
		emitTerminal:        emitter,
		gitSnapshots:        gitinfo.Service{},
		gitWorktrees:        gitinfo.Service{},
		confirmationTTL:     time.Minute,
		shutdownWaitTimeout: 3 * time.Second,
		terminalAttachLease: 30 * time.Second,
		terminalAttachAfter: time.After,
		startupReady:        make(chan struct{}),
		shutdownStarted:     make(chan struct{}),
		updateState: UpdateState{
			Phase:          UpdateIdle,
			CurrentVersion: store.WriterVersion,
		},
	}
}

func (a *App) GetWorkspaceState() WorkspaceState {
	a.workspaceMu.RLock()
	defer a.workspaceMu.RUnlock()
	return a.workspaceStateLocked()
}

func (a *App) workspaceStateLocked() WorkspaceState {
	state := WorkspaceState{
		Status:     a.workspaceStatus,
		Generation: a.lastGeneration,
		Version:    store.WriterVersion,
		Error:      a.workspaceError,
	}
	if a.workspace != nil {
		state.Generation = a.workspace.Generation()
		state.Project = &WorkspaceProject{
			Name:   a.workspace.name,
			Root:   a.workspace.root,
			DBPath: a.workspace.dbPath,
		}
	}
	return state
}

func (a *App) currentWorkspace(expectedGeneration uint64) (*WorkspaceContext, error) {
	a.workspaceMu.RLock()
	workspace := a.workspace
	a.workspaceMu.RUnlock()
	if workspace == nil {
		return nil, errNoWorkspace
	}
	if expectedGeneration != 0 && workspace.Generation() != expectedGeneration {
		return nil, fmt.Errorf(
			"%w: expected %d, active %d",
			errStaleWorkspaceGeneration,
			expectedGeneration,
			workspace.Generation(),
		)
	}
	return workspace, nil
}

// OpenProject opens or switches to path. A non-empty confirmation token must
// match the active fenced transition.
func (a *App) OpenProject(path, confirmationToken string) (WorkspaceChangeResult, error) {
	a.transitionMu.Lock()
	defer a.transitionMu.Unlock()

	var releaseFence func()
	if confirmationToken == "" {
		a.clearConfirmationLocked()
		if current, err := a.currentWorkspace(0); err == nil {
			releaseFence = current.fenceResourceAdmission()
			summary := current.activeResourceSummary()
			if summary.Terminals > 0 ||
				summary.AgentRuns > 0 ||
				summary.PendingAdmissions > 0 {
				token, tokenErr := randomWorkspaceToken()
				if tokenErr != nil {
					releaseFence()
					return WorkspaceChangeResult{}, tokenErr
				}
				a.confirmation = &workspaceConfirmation{
					token:      token,
					action:     "open",
					path:       path,
					generation: current.Generation(),
					revision:   summary.ResourceRevision,
					expiresAt:  time.Now().Add(a.workspaceConfirmationTTL()),
					release:    releaseFence,
				}
				a.scheduleConfirmationExpiryLocked(token)
				return WorkspaceChangeResult{
					State:                a.GetWorkspaceState(),
					RequiresConfirmation: true,
					ConfirmationToken:    token,
					ActiveResources:      summary,
				}, nil
			}
		}
	} else {
		confirmation := a.confirmation
		if confirmation == nil || confirmation.token != confirmationToken ||
			confirmation.action != "open" || confirmation.path != path ||
			time.Now().After(confirmation.expiresAt) {
			a.clearConfirmationLocked()
			return WorkspaceChangeResult{}, errInvalidConfirmation
		}
		current, err := a.currentWorkspace(confirmation.generation)
		if err != nil || current.resourceRevisionValue() != confirmation.revision {
			a.clearConfirmationLocked()
			return WorkspaceChangeResult{}, errInvalidConfirmation
		}
		releaseFence = confirmation.release
		a.confirmation = nil
		a.stopConfirmationTimerLocked()
	}
	if releaseFence != nil {
		defer releaseFence()
	}
	if a.buildWorkspace == nil {
		return WorkspaceChangeResult{}, errors.New("workspace builder is unavailable")
	}
	candidate, err := a.buildWorkspace(path, 0)
	if err != nil {
		return WorkspaceChangeResult{}, err
	}
	if candidate == nil {
		return WorkspaceChangeResult{}, errors.New("workspace builder returned nil context")
	}

	a.workspaceMu.Lock()
	old := a.workspace
	a.lastGeneration++
	candidate.setGeneration(a.lastGeneration)
	a.bindWorkspaceRuntimeNotifications(candidate)
	if err := candidate.activate(); err != nil {
		a.lastGeneration--
		a.workspaceMu.Unlock()
		closeCtx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		_ = candidate.Close(closeCtx)
		cancel()
		return WorkspaceChangeResult{}, fmt.Errorf("activate project workspace: %w", err)
	}
	a.workspace = candidate
	a.workspaceStatus = WorkspaceOpen
	a.workspaceError = ""
	a.syncLegacyWorkspaceFieldsLocked(candidate)
	state := a.workspaceStateLocked()
	a.workspaceMu.Unlock()
	a.startWorkspaceWatcher(candidate)

	if old != nil {
		closeCtx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		closeErr := old.Close(closeCtx)
		cancel()
		if closeErr != nil {
			return WorkspaceChangeResult{
				State:   state,
				Warning: fmt.Sprintf("previous project cleanup incomplete: %v", closeErr),
			}, nil
		}
	}
	return WorkspaceChangeResult{State: state}, nil
}

func (a *App) CloseProject(confirmationToken string) (WorkspaceChangeResult, error) {
	a.transitionMu.Lock()
	defer a.transitionMu.Unlock()
	current, err := a.currentWorkspace(0)
	if err != nil {
		return WorkspaceChangeResult{State: a.GetWorkspaceState()}, nil
	}

	var releaseFence func()
	if confirmationToken == "" {
		a.clearConfirmationLocked()
		releaseFence = current.fenceResourceAdmission()
		summary := current.activeResourceSummary()
		if summary.Terminals > 0 ||
			summary.AgentRuns > 0 ||
			summary.PendingAdmissions > 0 {
			token, tokenErr := randomWorkspaceToken()
			if tokenErr != nil {
				releaseFence()
				return WorkspaceChangeResult{}, tokenErr
			}
			a.confirmation = &workspaceConfirmation{
				token:      token,
				action:     "close",
				generation: current.Generation(),
				revision:   summary.ResourceRevision,
				expiresAt:  time.Now().Add(a.workspaceConfirmationTTL()),
				release:    releaseFence,
			}
			a.scheduleConfirmationExpiryLocked(token)
			return WorkspaceChangeResult{
				State:                a.GetWorkspaceState(),
				RequiresConfirmation: true,
				ConfirmationToken:    token,
				ActiveResources:      summary,
			}, nil
		}
	} else {
		confirmation := a.confirmation
		if confirmation == nil || confirmation.token != confirmationToken ||
			confirmation.action != "close" ||
			time.Now().After(confirmation.expiresAt) ||
			confirmation.generation != current.Generation() ||
			confirmation.revision != current.resourceRevisionValue() {
			a.clearConfirmationLocked()
			return WorkspaceChangeResult{}, errInvalidConfirmation
		}
		releaseFence = confirmation.release
		a.confirmation = nil
		a.stopConfirmationTimerLocked()
	}
	if releaseFence != nil {
		defer releaseFence()
	}

	a.workspaceMu.Lock()
	a.workspace = nil
	a.workspaceStatus = WorkspaceClosed
	a.workspaceError = ""
	a.syncLegacyWorkspaceFieldsLocked(nil)
	closedState := a.workspaceStateLocked()
	a.workspaceMu.Unlock()
	a.stopWorkspaceWatcher()
	closeCtx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	closeErr := current.Close(closeCtx)
	cancel()
	a.workspaceMu.Lock()
	a.workspaceStatus = WorkspaceWelcome
	a.workspaceMu.Unlock()
	result := WorkspaceChangeResult{State: closedState}
	if closeErr != nil {
		result.Warning = fmt.Sprintf("project cleanup incomplete: %v", closeErr)
	}
	return result, nil
}

func (a *App) CancelWorkspaceChange(token string) error {
	a.transitionMu.Lock()
	defer a.transitionMu.Unlock()
	if a.confirmation == nil || a.confirmation.token != token {
		return errInvalidConfirmation
	}
	a.clearConfirmationLocked()
	return nil
}

func (a *App) clearConfirmationLocked() {
	if a.confirmation != nil {
		a.confirmation.release()
		a.confirmation = nil
	}
	a.stopConfirmationTimerLocked()
}

func (a *App) workspaceConfirmationTTL() time.Duration {
	if a.confirmationTTL <= 0 {
		return time.Minute
	}
	return a.confirmationTTL
}

func (a *App) scheduleConfirmationExpiryLocked(token string) {
	a.stopConfirmationTimerLocked()
	if a.confirmation == nil || a.confirmation.token != token {
		return
	}
	delay := time.Until(a.confirmation.expiresAt)
	if delay < 0 {
		delay = 0
	}
	a.confirmationTimer = time.AfterFunc(delay, func() {
		a.transitionMu.Lock()
		defer a.transitionMu.Unlock()
		if a.confirmation != nil && a.confirmation.token == token &&
			!time.Now().Before(a.confirmation.expiresAt) {
			a.clearConfirmationLocked()
		}
	})
}

func (a *App) stopConfirmationTimerLocked() {
	if a.confirmationTimer != nil {
		a.confirmationTimer.Stop()
		a.confirmationTimer = nil
	}
}

func (a *App) syncLegacyWorkspaceFieldsLocked(workspace *WorkspaceContext) {
	if workspace == nil {
		a.dbPath = ""
		a.initialPlan = 0
		a.projectName = ""
		a.projectRoot = ""
		a.terminals = nil
		return
	}
	a.dbPath = workspace.dbPath
	a.initialPlan = workspace.initialPlan
	a.projectName = workspace.name
	a.projectRoot = workspace.root
	a.terminals = workspace.terminalManager()
}

func randomWorkspaceToken() (string, error) {
	value := make([]byte, 32)
	if _, err := rand.Read(value); err != nil {
		return "", fmt.Errorf("create workspace confirmation token: %w", err)
	}
	return base64.RawURLEncoding.EncodeToString(value), nil
}
