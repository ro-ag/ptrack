package gui

import (
	"context"
	"encoding/json"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
)

func TestSendAndAcknowledgeAgentHandoffIsExplicitBoundedAndPrivate(t *testing.T) {
	app, registry, source, target, planID, taskID := handoffDeliveryFixture(t)
	if _, err := registry.RecordEvent(source.Run.ID, source.LeaseToken, agentrun.EventObservation{
		ModelVersion: agentrun.EventModelVersion, SourceID: "summary-1", SourceSequence: 1,
		Kind: agentrun.EventSummary, Phase: agentrun.EventCompleted,
		Summary: "Bearer HANDOFF_DELIVERY_SECRET finished bounded context.",
	}); err != nil {
		t.Fatal(err)
	}
	envelope, err := app.SendAgentHandoffV2(1, source.Run.ID, target.Run.ID, 1, 1)
	if err != nil {
		t.Fatal(err)
	}
	if envelope.ID == "" || envelope.SourceRunID != source.Run.ID ||
		envelope.TargetRunID != target.Run.ID || envelope.SourceAssociation == nil ||
		envelope.SourceAssociation.PlanID != planID ||
		envelope.SourceAssociation.TaskID != taskID ||
		len(envelope.Preview.Text) > 2*1024 ||
		strings.Contains(envelope.Preview.Text, "HANDOFF_DELIVERY_SECRET") {
		t.Fatalf("envelope = %#v", envelope)
	}
	snapshot, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	if len(snapshot.AgentActivity.Handoffs.Items) != 1 {
		t.Fatalf("inbox = %#v", snapshot.AgentActivity.Handoffs)
	}
	encoded, _ := json.Marshal(snapshot.AgentActivity.Handoffs)
	for _, forbidden := range []string{"HANDOFF_DELIVERY_SECRET", `"provider"`, `"projectRoot"`, `"lifecycleRevision"`} {
		if strings.Contains(string(encoded), forbidden) {
			t.Fatalf("handoff inbox contains %q: %s", forbidden, encoded)
		}
	}
	if _, err := app.AcknowledgeAgentHandoffV2(1, envelope.ID, source.Run.ID); !errors.Is(err, ErrAgentHandoffStale) {
		t.Fatalf("wrong target acknowledgement = %v", err)
	}
	ack, err := app.AcknowledgeAgentHandoffV2(1, envelope.ID, target.Run.ID)
	if err != nil || !ack.Removed {
		t.Fatalf("acknowledgement = %#v err=%v", ack, err)
	}
}

func TestAgentHandoffInvalidatesOnRelinkExpiryAndProjectReset(t *testing.T) {
	app, _, source, target, planID, taskID := handoffDeliveryFixture(t)
	now := time.Date(2026, time.August, 10, 20, 0, 0, 0, time.UTC)
	app.workspace.handoffs = newAgentHandoffRegistry(func() time.Time { return now })
	envelope, err := app.SendAgentHandoffV2(1, source.Run.ID, target.Run.ID, 1, 1)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := app.AssociateAgentRunV2(1, source.Run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: planID, TaskID: taskID,
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := app.AcknowledgeAgentHandoffV2(1, envelope.ID, target.Run.ID); !errors.Is(err, ErrAgentHandoffStale) {
		t.Fatalf("relinked acknowledgement = %v", err)
	}

	second, err := app.SendAgentHandoffV2(1, source.Run.ID, target.Run.ID, 2, 1)
	if err != nil {
		t.Fatal(err)
	}
	now = now.Add(agentHandoffTTL)
	if _, exists := app.workspace.handoffs.get(second.ID); exists {
		t.Fatal("expired handoff remained in memory")
	}
	other := newWorkspaceContext(workspaceContextConfig{generation: 2})
	if len(other.handoffs.snapshot()) != 0 {
		t.Fatal("new project workspace inherited handoffs")
	}
}

func TestAgentHandoffRejectsSameOrInactiveTargetAndBoundsInbox(t *testing.T) {
	app, registry, source, target, _, _ := handoffDeliveryFixture(t)
	if _, err := app.SendAgentHandoffV2(1, source.Run.ID, target.Run.ID, 2, 1); !errors.Is(err, ErrAgentHandoffStale) {
		t.Fatalf("stale source association revision = %v", err)
	}
	if _, err := app.SendAgentHandoffV2(1, source.Run.ID, source.Run.ID, 1, 1); !errors.Is(err, ErrAgentHandoffSameRun) {
		t.Fatalf("same-run handoff = %v", err)
	}
	if err := registry.ExitExternal(target.Run.ID, target.LeaseToken, 0, "completed"); err != nil {
		t.Fatal(err)
	}
	if _, err := app.SendAgentHandoffV2(1, source.Run.ID, target.Run.ID, 1, 1); !errors.Is(err, ErrAgentHandoffInactive) {
		t.Fatalf("inactive target handoff = %v", err)
	}

	bounded := newAgentHandoffRegistry(nil)
	for index := 0; index < agentHandoffLimit; index++ {
		now := time.Now().Add(time.Duration(index) * time.Second)
		if err := bounded.add(agentHandoffEnvelope{
			ID: string(rune('a' + index)), CreatedAt: now, ExpiresAt: now.Add(time.Hour),
		}); err != nil {
			t.Fatal(err)
		}
	}
	if err := bounded.add(agentHandoffEnvelope{
		ID: "overflow", CreatedAt: time.Now(), ExpiresAt: time.Now().Add(time.Hour),
	}); !errors.Is(err, ErrAgentHandoffFull) {
		t.Fatalf("overflow = %v", err)
	}
}

func handoffDeliveryFixture(
	t *testing.T,
) (*App, *agentrun.Registry, agentrun.Lease, agentrun.Lease, uint64, uint64) {
	t.Helper()
	policy := agentrun.DefaultEventPrivacyPolicy()
	policy.AllowSummaries = true
	app, registry, source, planID, taskID := ownershipTestFixture(t, func(config *agentrun.Config) {
		config.EventPolicy = &policy
	})
	target, err := registry.RegisterExternal(agentrun.Registration{
		Profile: "target", Provider: "codex", CWD: app.workspace.root,
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := app.AssociateAgentRunV2(1, target.Run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: planID, TaskID: taskID,
	}); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	return app, registry, source, target, planID, taskID
}
