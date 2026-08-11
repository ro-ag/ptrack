package gui

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"strings"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/gitinfo"
)

var (
	ErrAgentWorktreeInactive = errors.New("worktree association requires an active run")
	ErrAgentWorktreeRevision = errors.New("agent association revision changed")
	ErrAgentWorktreeCWD      = errors.New("agent working directory is outside the selected worktree")
)

type gitWorktreeInspector interface {
	InspectWorktree(context.Context, string, string) (gitinfo.WorktreeIdentity, error)
}

// agentWorktreeClaim is ephemeral presentation metadata. It is deliberately
// separate from AgentRun association and from every capability decision.
type agentWorktreeClaim struct {
	Generation          uint64
	RunID               string
	LifecycleRevision   uint64
	AssociationRevision uint64
	PlanID              uint64
	TaskID              uint64
	Identity            gitinfo.WorktreeIdentity
	Isolated            bool
}

type AgentWorktreeIdentity struct {
	Root   string `json:"root"`
	Branch string `json:"branch,omitempty"`
	Head   string `json:"head"`
	Linked bool   `json:"linked"`
}

type AgentWorktreeAssociation struct {
	Identity   AgentWorktreeIdentity `json:"identity"`
	Verified   bool                  `json:"verified"`
	Isolated   bool                  `json:"isolated"`
	CWDMatches bool                  `json:"cwdMatches"`
}

type AgentWorktreeMutationV2 struct {
	Generation uint64                    `json:"generation"`
	RunID      string                    `json:"runId"`
	Associated bool                      `json:"associated"`
	Worktree   *AgentWorktreeAssociation `json:"worktree,omitempty"`
}

// SetAgentWorktreeV2 associates existing host-observed metadata with one live
// run. It never changes CWD, launches a process, runs a mutating Git command,
// or grants filesystem/network/capability authority.
func (a *App) SetAgentWorktreeV2(
	generation uint64,
	runID string,
	expectedAssociationRevision uint64,
	root string,
	associated bool,
) (AgentWorktreeMutationV2, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return AgentWorktreeMutationV2{}, err
	}
	defer release()
	defer s.Close()
	result := AgentWorktreeMutationV2{
		Generation: workspace.Generation(), RunID: runID, Associated: associated,
	}
	if runID == "" {
		return AgentWorktreeMutationV2{}, agentrun.ErrRunNotFound
	}
	workspace.associationMu.Lock()
	defer workspace.associationMu.Unlock()
	registry := workspace.agentRegistry()
	if registry == nil {
		return AgentWorktreeMutationV2{}, errors.New("AgentRun registry is unavailable")
	}
	host, err := workspaceAssociationHost(workspace, s)
	if err != nil {
		return AgentWorktreeMutationV2{}, err
	}
	before, beforeAssociation, err := exactWorktreeRun(
		registry, host, runID, expectedAssociationRevision,
	)
	if err != nil {
		return AgentWorktreeMutationV2{}, err
	}
	if !associated {
		workspace.worktreeMu.Lock()
		existing, exists := workspace.agentWorktrees[runID]
		if exists && !worktreeClaimIsCurrent(
			workspace.Generation(), existing, before, beforeAssociation,
		) {
			workspace.worktreeMu.Unlock()
			return AgentWorktreeMutationV2{}, ErrAgentWorktreeRevision
		}
		if exists {
			delete(workspace.agentWorktrees, runID)
		}
		workspace.worktreeMu.Unlock()
		if exists {
			workspace.bumpResourceRevision()
			a.publishWorkspaceRuntimeChanged(workspace)
		}
		return result, nil
	}
	if root == "" || root != strings.TrimSpace(root) || len(root) > 4096 {
		return AgentWorktreeMutationV2{}, errors.New("an existing worktree root is required")
	}
	inspector := a.gitWorktrees
	if inspector == nil {
		inspector = gitinfo.Service{}
	}
	identity, err := inspector.InspectWorktree(workspace.Context(), workspace.root, root)
	if err != nil {
		return AgentWorktreeMutationV2{}, err
	}
	cwd, err := filepath.EvalSymlinks(before.CWD)
	if err != nil || !pathInside(identity.Root, cwd) {
		return AgentWorktreeMutationV2{}, ErrAgentWorktreeCWD
	}
	after, afterAssociation, err := exactWorktreeRun(
		registry, host, runID, expectedAssociationRevision,
	)
	if err != nil {
		return AgentWorktreeMutationV2{}, err
	}
	if before.LifecycleRevision != after.LifecycleRevision ||
		filepath.Clean(before.CWD) != filepath.Clean(after.CWD) ||
		!sameRuntimeAssociation(beforeAssociation, afterAssociation) {
		return AgentWorktreeMutationV2{}, errors.New("agent changed while worktree identity was inspected")
	}
	projectRoot, err := filepath.EvalSymlinks(workspace.root)
	if err != nil {
		return AgentWorktreeMutationV2{}, fmt.Errorf("canonicalize project root: %w", err)
	}
	claim := agentWorktreeClaim{
		Generation: workspace.Generation(), RunID: runID,
		LifecycleRevision: after.LifecycleRevision, Identity: identity,
		Isolated: filepath.Clean(identity.Root) != filepath.Clean(projectRoot),
	}
	if afterAssociation != nil {
		claim.AssociationRevision = afterAssociation.Revision
		claim.PlanID = afterAssociation.PlanID
		claim.TaskID = afterAssociation.TaskID
	}
	workspace.worktreeMu.Lock()
	workspace.agentWorktrees[runID] = claim
	workspace.worktreeMu.Unlock()
	result.Worktree = projectAgentWorktree(claim)
	workspace.bumpResourceRevision()
	a.publishWorkspaceRuntimeChanged(workspace)
	return result, nil
}

func exactWorktreeRun(
	registry interface {
		WithExactRuntimeSnapshot(int, func([]agentrun.Run) error) error
	},
	host *association.Host,
	runID string,
	expectedRevision uint64,
) (agentrun.Run, *RuntimeAssociation, error) {
	var selected agentrun.Run
	var current *RuntimeAssociation
	err := registry.WithExactRuntimeSnapshot(
		linkedRuntimeCandidateLimit,
		func(runs []agentrun.Run) error {
			for _, run := range runs {
				if run.ID != runID {
					continue
				}
				if !agentRunIsLive(run) {
					return ErrAgentWorktreeInactive
				}
				association := currentRuntimeAssociation(host, run.ID, run.Association)
				actualRevision := uint64(0)
				if association != nil {
					actualRevision = association.Revision
				}
				if actualRevision != expectedRevision {
					return ErrAgentWorktreeRevision
				}
				selected = run
				current = cloneRuntimeAssociation(association)
				return nil
			}
			return agentrun.ErrRunNotFound
		},
	)
	return selected, current, err
}

func pathInside(root, candidate string) bool {
	relative, err := filepath.Rel(filepath.Clean(root), filepath.Clean(candidate))
	return err == nil && relative != ".." &&
		!strings.HasPrefix(relative, ".."+string(filepath.Separator))
}

func sameRuntimeAssociation(left, right *RuntimeAssociation) bool {
	if left == nil || right == nil {
		return left == nil && right == nil
	}
	return *left == *right
}

func projectAgentWorktree(claim agentWorktreeClaim) *AgentWorktreeAssociation {
	return &AgentWorktreeAssociation{
		Identity: AgentWorktreeIdentity{
			Root: claim.Identity.Root, Branch: claim.Identity.Branch,
			Head: claim.Identity.Head, Linked: claim.Identity.Linked,
		},
		Verified: true, Isolated: claim.Isolated,
		CWDMatches: true,
	}
}

func applyAgentWorktrees(
	workspace *WorkspaceContext,
	activity *AgentActivitySnapshot,
	projection runtimeProjection,
) {
	if workspace == nil || activity == nil {
		return
	}
	items := make(map[string]*AgentActivity, len(activity.Items))
	for index := range activity.Items {
		items[activity.Items[index].RunID] = &activity.Items[index]
	}
	workspace.worktreeMu.Lock()
	defer workspace.worktreeMu.Unlock()
	for runID, claim := range workspace.agentWorktrees {
		run, exists := projection.exactAgentRuns[runID]
		item := items[runID]
		if !exists || item == nil || !worktreeClaimIsCurrent(
			workspace.Generation(), claim, run, item.Association,
		) {
			delete(workspace.agentWorktrees, runID)
			continue
		}
		item.Worktree = projectAgentWorktree(claim)
	}
}

func worktreeClaimIsCurrent(
	generation uint64,
	claim agentWorktreeClaim,
	run agentrun.Run,
	current *RuntimeAssociation,
) bool {
	if claim.Generation != generation || claim.RunID != run.ID ||
		claim.LifecycleRevision == 0 || claim.LifecycleRevision != run.LifecycleRevision ||
		!agentRunIsLive(run) {
		return false
	}
	if current == nil {
		return claim.AssociationRevision == 0 && claim.PlanID == 0 && claim.TaskID == 0
	}
	return claim.AssociationRevision == current.Revision &&
		claim.PlanID == current.PlanID && claim.TaskID == current.TaskID
}
