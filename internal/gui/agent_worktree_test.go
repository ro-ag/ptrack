package gui

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/gitinfo"
)

type fakeWorktreeInspector struct {
	identity gitinfo.WorktreeIdentity
	err      error
}

func (f fakeWorktreeInspector) InspectWorktree(
	context.Context,
	string,
	string,
) (gitinfo.WorktreeIdentity, error) {
	return f.identity, f.err
}

func TestAgentWorktreeAssociationIsExplicitVerifiedAndLifecycleBound(t *testing.T) {
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
	identity := gitinfo.WorktreeIdentity{
		Root: sibling, GitDir: filepath.Join(projectRoot, ".git", "worktrees", "parallel"),
		CommonGitDir: filepath.Join(projectRoot, ".git"), Branch: "parallel",
		Head: strings.Repeat("a", 40), Linked: true,
	}
	app.gitWorktrees = fakeWorktreeInspector{identity: identity}
	app.gitSnapshots = fakeGitSnapshotter{snapshot: gitinfo.Snapshot{
		State: gitinfo.RepositoryReady,
		Worktrees: []gitinfo.ExistingWorktree{{
			Root: sibling, Branch: "parallel", Head: identity.Head,
		}},
		WorktreeBounds: gitinfo.WorktreeBounds{Shown: 1, Total: 1},
	}}
	mutation, err := app.SetAgentWorktreeV2(1, run.ID, 1, sibling, true)
	if err != nil {
		t.Fatal(err)
	}
	if mutation.Worktree == nil || !mutation.Worktree.Verified ||
		!mutation.Worktree.Isolated || !mutation.Worktree.CWDMatches {
		t.Fatalf("worktree mutation = %#v", mutation)
	}
	snapshot, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	if len(snapshot.AgentActivity.Items) != 1 ||
		snapshot.AgentActivity.Items[0].Worktree == nil ||
		len(snapshot.AgentActivity.Worktrees) != 1 {
		t.Fatalf("worktree activity = %#v", snapshot.AgentActivity)
	}
	encoded, err := json.Marshal(snapshot.AgentActivity)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(encoded), "gitDir") ||
		strings.Contains(string(encoded), "commonGitDir") {
		t.Fatalf("activity exposed internal Git directories: %s", encoded)
	}

	if _, err := app.AssociateAgentRunV2(1, run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: planID, TaskID: taskID,
	}); err != nil {
		t.Fatal(err)
	}
	refreshed, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	if refreshed.AgentActivity.Items[0].Worktree != nil {
		t.Fatalf("relinked run retained worktree metadata: %#v", refreshed.AgentActivity.Items[0])
	}
	if _, err := app.SetAgentWorktreeV2(1, run.ID, 2, sibling, true); err != nil {
		t.Fatal(err)
	}
	if _, err := app.SetAgentWorktreeV2(1, run.ID, 1, "", false); !errors.Is(err, ErrAgentWorktreeRevision) {
		t.Fatalf("stale detach = %v", err)
	}
	refreshed, err = app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	if refreshed.AgentActivity.Items[0].Worktree == nil {
		t.Fatal("stale detach removed the current worktree association")
	}
	other := newWorkspaceContext(workspaceContextConfig{generation: 2})
	if len(other.agentWorktrees) != 0 {
		t.Fatal("new project workspace inherited worktree metadata")
	}
}

func TestAgentWorktreeRequiresMatchingCWDAndLinkedLaunchValidation(t *testing.T) {
	projectRoot, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	sibling, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	subdir := filepath.Join(sibling, "subdir")
	if err := mkdirTestDir(subdir); err != nil {
		t.Fatal(err)
	}
	identity := gitinfo.WorktreeIdentity{
		Root: sibling, CommonGitDir: filepath.Join(projectRoot, ".git"),
		GitDir: filepath.Join(projectRoot, ".git", "worktrees", "parallel"),
		Head:   strings.Repeat("b", 40), Linked: true,
	}
	app := &App{gitWorktrees: fakeWorktreeInspector{identity: identity}}
	resolved, err := app.resolveLinkedLaunchCWD(context.Background(), projectRoot, subdir)
	if err != nil || resolved != subdir {
		t.Fatalf("linked worktree CWD = %q err=%v", resolved, err)
	}
	app.gitWorktrees = fakeWorktreeInspector{err: errors.New("different repository")}
	if _, err := app.resolveLinkedLaunchCWD(
		context.Background(), projectRoot, sibling,
	); err == nil {
		t.Fatal("unverified sibling CWD was accepted")
	}
}

func mkdirTestDir(path string) error {
	return os.MkdirAll(path, 0o700)
}
