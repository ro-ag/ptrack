package gui

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"sync"
	"testing"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/store"
	"github.com/ro-ag/ptrack/internal/terminal"
)

func TestAssociateTerminalV2ValidatesCurrentProjectAndRevision(t *testing.T) {
	manager := &fakeGUITerminalManager{}
	app, projectRoot := newTerminalBindingTestApp(t, manager, nil)
	planID, taskID, otherPlanID := seedAssociationCatalog(t, projectRoot)

	first, err := app.AssociateTerminalV2(1, "opaque-session", association.PointerV1{
		Version: association.VersionV1, PlanID: planID, TaskID: taskID,
	})
	if err != nil {
		t.Fatal(err)
	}
	second, err := app.AssociateTerminalV2(1, "opaque-session", association.PointerV1{
		Version: association.VersionV1, PlanID: planID,
	})
	if err != nil {
		t.Fatal(err)
	}
	canonicalRoot, _ := filepath.EvalSymlinks(projectRoot)
	if first.ProjectRoot != canonicalRoot || first.Generation != 1 ||
		first.LiveID != "opaque-session" || first.Revision != 1 ||
		second.Revision != 2 {
		t.Fatalf("associations = first %#v second %#v", first, second)
	}
	if _, err := app.AssociateTerminalV2(2, "opaque-session", association.PointerV1{
		Version: association.VersionV1,
	}); !errors.Is(err, errStaleWorkspaceGeneration) {
		t.Fatalf("stale generation = %v", err)
	}
	if _, err := app.AssociateTerminalV2(1, "opaque-session", association.PointerV1{
		Version: 2,
	}); !errors.Is(err, association.ErrUnsupportedVersion) {
		t.Fatalf("unsupported version = %v", err)
	}
	if _, err := app.AssociateTerminalV2(1, "opaque-session", association.PointerV1{
		Version: association.VersionV1, PlanID: otherPlanID, TaskID: taskID,
	}); !errors.Is(err, association.ErrInvalidTarget) {
		t.Fatalf("mismatched task = %v", err)
	}
}

func TestAssociateAgentRunV2RequiresExplicitHostBinding(t *testing.T) {
	app, projectRoot := newTerminalBindingTestApp(t, &fakeGUITerminalManager{}, nil)
	planID, taskID, _ := seedAssociationCatalog(t, projectRoot)
	registry := agentrun.NewRegistry(agentrun.Config{ProjectRoot: projectRoot})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	app.workspace.agents = &workspaceAgentResources{registry: registry}
	lease, err := registry.RegisterExternal(agentrun.Registration{
		Profile: "wrapper", Provider: "external", CWD: projectRoot,
	})
	if err != nil {
		t.Fatal(err)
	}
	if lease.Run.Association != nil {
		t.Fatalf("external run registered with association %#v", lease.Run.Association)
	}

	bound, err := app.AssociateAgentRunV2(1, lease.Run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: planID, TaskID: taskID,
	})
	if err != nil {
		t.Fatal(err)
	}
	if bound.LiveID != lease.Run.ID || bound.Generation != 1 || bound.Revision != 1 {
		t.Fatalf("bound association = %#v", bound)
	}
	run := registry.Snapshot(1)[0]
	if run.Association == nil || run.Association.Target.TaskID != taskID {
		t.Fatalf("associated run = %#v", run)
	}
	if _, err := app.AssociateAgentRunV2(1, "missing", association.PointerV1{
		Version: association.VersionV1,
	}); !errors.Is(err, agentrun.ErrRunNotFound) {
		t.Fatalf("missing run = %v", err)
	}
}

func TestAssociationChangesPublishGenerationScopedRuntimeRefresh(t *testing.T) {
	var mu sync.Mutex
	events := []emittedTerminalEvent{}
	emitter := func(ctx context.Context, name string, payload any) {
		mu.Lock()
		defer mu.Unlock()
		events = append(events, emittedTerminalEvent{ctx: ctx, name: name, payload: payload})
	}
	app, projectRoot := newTerminalBindingTestApp(
		t,
		&fakeGUITerminalManager{},
		emitter,
	)
	planID, _, _ := seedAssociationCatalog(t, projectRoot)
	if _, err := app.AssociateTerminalV2(1, "opaque-session", association.PointerV1{
		Version: association.VersionV1,
		PlanID:  planID,
	}); err != nil {
		t.Fatal(err)
	}

	mu.Lock()
	defer mu.Unlock()
	if len(events) != 1 || events[0].name != workspaceRuntimeChangedEvent ||
		events[0].payload != uint64(1) {
		t.Fatalf("runtime refresh events = %#v", events)
	}
}

func TestAssociationHostRejectsCrossProjectDatabaseSymlinkBeforeMutation(t *testing.T) {
	manager := &fakeGUITerminalManager{}
	app, workspaceRoot := newTerminalBindingTestApp(t, manager, nil)
	otherRoot := t.TempDir()
	if err := os.Mkdir(filepath.Join(otherRoot, ".ptrack"), 0o755); err != nil {
		t.Fatal(err)
	}
	otherDB := filepath.Join(otherRoot, ".ptrack", "ptrack.db")
	otherStore, err := store.Open(otherDB)
	if err != nil {
		t.Fatal(err)
	}
	plan, err := otherStore.AddPlan("Other project plan")
	if err != nil {
		t.Fatal(err)
	}
	task, err := otherStore.AddTask(plan.ID, "Other project task")
	if err != nil {
		t.Fatal(err)
	}
	vulnerableHost, err := association.NewHost(
		workspaceRoot, 1, storeAssociationCatalog{store: otherStore},
	)
	if err != nil {
		t.Fatal(err)
	}
	preexisting, err := vulnerableHost.Bind("opaque-session", association.PointerV1{
		Version: association.VersionV1, PlanID: plan.ID, TaskID: task.ID,
	}, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := otherStore.Close(); err != nil {
		t.Fatal(err)
	}
	workspaceDB := filepath.Join(workspaceRoot, ".ptrack", "ptrack.db")
	if err := os.Symlink(otherDB, workspaceDB); err != nil {
		t.Fatal(err)
	}

	if _, err := app.AssociateTerminalV2(1, "opaque-session", association.PointerV1{
		Version: association.VersionV1, PlanID: plan.ID, TaskID: task.ID,
	}); !errors.Is(err, errAssociationProjectMismatch) {
		t.Fatalf("cross-project association = %v", err)
	}
	manager.mu.Lock()
	if manager.association != nil {
		manager.mu.Unlock()
		t.Fatalf("cross-project association was published: %#v", manager.association)
	}
	manager.association = &preexisting
	manager.createResult = managedTerminalSession{
		SessionID: "opaque-session", ProfileID: "agent", ProfileKind: terminal.ProfileAgent,
		CWD: workspaceRoot, State: terminal.SessionRunning,
	}
	manager.mu.Unlock()
	if _, err := app.WriteTerminalMemoryV2(
		1, "opaque-session", preexisting.Revision,
		"cross-project-write", "decision", "must not cross projects", false,
	); !errors.Is(err, errAssociationProjectMismatch) {
		t.Fatalf("cross-project write-back = %v", err)
	}
	otherStore, err = store.Open(otherDB)
	if err != nil {
		t.Fatal(err)
	}
	defer otherStore.Close()
	notes, err := otherStore.ListNotes()
	if err != nil {
		t.Fatal(err)
	}
	if len(notes) != 0 {
		t.Fatalf("cross-project write persisted notes: %#v", notes)
	}
}

func seedAssociationCatalog(t *testing.T, projectRoot string) (uint64, uint64, uint64) {
	t.Helper()
	dbPath := filepath.Join(projectRoot, ".ptrack", "ptrack.db")
	s, err := store.Open(dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	plan, err := s.AddPlan("First")
	if err != nil {
		t.Fatal(err)
	}
	task, err := s.AddTask(plan.ID, "Task")
	if err != nil {
		t.Fatal(err)
	}
	other, err := s.AddPlan("Other")
	if err != nil {
		t.Fatal(err)
	}
	return plan.ID, task.ID, other.ID
}
