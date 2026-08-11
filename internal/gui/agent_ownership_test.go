package gui

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/gitinfo"
)

func TestAgentTaskOwnershipRequiresExplicitClaimAndRelease(t *testing.T) {
	app, registry, lease, planID, taskID := ownershipTestFixture(t, nil)
	claimed, err := app.SetAgentTaskOwnershipV2(1, lease.Run.ID, 1, true)
	if err != nil {
		t.Fatal(err)
	}
	if !claimed.Owned || claimed.Ownership == nil ||
		claimed.Ownership.PlanID != planID || claimed.Ownership.TaskID != taskID ||
		claimed.Ownership.AssociationRevision != 1 {
		t.Fatalf("claim = %#v", claimed)
	}
	assertSnapshotOwnership(t, app, lease.Run.ID, true)

	released, err := app.SetAgentTaskOwnershipV2(1, lease.Run.ID, 1, false)
	if err != nil {
		t.Fatal(err)
	}
	if released.Owned || released.Ownership != nil {
		t.Fatalf("release = %#v", released)
	}
	assertSnapshotOwnership(t, app, lease.Run.ID, false)
	_ = registry
}

func TestAgentTaskOwnershipRejectsRevisionMismatchAndInactiveRun(t *testing.T) {
	app, registry, lease, _, _ := ownershipTestFixture(t, nil)
	if _, err := app.SetAgentTaskOwnershipV2(1, lease.Run.ID, 2, true); !errors.Is(err, ErrAgentOwnershipRevision) {
		t.Fatalf("revision mismatch = %v", err)
	}
	if err := registry.ExitExternal(lease.Run.ID, lease.LeaseToken, 0, "completed"); err != nil {
		t.Fatal(err)
	}
	if _, err := app.SetAgentTaskOwnershipV2(1, lease.Run.ID, 1, true); !errors.Is(err, ErrAgentOwnershipInactive) {
		t.Fatalf("inactive claim = %v", err)
	}
}

func TestAgentTaskOwnershipDoesNotReviveAfterStaleHeartbeatOrReassociation(t *testing.T) {
	now := time.Date(2026, time.August, 10, 12, 0, 0, 0, time.UTC)
	app, registry, lease, planID, taskID := ownershipTestFixture(t, func(config *agentrun.Config) {
		config.Now = func() time.Time { return now }
		config.LeaseDuration = time.Second
	})
	if _, err := app.SetAgentTaskOwnershipV2(1, lease.Run.ID, 1, true); err != nil {
		t.Fatal(err)
	}
	now = now.Add(2 * time.Second)
	assertSnapshotOwnership(t, app, lease.Run.ID, false)
	if err := registry.Heartbeat(lease.Run.ID, lease.LeaseToken); err != nil {
		t.Fatal(err)
	}
	assertSnapshotOwnership(t, app, lease.Run.ID, false)

	if _, err := app.SetAgentTaskOwnershipV2(1, lease.Run.ID, 1, true); err != nil {
		t.Fatal(err)
	}
	if _, err := app.AssociateAgentRunV2(1, lease.Run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: planID, TaskID: taskID,
	}); err != nil {
		t.Fatal(err)
	}
	assertSnapshotOwnership(t, app, lease.Run.ID, false)
}

func TestAgentTaskOwnershipRejectsStaleReleaseAfterReclaim(t *testing.T) {
	app, _, lease, planID, taskID := ownershipTestFixture(t, nil)
	if _, err := app.SetAgentTaskOwnershipV2(1, lease.Run.ID, 1, true); err != nil {
		t.Fatal(err)
	}
	if _, err := app.AssociateAgentRunV2(1, lease.Run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: planID, TaskID: taskID,
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := app.SetAgentTaskOwnershipV2(1, lease.Run.ID, 2, true); err != nil {
		t.Fatal(err)
	}
	if _, err := app.SetAgentTaskOwnershipV2(1, lease.Run.ID, 1, false); !errors.Is(err, ErrAgentOwnershipRevision) {
		t.Fatalf("stale release = %v", err)
	}
	assertSnapshotOwnership(t, app, lease.Run.ID, true)
}

func TestAgentActivityConflictsDeduplicateLogicalRunsAndExposeIncompleteAnalysis(t *testing.T) {
	current := &RuntimeAssociation{PlanID: 5, TaskID: 38, Revision: 1}
	runs := []AgentRuntimeSummary{
		{RunID: "run-a", Live: true, TerminalBacked: true, Association: current},
		{RunID: "run-a", Live: true, TerminalBacked: true, Association: current},
		{RunID: "run-b", Live: true, Association: current},
		{RunID: "run-c", Live: false, Association: current},
		{RunID: "run-d", Live: true, Association: &RuntimeAssociation{PlanID: 5, TaskID: 39, Revision: 1}},
	}
	conflicts, bounds := agentActivityConflicts(runs, map[string]agentTaskOwnershipClaim{
		"run-a": {RunID: "run-a"},
	})
	if len(conflicts) != 1 || bounds.Total != 1 || conflicts[0].AgentCount != 2 ||
		conflicts[0].OwnerCount != 1 || len(conflicts[0].RunIDs) != 2 {
		t.Fatalf("conflicts = %#v bounds=%#v", conflicts, bounds)
	}

	workspace := newWorkspaceContext(workspaceContextConfig{generation: 1})
	activity := buildAgentActivitySnapshot(runtimeProjection{})
	applyAgentOwnership(workspace, &activity, runtimeProjection{agentAnalysisIncomplete: true})
	if !activity.AnalysisIncomplete {
		t.Fatal("truncated runtime projection claimed complete conflict analysis")
	}
	other := newWorkspaceContext(workspaceContextConfig{generation: 2})
	if len(other.agentOwnership) != 0 {
		t.Fatal("a new project workspace inherited ownership")
	}
}

func ownershipTestFixture(
	t *testing.T,
	configure func(*agentrun.Config),
) (*App, *agentrun.Registry, agentrun.Lease, uint64, uint64) {
	t.Helper()
	app, projectRoot := newTerminalBindingTestApp(t, &fakeGUITerminalManager{}, nil)
	planID, taskID, _ := seedAssociationCatalog(t, projectRoot)
	config := agentrun.Config{ProjectRoot: projectRoot}
	if configure != nil {
		configure(&config)
	}
	registry := agentrun.NewRegistry(config)
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	app.workspace.agents = &workspaceAgentResources{registry: registry}
	app.gitSnapshots = fakeGitSnapshotter{
		snapshot: gitinfo.Snapshot{State: gitinfo.RepositoryNotFound},
	}
	lease, err := registry.RegisterExternal(agentrun.Registration{
		Profile: "wrapper", Provider: "codex", CWD: projectRoot,
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := app.AssociateAgentRunV2(1, lease.Run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: planID, TaskID: taskID,
	}); err != nil {
		t.Fatal(err)
	}
	return app, registry, lease, planID, taskID
}

func assertSnapshotOwnership(t *testing.T, app *App, runID string, want bool) {
	t.Helper()
	snapshot, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	for _, item := range snapshot.AgentActivity.Items {
		if item.RunID == runID {
			if (item.Ownership != nil) != want {
				t.Fatalf("ownership = %#v, want present=%v", item.Ownership, want)
			}
			return
		}
	}
	t.Fatalf("activity lacks run %q: %#v", runID, snapshot.AgentActivity.Items)
}
