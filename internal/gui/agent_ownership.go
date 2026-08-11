package gui

import (
	"errors"
	"sort"

	"github.com/ro-ag/ptrack/internal/agentrun"
)

const (
	agentActivityConflictLimit = 64
	agentConflictRunIDLimit    = 16
)

var (
	ErrAgentOwnershipRequiresTask = errors.New("agent ownership requires a current task association")
	ErrAgentOwnershipInactive     = errors.New("agent ownership requires an active run")
	ErrAgentOwnershipRevision     = errors.New("agent ownership association revision changed")
)

// agentTaskOwnershipClaim is workspace-generation state, not an association
// and not authority. LifecycleRevision prevents a stale heartbeat or process
// restart from silently reviving an earlier claim.
type agentTaskOwnershipClaim struct {
	Generation          uint64
	RunID               string
	PlanID              uint64
	TaskID              uint64
	AssociationRevision uint64
	LifecycleRevision   uint64
}

type AgentTaskOwnership struct {
	PlanID              uint64 `json:"planId"`
	TaskID              uint64 `json:"taskId"`
	AssociationRevision uint64 `json:"associationRevision"`
}

type AgentOwnershipMutationV2 struct {
	Generation uint64              `json:"generation"`
	RunID      string              `json:"runId"`
	Owned      bool                `json:"owned"`
	Ownership  *AgentTaskOwnership `json:"ownership,omitempty"`
}

// SetAgentTaskOwnershipV2 records or releases descriptive ownership. It does
// not alter an association, task, process, capability, or status.
func (a *App) SetAgentTaskOwnershipV2(
	generation uint64,
	runID string,
	expectedAssociationRevision uint64,
	owned bool,
) (AgentOwnershipMutationV2, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return AgentOwnershipMutationV2{}, err
	}
	defer release()
	defer s.Close()
	result := AgentOwnershipMutationV2{
		Generation: workspace.Generation(),
		RunID:      runID,
		Owned:      owned,
	}
	if runID == "" {
		return AgentOwnershipMutationV2{}, agentrun.ErrRunNotFound
	}

	workspace.associationMu.Lock()
	defer workspace.associationMu.Unlock()
	if expectedAssociationRevision == 0 {
		return AgentOwnershipMutationV2{}, ErrAgentOwnershipRevision
	}
	registry := workspace.agentRegistry()
	if registry == nil {
		return AgentOwnershipMutationV2{}, errors.New("AgentRun registry is unavailable")
	}
	host, err := workspaceAssociationHost(workspace, s)
	if err != nil {
		return AgentOwnershipMutationV2{}, err
	}
	var claim agentTaskOwnershipClaim
	err = registry.WithExactRuntimeSnapshot(
		linkedRuntimeCandidateLimit,
		func(runs []agentrun.Run) error {
			for _, run := range runs {
				if run.ID != runID {
					continue
				}
				if !agentRunIsLive(run) {
					return ErrAgentOwnershipInactive
				}
				current := currentRuntimeAssociation(host, run.ID, run.Association)
				if current == nil || current.TaskID == 0 {
					return ErrAgentOwnershipRequiresTask
				}
				if current.Revision != expectedAssociationRevision {
					return ErrAgentOwnershipRevision
				}
				claim = agentTaskOwnershipClaim{
					Generation: workspace.Generation(), RunID: run.ID,
					PlanID: current.PlanID, TaskID: current.TaskID,
					AssociationRevision: current.Revision,
					LifecycleRevision:   run.LifecycleRevision,
				}
				return nil
			}
			return agentrun.ErrRunNotFound
		},
	)
	if err != nil {
		return AgentOwnershipMutationV2{}, err
	}
	if !owned {
		workspace.ownershipMu.Lock()
		existing, exists := workspace.agentOwnership[runID]
		if exists && existing != claim {
			workspace.ownershipMu.Unlock()
			return AgentOwnershipMutationV2{}, ErrAgentOwnershipRevision
		}
		if exists {
			delete(workspace.agentOwnership, runID)
		}
		workspace.ownershipMu.Unlock()
		if exists {
			workspace.bumpResourceRevision()
			a.publishWorkspaceRuntimeChanged(workspace)
		}
		return result, nil
	}
	workspace.ownershipMu.Lock()
	workspace.agentOwnership[runID] = claim
	workspace.ownershipMu.Unlock()
	result.Ownership = projectAgentTaskOwnership(claim)
	workspace.bumpResourceRevision()
	a.publishWorkspaceRuntimeChanged(workspace)
	return result, nil
}

func projectAgentTaskOwnership(claim agentTaskOwnershipClaim) *AgentTaskOwnership {
	return &AgentTaskOwnership{
		PlanID: claim.PlanID, TaskID: claim.TaskID,
		AssociationRevision: claim.AssociationRevision,
	}
}

func applyAgentOwnership(
	workspace *WorkspaceContext,
	activity *AgentActivitySnapshot,
	projection runtimeProjection,
) {
	if workspace == nil || activity == nil {
		return
	}
	workspace.ownershipMu.Lock()
	claims := make(map[string]agentTaskOwnershipClaim, len(workspace.agentOwnership))
	for runID, claim := range workspace.agentOwnership {
		claims[runID] = claim
	}
	workspace.ownershipMu.Unlock()

	activity.AnalysisIncomplete = projection.agentAnalysisIncomplete
	itemsByRun := make(map[string]*AgentActivity, len(activity.Items))
	for index := range activity.Items {
		itemsByRun[activity.Items[index].RunID] = &activity.Items[index]
	}
	validClaims := make(map[string]agentTaskOwnershipClaim, len(claims))
	for runID, claim := range claims {
		item := itemsByRun[runID]
		exact, exactExists := projection.exactAgentRuns[runID]
		if item == nil || !exactExists || !ownershipClaimIsCurrent(
			workspace.Generation(), claim, exact, item.Association,
		) {
			continue
		}
		item.Ownership = projectAgentTaskOwnership(claim)
		validClaims[runID] = claim
	}
	activity.Conflicts, activity.ConflictBounds = agentActivityConflicts(
		projection.agentCandidates,
		validClaims,
	)
	activity.AnalysisIncomplete = activity.AnalysisIncomplete ||
		activity.ConflictBounds.More > 0
}

func ownershipClaimIsCurrent(
	generation uint64,
	claim agentTaskOwnershipClaim,
	run agentrun.Run,
	current *RuntimeAssociation,
) bool {
	return claim.Generation == generation && claim.RunID == run.ID &&
		claim.LifecycleRevision != 0 && claim.LifecycleRevision == run.LifecycleRevision &&
		agentRunIsLive(run) && current != nil && current.TaskID != 0 &&
		claim.PlanID == current.PlanID && claim.TaskID == current.TaskID &&
		claim.AssociationRevision == current.Revision
}

type agentConflictTarget struct {
	planID uint64
	taskID uint64
}

func agentActivityConflicts(
	runs []AgentRuntimeSummary,
	ownership map[string]agentTaskOwnershipClaim,
) ([]AgentActivityConflict, BoundedSnapshot) {
	grouped := make(map[agentConflictTarget][]string)
	seenRuns := make(map[string]bool, len(runs))
	for _, run := range runs {
		if seenRuns[run.RunID] || !run.Live || run.Association == nil ||
			run.Association.TaskID == 0 {
			continue
		}
		seenRuns[run.RunID] = true
		target := agentConflictTarget{
			planID: run.Association.PlanID,
			taskID: run.Association.TaskID,
		}
		grouped[target] = append(grouped[target], run.RunID)
	}
	targets := make([]agentConflictTarget, 0, len(grouped))
	for target, runIDs := range grouped {
		if len(runIDs) >= 2 {
			targets = append(targets, target)
		}
	}
	sort.Slice(targets, func(i, j int) bool {
		if targets[i].planID != targets[j].planID {
			return targets[i].planID < targets[j].planID
		}
		return targets[i].taskID < targets[j].taskID
	})
	total := len(targets)
	if len(targets) > agentActivityConflictLimit {
		targets = targets[:agentActivityConflictLimit]
	}
	conflicts := make([]AgentActivityConflict, 0, len(targets))
	for _, target := range targets {
		runIDs := grouped[target]
		sort.Strings(runIDs)
		owners := 0
		for _, runID := range runIDs {
			if _, exists := ownership[runID]; exists {
				owners++
			}
		}
		shown := min(len(runIDs), agentConflictRunIDLimit)
		conflicts = append(conflicts, AgentActivityConflict{
			PlanID: target.planID, TaskID: target.taskID,
			AgentCount: len(runIDs), OwnerCount: owners,
			RunIDs: append([]string{}, runIDs[:shown]...),
			Bounds: snapshotBound(shown, len(runIDs)),
		})
	}
	return conflicts, snapshotBound(len(conflicts), total)
}
