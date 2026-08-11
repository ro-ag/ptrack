package gui

import (
	"context"
	"errors"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/gitinfo"
)

func TestAgentCoordinationDetectsDuplicateWorkAndDeduplicatesLogicalAgents(t *testing.T) {
	app, registry, first, planID, taskID := ownershipTestFixture(t, nil)
	second, err := registry.RegisterExternal(agentrun.Registration{
		Profile: "second", Provider: "codex", CWD: app.workspace.root,
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := app.AssociateAgentRunV2(1, second.Run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: planID, TaskID: taskID,
	}); err != nil {
		t.Fatal(err)
	}
	for _, runID := range []string{first.Run.ID, second.Run.ID} {
		if _, err := app.SetAgentTaskOwnershipV2(1, runID, 1, true); err != nil {
			t.Fatal(err)
		}
	}
	snapshot, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	if len(snapshot.AgentActivity.Items) != 2 || len(snapshot.AgentActivity.Conflicts) != 1 {
		t.Fatalf("activity = %#v", snapshot.AgentActivity)
	}
	conflict := snapshot.AgentActivity.Conflicts[0]
	if conflict.AgentCount != 2 || conflict.OwnerCount != 2 ||
		conflict.PlanID != planID || conflict.TaskID != taskID {
		t.Fatalf("conflict = %#v", conflict)
	}

	// A linked terminal and its AgentRun may be observed more than once by a
	// bounded caller, but the logical worker is counted once by run ID.
	current := &RuntimeAssociation{PlanID: planID, TaskID: taskID, Revision: 1}
	conflicts, _ := agentActivityConflicts([]AgentRuntimeSummary{
		{RunID: first.Run.ID, Live: true, TerminalBacked: true, Association: current},
		{RunID: first.Run.ID, Live: true, TerminalBacked: true, Association: current},
		{RunID: second.Run.ID, Live: true, Association: current},
	}, map[string]agentTaskOwnershipClaim{
		first.Run.ID:  {RunID: first.Run.ID},
		second.Run.ID: {RunID: second.Run.ID},
	})
	if len(conflicts) != 1 || conflicts[0].AgentCount != 2 {
		t.Fatalf("deduplicated conflicts = %#v", conflicts)
	}
}

func TestAgentCoordinationStaleOwnershipDoesNotReviveAfterHeartbeat(t *testing.T) {
	now := time.Date(2026, time.August, 10, 12, 0, 0, 0, time.UTC)
	app, registry, lease, _, _ := ownershipTestFixture(t, func(config *agentrun.Config) {
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
}

func TestAgentCoordinationConcurrentWorkflowApprovalIsOneTime(t *testing.T) {
	app, _, lease, _, _ := ownershipTestFixture(t, nil)
	app.gitSnapshots = &mutableWorkflowSnapshotter{
		snapshot: workflowGitSnapshot(app.workspace.root),
	}
	proposal, err := app.PrepareAgentWorkflowV2(
		1, lease.Run.ID, 1, AgentWorkflowValidation, "",
	)
	if err != nil {
		t.Fatal(err)
	}

	const attempts = 8
	results := make(chan error, attempts)
	start := make(chan struct{})
	var ready sync.WaitGroup
	ready.Add(attempts)
	for range attempts {
		go func() {
			ready.Done()
			<-start
			_, approveErr := app.ApproveAgentWorkflowV2(1, proposal.ID)
			results <- approveErr
		}()
	}
	ready.Wait()
	close(start)

	successes := 0
	for range attempts {
		err := <-results
		switch {
		case err == nil:
			successes++
		case errors.Is(err, ErrAgentWorkflowApproved), errors.Is(err, ErrAgentWorkflowStale):
		default:
			t.Fatalf("concurrent workflow approval returned %v", err)
		}
	}
	if successes != 1 {
		t.Fatalf("workflow approval successes = %d, want exactly 1", successes)
	}
}

func TestAgentCoordinationConcurrentHandoffAcknowledgementIsOneTime(t *testing.T) {
	app, _, source, target, _, _ := handoffDeliveryFixture(t)
	envelope, err := app.SendAgentHandoffV2(
		1, source.Run.ID, target.Run.ID, 1, 1,
	)
	if err != nil {
		t.Fatal(err)
	}

	const attempts = 8
	results := make(chan error, attempts)
	start := make(chan struct{})
	var ready sync.WaitGroup
	ready.Add(attempts)
	// Hold exact-state validation so every contender can observe the same
	// envelope before any contender is allowed to remove it.
	app.workspace.associationMu.Lock()
	for range attempts {
		go func() {
			ready.Done()
			<-start
			_, acknowledgeErr := app.AcknowledgeAgentHandoffV2(
				1, envelope.ID, target.Run.ID,
			)
			results <- acknowledgeErr
		}()
	}
	ready.Wait()
	close(start)
	// Opening the bounded project store occurs before acquisition of
	// associationMu. Give every contender time to reach the held fence.
	time.Sleep(100 * time.Millisecond)
	app.workspace.associationMu.Unlock()

	successes := 0
	for range attempts {
		err := <-results
		switch {
		case err == nil:
			successes++
		case errors.Is(err, ErrAgentHandoffStale):
		default:
			t.Fatalf("concurrent handoff acknowledgement returned %v", err)
		}
	}
	if successes != 1 {
		t.Fatalf("handoff acknowledgement successes = %d, want exactly 1", successes)
	}
}

func TestAgentCoordinationProjectSwitchClearsEphemeralStateAndFencesOldGeneration(t *testing.T) {
	app, registry, source, target, planID, taskID := handoffDeliveryFixture(t)
	if _, err := app.SetAgentTaskOwnershipV2(1, source.Run.ID, 1, true); err != nil {
		t.Fatal(err)
	}
	if _, err := registry.RecordProviderEvent(
		source.Run.ID,
		source.LeaseToken,
		agentrun.ProviderEvent{
			ModelVersion: agentrun.ProviderEventModelVersion,
			ID:           "switch-question", Sequence: 1, Type: "question",
			Summary: "PROJECT_SWITCH_SECRET",
		},
	); err != nil {
		t.Fatal(err)
	}
	var run agentrun.Run
	if err := registry.WithExactRuntimeSnapshot(
		linkedRuntimeCandidateLimit,
		func(runs []agentrun.Run) error {
			for _, candidate := range runs {
				if candidate.ID == source.Run.ID {
					run = candidate
					return nil
				}
			}
			return agentrun.ErrRunNotFound
		},
	); err != nil {
		t.Fatal(err)
	}
	gitSnapshot := workflowGitSnapshot(app.workspace.root)
	identity := gitinfo.WorktreeIdentity{
		Root: app.workspace.root, GitDir: gitSnapshot.GitDir,
		CommonGitDir: gitSnapshot.CommonGitDir, Branch: gitSnapshot.Status.Branch,
		Head: gitSnapshot.Status.OID,
	}
	app.gitSnapshots = &mutableWorkflowSnapshotter{snapshot: gitSnapshot}
	app.gitWorktrees = fakeWorktreeInspector{identity: identity}
	app.workspace.worktreeMu.Lock()
	app.workspace.agentWorktrees[source.Run.ID] = agentWorktreeClaim{
		Generation: 1, RunID: source.Run.ID, LifecycleRevision: run.LifecycleRevision,
		AssociationRevision: 1, PlanID: planID, TaskID: taskID, Identity: identity,
	}
	app.workspace.worktreeMu.Unlock()
	handoff, err := app.SendAgentHandoffV2(1, source.Run.ID, target.Run.ID, 1, 1)
	if err != nil {
		t.Fatal(err)
	}
	workflow, err := app.PrepareAgentWorkflowV2(
		1, source.Run.ID, 1, AgentWorkflowValidation, "",
	)
	if err != nil {
		t.Fatal(err)
	}
	before, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	var sourceActivity *AgentActivity
	for index := range before.AgentActivity.Items {
		if before.AgentActivity.Items[index].RunID == source.Run.ID {
			sourceActivity = &before.AgentActivity.Items[index]
			break
		}
	}
	if len(before.AgentActivity.Notifications) != 1 ||
		len(before.AgentActivity.Handoffs.Items) != 1 ||
		len(before.AgentActivity.Workflows.Items) != 1 ||
		sourceActivity == nil || sourceActivity.Ownership == nil ||
		sourceActivity.Worktree == nil {
		t.Fatalf("old project coordination state = %#v", before.AgentActivity)
	}

	oldWorkspace := app.workspace
	nextRoot, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	next := newWorkspaceContext(workspaceContextConfig{
		generation: 2, root: nextRoot,
	})
	app.workspaceMu.Lock()
	app.workspace = next
	app.lastGeneration = 2
	app.workspaceMu.Unlock()
	if len(next.agentOwnership) != 0 || len(next.agentWorktrees) != 0 ||
		len(next.handoffs.snapshot()) != 0 || len(next.workflows.snapshot()) != 0 {
		t.Fatalf("new project inherited ephemeral state: %#v", next)
	}
	if notifications, _, _ := buildAgentNotifications(next, runtimeProjection{}); len(notifications) != 0 {
		t.Fatalf("new project inherited notifications: %#v", notifications)
	}

	if _, err := app.SetAgentTaskOwnershipV2(1, source.Run.ID, 1, false); !errors.Is(err, errStaleWorkspaceGeneration) {
		t.Fatalf("old ownership generation = %v", err)
	}
	if _, err := app.SetAgentWorktreeV2(1, source.Run.ID, 1, "", false); !errors.Is(err, errStaleWorkspaceGeneration) {
		t.Fatalf("old worktree generation = %v", err)
	}
	if _, err := app.AcknowledgeAgentHandoffV2(1, handoff.ID, target.Run.ID); !errors.Is(err, errStaleWorkspaceGeneration) {
		t.Fatalf("old handoff generation = %v", err)
	}
	if _, err := app.ApproveAgentWorkflowV2(1, workflow.ID); !errors.Is(err, errStaleWorkspaceGeneration) {
		t.Fatalf("old workflow generation = %v", err)
	}

	// Restore the fixture-owned context so its registered cleanup remains
	// authoritative and the synthetic next context is closed explicitly.
	app.workspaceMu.Lock()
	app.workspace = oldWorkspace
	app.lastGeneration = 1
	app.workspaceMu.Unlock()
	if err := next.Close(context.Background()); err != nil {
		t.Fatal(err)
	}
	if strings.Contains(workflow.Notice, "PROJECT_SWITCH_SECRET") {
		t.Fatal("workflow DTO retained provider content")
	}
}

func TestAgentCoordinationRuntimeNotificationIsPublishedGenerationScoped(t *testing.T) {
	type emitted struct {
		name    string
		payload any
	}
	events := []emitted{}
	app := newWorkspaceCoordinator(nil, func(_ context.Context, name string, payload any) {
		events = append(events, emitted{name: name, payload: payload})
	})
	app.wailsContext = context.Background()
	resources := &workspaceAgentResources{}
	workspace := newWorkspaceContext(workspaceContextConfig{
		generation: 3, agents: resources,
	})
	app.bindWorkspaceRuntimeNotifications(workspace)
	resources.runtimeChanged()
	if len(events) != 0 {
		t.Fatalf("unpublished workspace emitted runtime change: %#v", events)
	}
	app.workspace = workspace
	app.lastGeneration = 3
	resources.runtimeChanged()
	if len(events) != 1 || events[0].name != workspaceRuntimeChangedEvent ||
		events[0].payload != uint64(3) {
		t.Fatalf("published runtime changes = %#v", events)
	}
	app.workspace = newWorkspaceContext(workspaceContextConfig{generation: 4})
	resources.runtimeChanged()
	if len(events) != 1 {
		t.Fatalf("old workspace emitted after replacement: %#v", events)
	}
}
