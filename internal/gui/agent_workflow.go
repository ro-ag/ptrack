package gui

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"path/filepath"
	"sort"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/gitinfo"
	"github.com/ro-ag/ptrack/internal/store"
)

type AgentWorkflowStatus struct {
	Staged     int `json:"staged"`
	Unstaged   int `json:"unstaged"`
	Untracked  int `json:"untracked"`
	Conflicted int `json:"conflicted"`
	Ahead      int `json:"ahead"`
	Behind     int `json:"behind"`
}

type AgentWorkflowProposalV2 struct {
	ID           string              `json:"id"`
	Generation   uint64              `json:"generation"`
	Kind         AgentWorkflowKind   `json:"kind"`
	State        AgentWorkflowState  `json:"state"`
	RunID        string              `json:"runId"`
	Association  *RuntimeAssociation `json:"association,omitempty"`
	WorktreeRoot string              `json:"worktreeRoot,omitempty"`
	Isolated     bool                `json:"isolated"`
	Branch       string              `json:"branch"`
	Head         string              `json:"head"`
	TargetBranch string              `json:"targetBranch,omitempty"`
	TargetHead   string              `json:"targetHead,omitempty"`
	Status       AgentWorkflowStatus `json:"status"`
	CreatedAt    string              `json:"createdAt"`
	ExpiresAt    string              `json:"expiresAt"`
	ApprovedAt   string              `json:"approvedAt,omitempty"`
	Notice       string              `json:"notice"`
}

type AgentWorkflowInbox struct {
	Items      []AgentWorkflowProposalV2 `json:"items"`
	Bounds     BoundedSnapshot           `json:"bounds"`
	Incomplete bool                      `json:"incomplete"`
	Notice     string                    `json:"notice"`
}

type AgentWorkflowDismissalV2 struct {
	Generation uint64 `json:"generation"`
	ID         string `json:"id"`
	Removed    bool   `json:"removed"`
}

const workflowNoExecutionNotice = "Proposal and approval only; no command runs and no Git, hosting, task, or capability state changes."

func (a *App) PrepareAgentWorkflowV2(
	generation uint64,
	runID string,
	expectedAssociationRevision uint64,
	kind AgentWorkflowKind,
	targetBranch string,
) (AgentWorkflowProposalV2, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return AgentWorkflowProposalV2{}, err
	}
	defer release()
	defer s.Close()
	if !validAgentWorkflowKind(kind) {
		return AgentWorkflowProposalV2{}, ErrAgentWorkflowKind
	}
	workspace.associationMu.Lock()
	defer workspace.associationMu.Unlock()
	run, current, worktree, binding, err := a.captureAgentWorkflowBinding(
		s, workspace, runID, expectedAssociationRevision, kind, targetBranch,
	)
	if err != nil {
		return AgentWorkflowProposalV2{}, err
	}
	id, err := randomWorkspaceToken()
	if err != nil {
		return AgentWorkflowProposalV2{}, err
	}
	now := workspace.workflows.now().UTC()
	proposal := agentWorkflowProposal{
		ID: id, Generation: workspace.Generation(), Kind: kind,
		State: AgentWorkflowProposed, RunID: run.ID,
		LifecycleRevision: run.LifecycleRevision,
		Association:       cloneRuntimeAssociation(current), Worktree: worktree,
		Git: binding, CreatedAt: now, ExpiresAt: now.Add(agentWorkflowTTL),
	}
	if err := workspace.workflows.add(proposal); err != nil {
		return AgentWorkflowProposalV2{}, err
	}
	workspace.bumpResourceRevision()
	a.publishWorkspaceRuntimeChanged(workspace)
	return projectAgentWorkflow(proposal, workspace.root), nil
}

func (a *App) ApproveAgentWorkflowV2(
	generation uint64,
	id string,
) (AgentWorkflowProposalV2, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return AgentWorkflowProposalV2{}, err
	}
	defer release()
	defer s.Close()
	proposal, exists := workspace.workflows.get(id)
	if !exists || proposal.Generation != workspace.Generation() {
		return AgentWorkflowProposalV2{}, ErrAgentWorkflowStale
	}
	if proposal.State == AgentWorkflowApproved {
		return AgentWorkflowProposalV2{}, ErrAgentWorkflowApproved
	}
	workspace.associationMu.Lock()
	defer workspace.associationMu.Unlock()
	run, current, worktree, binding, err := a.captureAgentWorkflowBinding(
		s, workspace, proposal.RunID, runtimeAssociationRevision(proposal.Association),
		proposal.Kind, proposal.Git.TargetBranch,
	)
	if err != nil || run.LifecycleRevision != proposal.LifecycleRevision ||
		!runtimeAssociationsEqualOrNil(current, proposal.Association) ||
		!worktreeIdentitiesEqual(worktree, proposal.Worktree) || binding != proposal.Git {
		workspace.workflows.remove(id)
		return AgentWorkflowProposalV2{}, ErrAgentWorkflowStale
	}
	approved, err := workspace.workflows.approve(id, workspace.workflows.now().UTC())
	if err != nil {
		return AgentWorkflowProposalV2{}, err
	}
	workspace.bumpResourceRevision()
	a.publishWorkspaceRuntimeChanged(workspace)
	return projectAgentWorkflow(approved, workspace.root), nil
}

func (a *App) DismissAgentWorkflowV2(
	generation uint64,
	id string,
) (AgentWorkflowDismissalV2, error) {
	workspace, err := a.currentWorkspace(generation)
	if err != nil {
		return AgentWorkflowDismissalV2{}, err
	}
	release, err := workspace.beginOperation(generation, false)
	if err != nil {
		return AgentWorkflowDismissalV2{}, err
	}
	defer release()
	removed := workspace.workflows.remove(id)
	if !removed {
		return AgentWorkflowDismissalV2{}, ErrAgentWorkflowStale
	}
	workspace.bumpResourceRevision()
	a.publishWorkspaceRuntimeChanged(workspace)
	return AgentWorkflowDismissalV2{
		Generation: workspace.Generation(), ID: id, Removed: true,
	}, nil
}

func (a *App) captureAgentWorkflowBinding(
	s *store.Store,
	workspace *WorkspaceContext,
	runID string,
	expectedAssociationRevision uint64,
	kind AgentWorkflowKind,
	targetBranch string,
) (agentrun.Run, *RuntimeAssociation, *gitinfo.WorktreeIdentity, agentWorkflowGitBinding, error) {
	registry := workspace.agentRegistry()
	if registry == nil {
		return agentrun.Run{}, nil, nil, agentWorkflowGitBinding{}, errors.New("AgentRun registry is unavailable")
	}
	host, err := workspaceAssociationHost(workspace, s)
	if err != nil {
		return agentrun.Run{}, nil, nil, agentWorkflowGitBinding{}, err
	}
	before, current, err := exactWorkflowRun(registry, host, runID, expectedAssociationRevision)
	if err != nil {
		return agentrun.Run{}, nil, nil, agentWorkflowGitBinding{}, err
	}
	worktree, err := currentWorkflowWorktree(workspace, before, current)
	if err != nil {
		return agentrun.Run{}, nil, nil, agentWorkflowGitBinding{}, err
	}
	root := workspace.root
	if worktree != nil {
		inspector := a.gitWorktrees
		if inspector == nil {
			inspector = gitinfo.Service{}
		}
		observed, inspectErr := inspector.InspectWorktree(workspace.Context(), workspace.root, worktree.Root)
		if inspectErr != nil || !sameWorktreeRepositoryIdentity(&observed, worktree) {
			return agentrun.Run{}, nil, nil, agentWorkflowGitBinding{}, ErrAgentWorkflowStale
		}
		cwd, cwdErr := filepath.EvalSymlinks(before.CWD)
		if cwdErr != nil || !pathInside(observed.Root, cwd) {
			return agentrun.Run{}, nil, nil, agentWorkflowGitBinding{}, ErrAgentWorkflowStale
		}
		worktree = &observed
		root = observed.Root
	} else {
		projectRoot, projectErr := filepath.EvalSymlinks(workspace.root)
		cwd, cwdErr := filepath.EvalSymlinks(before.CWD)
		if projectErr != nil || cwdErr != nil || !pathInside(projectRoot, cwd) {
			return agentrun.Run{}, nil, nil, agentWorkflowGitBinding{}, ErrAgentWorkflowStale
		}
	}
	snapshotter := a.gitSnapshots
	if snapshotter == nil {
		snapshotter = gitinfo.Service{}
	}
	gitSnapshot, err := snapshotter.Capture(workspace.Context(), root)
	if err != nil {
		return agentrun.Run{}, nil, nil, agentWorkflowGitBinding{}, err
	}
	binding, err := buildAgentWorkflowGitBinding(gitSnapshot, kind, targetBranch)
	if err != nil {
		return agentrun.Run{}, nil, nil, agentWorkflowGitBinding{}, err
	}
	if worktree != nil && (binding.Root != worktree.Root ||
		binding.GitDir != worktree.GitDir || binding.CommonGitDir != worktree.CommonGitDir ||
		binding.Head != worktree.Head || binding.Branch != worktree.Branch) {
		return agentrun.Run{}, nil, nil, agentWorkflowGitBinding{}, ErrAgentWorkflowStale
	}
	after, afterAssociation, err := exactWorkflowRun(registry, host, runID, expectedAssociationRevision)
	if err != nil || before.LifecycleRevision != after.LifecycleRevision ||
		!runtimeAssociationsEqualOrNil(current, afterAssociation) {
		return agentrun.Run{}, nil, nil, agentWorkflowGitBinding{}, ErrAgentWorkflowStale
	}
	afterWorktree, err := currentWorkflowWorktree(workspace, after, afterAssociation)
	if err != nil || !sameWorktreeRepositoryIdentity(worktree, afterWorktree) {
		return agentrun.Run{}, nil, nil, agentWorkflowGitBinding{}, ErrAgentWorkflowStale
	}
	return after, afterAssociation, worktree, binding, nil
}

func exactWorkflowRun(
	registry interface {
		WithExactRuntimeSnapshot(int, func([]agentrun.Run) error) error
	},
	host *association.Host,
	runID string,
	expectedRevision uint64,
) (agentrun.Run, *RuntimeAssociation, error) {
	run, current, err := exactWorktreeRun(registry, host, runID, expectedRevision)
	if errors.Is(err, ErrAgentWorktreeInactive) {
		return agentrun.Run{}, nil, ErrAgentWorkflowInactive
	}
	if err != nil {
		return agentrun.Run{}, nil, err
	}
	return run, current, nil
}

func currentWorkflowWorktree(
	workspace *WorkspaceContext,
	run agentrun.Run,
	current *RuntimeAssociation,
) (*gitinfo.WorktreeIdentity, error) {
	workspace.worktreeMu.Lock()
	defer workspace.worktreeMu.Unlock()
	claim, exists := workspace.agentWorktrees[run.ID]
	if !exists {
		return nil, nil
	}
	if !worktreeClaimIsCurrent(workspace.Generation(), claim, run, current) {
		delete(workspace.agentWorktrees, run.ID)
		return nil, ErrAgentWorkflowStale
	}
	identity := claim.Identity
	return &identity, nil
}

func buildAgentWorkflowGitBinding(
	snapshot gitinfo.Snapshot,
	kind AgentWorkflowKind,
	targetBranch string,
) (agentWorkflowGitBinding, error) {
	if snapshot.State != gitinfo.RepositoryReady || snapshot.Bare || snapshot.Root == "" ||
		snapshot.GitDir == "" || snapshot.CommonGitDir == "" || snapshot.Status.Branch == "" ||
		!validWorkflowHead(snapshot.Status.OID) {
		return agentWorkflowGitBinding{}, ErrAgentWorkflowStale
	}
	targetHead := ""
	if kind == AgentWorkflowPullRequest || kind == AgentWorkflowMerge {
		for _, branch := range snapshot.LocalBranches {
			if branch.Name == targetBranch && branch.Name != snapshot.Status.Branch &&
				validWorkflowHead(branch.OID) {
				targetHead = branch.OID
				break
			}
		}
		if targetHead == "" {
			return agentWorkflowGitBinding{}, ErrAgentWorkflowTarget
		}
	} else if targetBranch != "" {
		return agentWorkflowGitBinding{}, ErrAgentWorkflowTarget
	}
	status := AgentWorkflowStatus{
		Staged: snapshot.Status.Staged, Unstaged: snapshot.Status.Unstaged,
		Untracked: snapshot.Status.Untracked, Conflicted: snapshot.Status.Conflicted,
		Ahead: snapshot.Status.Ahead, Behind: snapshot.Status.Behind,
	}
	digestInput := struct {
		Root, GitDir, CommonGitDir, Branch, Head, Upstream string
		TargetBranch, TargetHead                           string
		Detached, Initial                                  bool
		Status                                             AgentWorkflowStatus
		Changed, Untracked                                 []string
		ChangedMore, UntrackedMore                         int
		Divergence                                         *gitinfo.Divergence
	}{
		Root: snapshot.Root, GitDir: snapshot.GitDir, CommonGitDir: snapshot.CommonGitDir,
		Branch: snapshot.Status.Branch, Head: snapshot.Status.OID,
		TargetBranch: targetBranch, TargetHead: targetHead,
		Upstream: snapshot.Status.Upstream, Detached: snapshot.Status.Detached,
		Initial: snapshot.Status.Initial, Status: status,
		Changed:       append([]string(nil), snapshot.Status.ChangedPaths...),
		Untracked:     append([]string(nil), snapshot.Status.UntrackedPaths...),
		ChangedMore:   snapshot.Status.ChangedPathBounds.More,
		UntrackedMore: snapshot.Status.UntrackedPathBounds.More,
		Divergence:    snapshot.Divergence,
	}
	sort.Strings(digestInput.Changed)
	sort.Strings(digestInput.Untracked)
	encoded, err := json.Marshal(digestInput)
	if err != nil {
		return agentWorkflowGitBinding{}, err
	}
	digest := sha256.Sum256(encoded)
	return agentWorkflowGitBinding{
		Root: snapshot.Root, GitDir: snapshot.GitDir, CommonGitDir: snapshot.CommonGitDir,
		Branch: snapshot.Status.Branch, Head: snapshot.Status.OID,
		Digest: hex.EncodeToString(digest[:]), TargetBranch: targetBranch,
		TargetHead: targetHead, Status: status,
	}, nil
}

func validAgentWorkflowKind(kind AgentWorkflowKind) bool {
	return kind == AgentWorkflowValidation || kind == AgentWorkflowCommit ||
		kind == AgentWorkflowPullRequest || kind == AgentWorkflowMerge
}

func validWorkflowHead(head string) bool {
	if len(head) != 40 && len(head) != 64 {
		return false
	}
	_, err := hex.DecodeString(head)
	return err == nil
}

func worktreeIdentitiesEqual(left, right *gitinfo.WorktreeIdentity) bool {
	if left == nil || right == nil {
		return left == nil && right == nil
	}
	return *left == *right
}

func sameWorktreeRepositoryIdentity(left, right *gitinfo.WorktreeIdentity) bool {
	if left == nil || right == nil {
		return left == nil && right == nil
	}
	return left.Root == right.Root && left.GitDir == right.GitDir &&
		left.CommonGitDir == right.CommonGitDir && left.Linked == right.Linked
}

func projectAgentWorkflow(
	proposal agentWorkflowProposal,
	projectRoot string,
) AgentWorkflowProposalV2 {
	projected := AgentWorkflowProposalV2{
		ID: proposal.ID, Generation: proposal.Generation, Kind: proposal.Kind,
		State: proposal.State, RunID: proposal.RunID,
		Association: cloneRuntimeAssociation(proposal.Association),
		Branch:      proposal.Git.Branch, Head: proposal.Git.Head,
		TargetBranch: proposal.Git.TargetBranch, TargetHead: proposal.Git.TargetHead,
		Status:    proposal.Git.Status,
		CreatedAt: proposal.CreatedAt.UTC().Format(time.RFC3339Nano),
		ExpiresAt: proposal.ExpiresAt.UTC().Format(time.RFC3339Nano),
		Notice:    workflowNoExecutionNotice,
	}
	if proposal.Worktree != nil {
		projected.WorktreeRoot = proposal.Worktree.Root
		projected.Isolated = proposal.Worktree.Root != projectRoot
	}
	if !proposal.ApprovedAt.IsZero() {
		projected.ApprovedAt = proposal.ApprovedAt.UTC().Format(time.RFC3339Nano)
	}
	return projected
}

func buildAgentWorkflowInbox(
	workspace *WorkspaceContext,
	projection runtimeProjection,
) AgentWorkflowInbox {
	inbox := AgentWorkflowInbox{
		Items: []AgentWorkflowProposalV2{}, Notice: workflowNoExecutionNotice,
		Incomplete: projection.agentAnalysisIncomplete || projection.agentBounds.More > 0,
	}
	if workspace == nil || workspace.workflows == nil {
		inbox.Incomplete = true
		return inbox
	}
	associations := make(map[string]*RuntimeAssociation, len(projection.agentCandidates))
	for _, run := range projection.agentCandidates {
		associations[run.RunID] = run.Association
	}
	for _, proposal := range workspace.workflows.snapshot() {
		run, exists := projection.exactAgentRuns[proposal.RunID]
		if !exists || !agentRunIsLive(run) || run.LifecycleRevision != proposal.LifecycleRevision ||
			!runtimeAssociationsEqualOrNil(associations[proposal.RunID], proposal.Association) {
			workspace.workflows.remove(proposal.ID)
			continue
		}
		currentWorktree, err := currentWorkflowWorktree(
			workspace, run, associations[proposal.RunID],
		)
		if err != nil || !sameWorktreeRepositoryIdentity(currentWorktree, proposal.Worktree) {
			workspace.workflows.remove(proposal.ID)
			continue
		}
		inbox.Items = append(inbox.Items, projectAgentWorkflow(proposal, workspace.root))
	}
	inbox.Bounds = snapshotBound(len(inbox.Items), len(inbox.Items))
	return inbox
}

func workflowTargetBranches(snapshot gitinfo.Snapshot) ([]string, bool) {
	targets := make([]string, 0, len(snapshot.LocalBranches))
	seen := make(map[string]bool)
	for _, branch := range snapshot.LocalBranches {
		if branch.Name == "" || !validWorkflowHead(branch.OID) || seen[branch.Name] {
			continue
		}
		seen[branch.Name] = true
		targets = append(targets, branch.Name)
	}
	sort.Strings(targets)
	return targets, len(snapshot.LocalBranches) >= 100
}
