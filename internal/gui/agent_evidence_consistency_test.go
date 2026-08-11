package gui

import (
	"sync"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/gitinfo"
	"github.com/ro-ag/ptrack/internal/store"
)

type intelligenceRaceAgentResources struct {
	*workspaceAgentResources
	mu   sync.Mutex
	hook func()
}

func (r *intelligenceRaceAgentResources) setIntelligenceHook(hook func()) {
	r.mu.Lock()
	r.hook = hook
	r.mu.Unlock()
}

func (r *intelligenceRaceAgentResources) IntelligenceSnapshot(
	runID string,
	limit int,
) (agentrun.Run, []agentrun.Event, int, agentrun.RunIntelligence, error) {
	r.mu.Lock()
	hook := r.hook
	r.hook = nil
	r.mu.Unlock()
	if hook != nil {
		hook()
	}
	return r.registry.IntelligenceSnapshot(runID, limit)
}

func TestRuntimeProjectionOmitsIntelligenceFromChangedLifecycle(t *testing.T) {
	app, registry, lease, _, _ := ownershipTestFixture(t, nil)
	resources := &intelligenceRaceAgentResources{
		workspaceAgentResources: &workspaceAgentResources{registry: registry},
	}
	app.workspace.agents = resources
	resources.setIntelligenceHook(func() {
		if err := registry.ExitExternal(lease.Run.ID, lease.LeaseToken, 1, "failed"); err != nil {
			t.Fatal(err)
		}
	})
	s, err := store.Open(app.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()

	projection, err := workspaceRuntimeProjection(s, app.workspace)
	if err != nil {
		t.Fatal(err)
	}
	if !projection.agentAnalysisIncomplete || len(projection.agents) != 1 {
		t.Fatalf("projection bounds = incomplete=%v agents=%#v",
			projection.agentAnalysisIncomplete, projection.agents)
	}
	item := projection.agents[0]
	if item.Intelligence != nil || item.ActivityState != agentrun.ActivityRunning || !item.Live {
		t.Fatalf("changed lifecycle enriched exact row: %#v", item)
	}
}

func TestRuntimeProjectionDoesNotReusePriorAssociationEvidence(t *testing.T) {
	app, registry, lease, planID, taskID := ownershipTestFixture(t, nil)
	if _, err := registry.RecordProviderEvent(
		lease.Run.ID,
		lease.LeaseToken,
		agentrun.ProviderEvent{
			ModelVersion: agentrun.ProviderEventModelVersion,
			ID:           "prior-association-question", Sequence: 1, Type: "question",
		},
	); err != nil {
		t.Fatal(err)
	}
	if _, err := app.AssociateAgentRunV2(1, lease.Run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: planID, TaskID: taskID,
	}); err != nil {
		t.Fatal(err)
	}
	s, err := store.Open(app.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	projection, err := workspaceRuntimeProjection(s, app.workspace)
	if err != nil {
		t.Fatal(err)
	}
	if len(projection.agents) != 1 || projection.agents[0].Intelligence == nil {
		t.Fatalf("projection = %#v", projection.agents)
	}
	item := projection.agents[0]
	if item.ActivityState != agentrun.ActivityRunning ||
		item.Intelligence.State != agentrun.IntelligenceWorking ||
		item.Intelligence.EventCount != 0 || item.Intelligence.EvidenceCount != 1 {
		t.Fatalf("prior association evidence enriched current row: %#v", item)
	}
}

func TestNotificationsOmitEvidenceFromReassociatedSnapshot(t *testing.T) {
	app, registry, lease, planID, taskID := ownershipTestFixture(t, nil)
	resources := &intelligenceRaceAgentResources{
		workspaceAgentResources: &workspaceAgentResources{registry: registry},
	}
	app.workspace.agents = resources
	s, err := store.Open(app.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	projection, err := workspaceRuntimeProjection(s, app.workspace)
	if err != nil {
		t.Fatal(err)
	}
	host, err := workspaceAssociationHost(app.workspace, s)
	if err != nil {
		t.Fatal(err)
	}
	resources.setIntelligenceHook(func() {
		if _, err := registry.Associate(lease.Run.ID, host, association.PointerV1{
			Version: association.VersionV1, PlanID: planID, TaskID: taskID,
		}); err != nil {
			t.Fatal(err)
		}
		if _, err := registry.RecordProviderEvent(
			lease.Run.ID,
			lease.LeaseToken,
			agentrun.ProviderEvent{
				ModelVersion: agentrun.ProviderEventModelVersion,
				ID:           "reassociated-question", Sequence: 1, Type: "question",
			},
		); err != nil {
			t.Fatal(err)
		}
	})

	notifications, _, incomplete := buildAgentNotifications(app.workspace, projection)
	if !incomplete || len(notifications) != 0 {
		t.Fatalf("reassociated notifications = %#v incomplete=%v", notifications, incomplete)
	}
}

func TestDriftOmitsEvidenceFromReassociatedSnapshot(t *testing.T) {
	app, registry, lease, planID, taskID := ownershipTestFixture(t, nil)
	if _, err := app.SetAgentTaskOwnershipV2(1, lease.Run.ID, 1, true); err != nil {
		t.Fatal(err)
	}
	resources := &intelligenceRaceAgentResources{
		workspaceAgentResources: &workspaceAgentResources{registry: registry},
	}
	app.workspace.agents = resources
	s, err := store.Open(app.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	projection, err := workspaceRuntimeProjection(s, app.workspace)
	if err != nil {
		t.Fatal(err)
	}
	activity := buildAgentActivitySnapshot(projection)
	applyAgentOwnership(app.workspace, &activity, projection)
	host, err := workspaceAssociationHost(app.workspace, s)
	if err != nil {
		t.Fatal(err)
	}
	resources.setIntelligenceHook(func() {
		if _, err := registry.Associate(lease.Run.ID, host, association.PointerV1{
			Version: association.VersionV1, PlanID: planID, TaskID: taskID,
		}); err != nil {
			t.Fatal(err)
		}
		if _, err := registry.RecordEvent(
			lease.Run.ID,
			lease.LeaseToken,
			agentrun.EventObservation{
				ModelVersion: agentrun.EventModelVersion,
				SourceID:     "reassociated-drift", SourceSequence: 1,
				Kind: agentrun.EventError, Phase: agentrun.EventProgress,
				Paths: []string{"new-association.go"}, ErrorClass: "scope_mismatch",
			},
		); err != nil {
			t.Fatal(err)
		}
	})

	drift := buildDriftSnapshot(
		app.workspace,
		projection,
		activity,
		GitSnapshot{State: SnapshotReady, Snapshot: gitinfo.Snapshot{
			State: gitinfo.RepositoryNotFound,
		}},
		nil,
		time.Time{},
	)
	if !drift.Incomplete || len(drift.Findings) != 0 {
		t.Fatalf("reassociated drift = %#v", drift)
	}
}

var _ workspaceAgentRegistry = (*intelligenceRaceAgentResources)(nil)
var _ agentIntelligenceRegistry = (*intelligenceRaceAgentResources)(nil)
