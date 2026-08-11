package gui

import (
	"context"
	"encoding/json"
	"errors"
	"path/filepath"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/gitinfo"
)

type mutableWorkflowSnapshotter struct {
	mu       sync.Mutex
	snapshot gitinfo.Snapshot
	calls    int
}

func (f *mutableWorkflowSnapshotter) Capture(context.Context, string) (gitinfo.Snapshot, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls++
	return cloneWorkflowGitSnapshot(f.snapshot), nil
}

func (f *mutableWorkflowSnapshotter) update(update func(*gitinfo.Snapshot)) {
	f.mu.Lock()
	defer f.mu.Unlock()
	update(&f.snapshot)
}

func (f *mutableWorkflowSnapshotter) callCount() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.calls
}

func TestAgentWorkflowApprovalIsExactOneTimeAndDoesNotExecute(t *testing.T) {
	app, registry, lease, _, _ := ownershipTestFixture(t, nil)
	snapshots := &mutableWorkflowSnapshotter{snapshot: workflowGitSnapshot(app.workspace.root)}
	app.gitSnapshots = snapshots
	before, err := registry.Run(lease.Run.ID)
	if err != nil {
		t.Fatal(err)
	}

	proposal, err := app.PrepareAgentWorkflowV2(
		1, lease.Run.ID, 1, AgentWorkflowPullRequest, "main",
	)
	if err != nil {
		t.Fatal(err)
	}
	if proposal.State != AgentWorkflowProposed || proposal.TargetBranch != "main" ||
		proposal.TargetHead != strings.Repeat("b", 40) || proposal.Head != strings.Repeat("a", 40) ||
		proposal.Notice != workflowNoExecutionNotice {
		t.Fatalf("proposal = %#v", proposal)
	}
	payload, err := json.Marshal(proposal)
	if err != nil {
		t.Fatal(err)
	}
	for _, forbidden := range []string{`"gitDir":`, `"commonGitDir":`, `"digest":`, `"command":`, `"refspec":`, `"remoteUrl":`} {
		if strings.Contains(string(payload), forbidden) {
			t.Fatalf("proposal exposed %q: %s", forbidden, payload)
		}
	}

	approved, err := app.ApproveAgentWorkflowV2(1, proposal.ID)
	if err != nil {
		t.Fatal(err)
	}
	if approved.State != AgentWorkflowApproved || approved.ApprovedAt == "" ||
		approved.Notice != workflowNoExecutionNotice {
		t.Fatalf("approved = %#v", approved)
	}
	if _, err := app.ApproveAgentWorkflowV2(1, proposal.ID); !errors.Is(err, ErrAgentWorkflowApproved) {
		t.Fatalf("second approval = %v", err)
	}
	after, err := registry.Run(lease.Run.ID)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(before, after) {
		t.Fatalf("proposal workflow changed run state: before=%#v after=%#v", before, after)
	}
	if snapshots.callCount() != 2 {
		t.Fatalf("Git snapshot calls = %d, want prepare and approval revalidation only", snapshots.callCount())
	}
}

func TestAgentWorkflowRejectsTargetMovementAndInvalidTargets(t *testing.T) {
	app, _, lease, _, _ := ownershipTestFixture(t, nil)
	snapshots := &mutableWorkflowSnapshotter{snapshot: workflowGitSnapshot(app.workspace.root)}
	app.gitSnapshots = snapshots

	if _, err := app.PrepareAgentWorkflowV2(
		1, lease.Run.ID, 1, AgentWorkflowValidation, "main",
	); !errors.Is(err, ErrAgentWorkflowTarget) {
		t.Fatalf("validation target = %v", err)
	}
	if _, err := app.PrepareAgentWorkflowV2(
		1, lease.Run.ID, 1, AgentWorkflowMerge, "missing",
	); !errors.Is(err, ErrAgentWorkflowTarget) {
		t.Fatalf("missing merge target = %v", err)
	}
	if _, err := app.PrepareAgentWorkflowV2(
		1, lease.Run.ID, 1, AgentWorkflowKind("deploy"), "",
	); !errors.Is(err, ErrAgentWorkflowKind) {
		t.Fatalf("unsupported kind = %v", err)
	}

	proposal, err := app.PrepareAgentWorkflowV2(
		1, lease.Run.ID, 1, AgentWorkflowMerge, "main",
	)
	if err != nil {
		t.Fatal(err)
	}
	snapshots.update(func(snapshot *gitinfo.Snapshot) {
		for index := range snapshot.LocalBranches {
			if snapshot.LocalBranches[index].Name == "main" {
				snapshot.LocalBranches[index].OID = strings.Repeat("c", 40)
			}
		}
	})
	if _, err := app.ApproveAgentWorkflowV2(1, proposal.ID); !errors.Is(err, ErrAgentWorkflowStale) {
		t.Fatalf("moved target approval = %v", err)
	}
	if _, exists := app.workspace.workflows.get(proposal.ID); exists {
		t.Fatal("stale target proposal remained in the workflow registry")
	}
}

func TestAgentWorkflowRegistryIsBoundedExpiringAndProjectScoped(t *testing.T) {
	now := time.Date(2026, time.August, 10, 12, 0, 0, 0, time.UTC)
	registry := newAgentWorkflowRegistry(func() time.Time { return now })
	for index := 0; index < agentWorkflowLimit; index++ {
		id := string(rune('a' + index))
		err := registry.add(agentWorkflowProposal{
			ID: id, Generation: 1, State: AgentWorkflowProposed,
			CreatedAt: now, ExpiresAt: now.Add(agentWorkflowTTL),
		})
		if err != nil {
			t.Fatalf("add %d: %v", index, err)
		}
	}
	if err := registry.add(agentWorkflowProposal{
		ID: "overflow", Generation: 1, State: AgentWorkflowProposed,
		CreatedAt: now, ExpiresAt: now.Add(agentWorkflowTTL),
	}); !errors.Is(err, ErrAgentWorkflowFull) {
		t.Fatalf("overflow = %v", err)
	}
	now = now.Add(agentWorkflowTTL)
	if items := registry.snapshot(); len(items) != 0 {
		t.Fatalf("expired proposals = %#v", items)
	}
	other := newWorkspaceContext(workspaceContextConfig{generation: 2})
	if items := other.workflows.snapshot(); len(items) != 0 {
		t.Fatalf("new project inherited proposals: %#v", items)
	}
}

func TestWorkflowTargetsIncludeSharedCheckoutBranchForIsolatedAgents(t *testing.T) {
	snapshot := workflowGitSnapshot("/project")
	targets, incomplete := workflowTargetBranches(snapshot)
	if incomplete || len(targets) != 2 || targets[0] != "feature" || targets[1] != "main" {
		t.Fatalf("workflow targets = %#v incomplete=%v", targets, incomplete)
	}
}

func TestAgentWorkflowRefreshesMutableWorktreeHead(t *testing.T) {
	app, _, lease, _, _ := ownershipTestFixture(t, nil)
	root := app.workspace.root
	identity := gitinfo.WorktreeIdentity{
		Root: root, GitDir: filepath.Join(root, ".git"),
		CommonGitDir: filepath.Join(root, ".git"), Branch: "feature",
		Head: strings.Repeat("a", 40),
	}
	app.gitWorktrees = fakeWorktreeInspector{identity: identity}
	if _, err := app.SetAgentWorktreeV2(1, lease.Run.ID, 1, root, true); err != nil {
		t.Fatal(err)
	}
	identity.Head = strings.Repeat("c", 40)
	app.gitWorktrees = fakeWorktreeInspector{identity: identity}
	snapshot := workflowGitSnapshot(root)
	snapshot.Status.OID = identity.Head
	snapshot.LocalBranches[0].OID = identity.Head
	app.gitSnapshots = &mutableWorkflowSnapshotter{snapshot: snapshot}
	proposal, err := app.PrepareAgentWorkflowV2(
		1, lease.Run.ID, 1, AgentWorkflowPullRequest, "main",
	)
	if err != nil {
		t.Fatal(err)
	}
	if proposal.Head != identity.Head {
		t.Fatalf("workflow retained stale associated HEAD: %#v", proposal)
	}
}

func TestAgentWorkflowRejectsOutsideProjectRunWithoutExplicitWorktreeClaim(t *testing.T) {
	app, projectRoot := newTerminalBindingTestApp(t, &fakeGUITerminalManager{}, nil)
	planID, taskID, _ := seedAssociationCatalog(t, projectRoot)
	sibling, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	registry := agentrun.NewRegistry(agentrun.Config{
		ProjectRoot: projectRoot,
		AdditionalCWDValidator: func(candidate string) bool {
			return pathInside(sibling, candidate)
		},
	})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	app.workspace.agents = &workspaceAgentResources{registry: registry}
	run, err := registry.RegisterLaunched(agentrun.Registration{
		Profile: "parallel", Provider: "codex", CWD: sibling,
		PID: 42, TerminalID: "parallel-terminal",
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := app.AssociateAgentRunV2(1, run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: planID, TaskID: taskID,
	}); err != nil {
		t.Fatal(err)
	}
	app.gitSnapshots = &mutableWorkflowSnapshotter{snapshot: workflowGitSnapshot(projectRoot)}
	if _, err := app.PrepareAgentWorkflowV2(
		1, run.ID, 1, AgentWorkflowValidation, "",
	); !errors.Is(err, ErrAgentWorkflowStale) {
		t.Fatalf("outside-project workflow without explicit claim = %v", err)
	}
}

func workflowGitSnapshot(root string) gitinfo.Snapshot {
	head := strings.Repeat("a", 40)
	target := strings.Repeat("b", 40)
	gitDir := filepath.Join(root, ".git")
	return gitinfo.Snapshot{
		State: gitinfo.RepositoryReady, Root: root, GitDir: gitDir, CommonGitDir: gitDir,
		Status: gitinfo.Status{
			Branch: "feature", OID: head, Upstream: "origin/feature",
			Staged: 1, Unstaged: 2, Untracked: 3, Ahead: 1,
			ChangedPaths: []string{"internal/gui/agent_workflow.go"},
		},
		LocalBranches: []gitinfo.Branch{
			{Name: "feature", OID: head, Current: true},
			{Name: "main", OID: target},
		},
		Divergence: &gitinfo.Divergence{Upstream: "origin/feature", Ahead: 1},
	}
}

func cloneWorkflowGitSnapshot(snapshot gitinfo.Snapshot) gitinfo.Snapshot {
	snapshot.LocalBranches = append([]gitinfo.Branch(nil), snapshot.LocalBranches...)
	snapshot.Status.ChangedPaths = append([]string(nil), snapshot.Status.ChangedPaths...)
	snapshot.Status.UntrackedPaths = append([]string(nil), snapshot.Status.UntrackedPaths...)
	if snapshot.Divergence != nil {
		copy := *snapshot.Divergence
		snapshot.Divergence = &copy
	}
	return snapshot
}
