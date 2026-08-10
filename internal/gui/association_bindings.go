package gui

import (
	"errors"
	"fmt"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/store"
)

// TerminalAssociationMutationV2 is the authority-free result of one
// generation- and revision-fenced live metadata change. A detached result
// intentionally omits Pointer even though the backend retains a project-only
// target to preserve monotonic revision and linked-launch provenance.
type TerminalAssociationMutationV2 struct {
	Generation uint64                 `json:"generation"`
	SessionID  string                 `json:"sessionId"`
	Revision   uint64                 `json:"revision"`
	Detached   bool                   `json:"detached"`
	Pointer    *association.PointerV1 `json:"pointer,omitempty"`
}

type storeAssociationCatalog struct {
	store *store.Store
}

var errAssociationProjectMismatch = errors.New(
	"workspace project store does not match association root",
)

// workspaceAssociationHost is the only GUI constructor for association
// authority backed by project storage. It proves that the opened database and
// the live workspace describe the same canonical project before the catalog
// can validate any plan or task.
func workspaceAssociationHost(
	workspace *WorkspaceContext,
	s *store.Store,
) (*association.Host, error) {
	if workspace == nil || s == nil {
		return nil, errors.New("workspace and project store are required")
	}
	identityHost, err := association.NewHost(
		workspace.root,
		workspace.Generation(),
		nil,
	)
	if err != nil {
		return nil, err
	}
	storeRoot, err := s.ProjectRoot()
	if err != nil {
		return nil, err
	}
	if storeRoot != identityHost.ProjectRoot() {
		return nil, fmt.Errorf(
			"%w: workspace %q, store %q",
			errAssociationProjectMismatch,
			identityHost.ProjectRoot(),
			storeRoot,
		)
	}
	return association.NewHost(
		identityHost.ProjectRoot(),
		workspace.Generation(),
		storeAssociationCatalog{store: s},
	)
}

func (c storeAssociationCatalog) ValidatePlan(planID uint64) error {
	_, err := c.store.GetPlan(planID)
	return err
}

func (c storeAssociationCatalog) TaskPlan(taskID uint64) (uint64, error) {
	task, err := c.store.GetTask(taskID)
	if err != nil {
		return 0, err
	}
	return task.PlanID, nil
}

// AssociateTerminalV2 resolves an authority-free tab pointer against the
// current project and attaches the host-owned result to one live session.
func (a *App) AssociateTerminalV2(
	generation uint64,
	sessionID string,
	pointer association.PointerV1,
) (association.AssociationV1, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return association.AssociationV1{}, err
	}
	defer release()
	defer s.Close()
	manager, ok := workspace.terminalManager().(terminalAssociationManager)
	if !ok {
		return association.AssociationV1{}, errors.New("terminal association manager is unavailable")
	}
	host, err := workspaceAssociationHost(workspace, s)
	if err != nil {
		return association.AssociationV1{}, err
	}
	workspace.associationMu.Lock()
	if registry := workspace.agentRegistry(); registry != nil &&
		registry.HasLinkedTerminal(sessionID) {
		workspace.associationMu.Unlock()
		return association.AssociationV1{}, errors.New(
			"linked terminal association requires a revision-fenced mutation",
		)
	}
	result, err := manager.Associate(sessionID, host, pointer)
	workspace.associationMu.Unlock()
	if err != nil {
		return association.AssociationV1{}, err
	}
	workspace.bumpResourceRevision()
	a.publishWorkspaceRuntimeChanged(workspace)
	return result, nil
}

// AssociateAgentRunV2 validates and attaches context to a run after it has
// registered. Registration itself is always detached, including for external
// agents, so only this host binding can create a validated association.
func (a *App) AssociateAgentRunV2(
	generation uint64,
	runID string,
	pointer association.PointerV1,
) (association.AssociationV1, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return association.AssociationV1{}, err
	}
	defer release()
	defer s.Close()
	registry := workspace.agentRegistry()
	if registry == nil {
		return association.AssociationV1{}, errors.New("AgentRun registry is unavailable")
	}
	host, err := workspaceAssociationHost(workspace, s)
	if err != nil {
		return association.AssociationV1{}, err
	}
	workspace.associationMu.Lock()
	if registry.IsLinkedLaunchRun(runID) {
		workspace.associationMu.Unlock()
		return association.AssociationV1{}, errors.New(
			"linked AgentRun association requires a terminal revision-fenced mutation",
		)
	}
	result, err := registry.Associate(runID, host, pointer)
	workspace.associationMu.Unlock()
	if err != nil {
		return association.AssociationV1{}, err
	}
	workspace.bumpResourceRevision()
	a.publishWorkspaceRuntimeChanged(workspace)
	return result, nil
}

// MutateTerminalAssociationV2 relinks or detaches the exact live terminal
// session selected by the host. A corresponding host-launched AgentRun is
// derived from the terminal identity and changed within the same visible
// transaction; callers never supply a run identity.
func (a *App) MutateTerminalAssociationV2(
	generation uint64,
	sessionID string,
	expectedRevision uint64,
	detach bool,
	pointer association.PointerV1,
) (TerminalAssociationMutationV2, error) {
	workspace, manager, release, err := a.beginTerminalOperation(generation, false)
	if err != nil {
		return TerminalAssociationMutationV2{}, err
	}
	defer release()
	casManager, ok := manager.(terminalAssociationCASManager)
	if !ok {
		return TerminalAssociationMutationV2{}, errors.New(
			"terminal association mutation is unavailable",
		)
	}
	s, err := store.Open(workspace.dbPath)
	if err != nil {
		return TerminalAssociationMutationV2{}, err
	}
	defer s.Close()
	host, err := workspaceAssociationHost(workspace, s)
	if err != nil {
		return TerminalAssociationMutationV2{}, err
	}
	if detach {
		pointer = association.PointerV1{Version: association.VersionV1}
	} else if pointer.PlanID == 0 {
		return TerminalAssociationMutationV2{}, fmt.Errorf(
			"%w: relink requires a plan or task",
			association.ErrInvalidTarget,
		)
	}

	result, err := func() (TerminalAssociationMutationV2, error) {
		workspace.associationMu.Lock()
		defer workspace.associationMu.Unlock()
		terminalChange, prepareErr := casManager.PrepareAssociationChange(
			sessionID,
			host,
			pointer,
			expectedRevision,
		)
		if prepareErr != nil {
			return TerminalAssociationMutationV2{}, prepareErr
		}
		if detach && (terminalChange.Previous == nil ||
			terminalChange.Previous.Target.PlanID == 0) {
			return TerminalAssociationMutationV2{}, fmt.Errorf(
				"%w: terminal is already detached",
				association.ErrInvalidTarget,
			)
		}

		registry := workspace.agentRegistry()
		var runChange agentrun.LinkedAssociationChange
		hasLinkedRun := false
		if registry != nil {
			runChange, hasLinkedRun, prepareErr =
				registry.PrepareLinkedTerminalAssociationChange(
					sessionID,
					terminalChange.Previous,
					terminalChange.Next,
					host,
					pointer,
				)
			if prepareErr != nil {
				return TerminalAssociationMutationV2{}, prepareErr
			}
		}
		if commitErr := casManager.CommitAssociationChange(terminalChange); commitErr != nil {
			return TerminalAssociationMutationV2{}, commitErr
		}
		if hasLinkedRun {
			if commitErr := registry.CommitLinkedAssociationChange(runChange); commitErr != nil {
				rollbackErr := casManager.RollbackAssociationChange(terminalChange)
				return TerminalAssociationMutationV2{}, errors.Join(commitErr, rollbackErr)
			}
		}

		mutation := TerminalAssociationMutationV2{
			Generation: workspace.Generation(),
			SessionID:  sessionID,
			Revision:   terminalChange.Next.Revision,
			Detached:   detach,
		}
		if !detach {
			validated := association.PointerV1{
				Version: association.VersionV1,
				PlanID:  terminalChange.Next.Target.PlanID,
				TaskID:  terminalChange.Next.Target.TaskID,
			}
			mutation.Pointer = &validated
		}
		return mutation, nil
	}()
	if err != nil {
		return TerminalAssociationMutationV2{}, err
	}
	workspace.bumpResourceRevision()
	a.publishWorkspaceRuntimeChanged(workspace)
	return result, nil
}

var _ terminalAssociationCASManager = productionTerminalManager{}
var _ workspaceAgentRegistry = (*workspaceAgentResources)(nil)
