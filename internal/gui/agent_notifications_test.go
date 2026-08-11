package gui

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
)

func TestWorkspaceAgentNotificationsAreDeduplicatedOrderedAndAssociationFenced(t *testing.T) {
	now := time.Date(2026, time.August, 10, 20, 0, 0, 0, time.UTC)
	app, registry, lease, planID, taskID := ownershipTestFixture(t, func(config *agentrun.Config) {
		config.Now = func() time.Time { return now }
	})
	events := []struct {
		typeName string
		want     agentrun.EventNotificationKind
	}{
		{typeName: "PermissionRequest", want: agentrun.NotificationApprovalRequested},
		{typeName: "question", want: agentrun.NotificationQuestion},
		{typeName: "question", want: agentrun.NotificationQuestion},
		{typeName: "turn.failed", want: agentrun.NotificationFailure},
		{typeName: "sessionend", want: agentrun.NotificationCompletion},
	}
	for index, event := range events {
		now = now.Add(time.Second)
		recorded, err := registry.RecordProviderEvent(lease.Run.ID, lease.LeaseToken, agentrun.ProviderEvent{
			ModelVersion: agentrun.ProviderEventModelVersion,
			ID:           "notification-" + string(rune('a'+index)), Sequence: uint64(index + 1),
			Type: event.typeName, Summary: "RAW_QUESTION_PROMPT_CANARY",
		})
		if err != nil || recorded.Notification != event.want {
			t.Fatalf("record %d = %#v err=%v", index, recorded, err)
		}
	}
	snapshot, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	notifications := snapshot.AgentActivity.Notifications
	if len(notifications) != 4 || notifications[0].Kind != agentrun.NotificationCompletion ||
		notifications[1].Kind != agentrun.NotificationFailure ||
		notifications[2].Kind != agentrun.NotificationQuestion ||
		notifications[3].Kind != agentrun.NotificationApprovalRequested {
		t.Fatalf("notifications = %#v", notifications)
	}
	encoded, err := json.Marshal(notifications)
	if err != nil {
		t.Fatal(err)
	}
	for _, forbidden := range []string{"RAW_QUESTION_PROMPT_CANARY", `"summary"`, `"subject"`, `"paths"`} {
		if strings.Contains(string(encoded), forbidden) {
			t.Fatalf("notification DTO contains %q: %s", forbidden, encoded)
		}
	}

	if _, err := app.AssociateAgentRunV2(1, lease.Run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: planID, TaskID: taskID,
	}); err != nil {
		t.Fatal(err)
	}
	refreshed, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	if len(refreshed.AgentActivity.Notifications) != 0 {
		t.Fatalf("old-association notifications survived relink: %#v", refreshed.AgentActivity.Notifications)
	}
}

func TestRevivedAgentLifecycleDoesNotReusePriorEvents(t *testing.T) {
	now := time.Date(2026, time.August, 10, 21, 0, 0, 0, time.UTC)
	app, registry, lease, _, _ := ownershipTestFixture(t, func(config *agentrun.Config) {
		config.Now = func() time.Time { return now }
		config.LeaseDuration = time.Second
	})
	if _, err := registry.RecordProviderEvent(lease.Run.ID, lease.LeaseToken, agentrun.ProviderEvent{
		ModelVersion: agentrun.ProviderEventModelVersion,
		ID:           "prior-question", Sequence: 1, Type: "question",
	}); err != nil {
		t.Fatal(err)
	}
	now = now.Add(2 * time.Second)
	if _, err := app.GetWorkspaceSnapshot(1, 0); err != nil {
		t.Fatal(err)
	}
	if err := registry.Heartbeat(lease.Run.ID, lease.LeaseToken); err != nil {
		t.Fatal(err)
	}
	snapshot, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	if len(snapshot.AgentActivity.Notifications) != 0 ||
		len(snapshot.AgentActivity.Items) != 1 ||
		snapshot.AgentActivity.Items[0].State != agentrun.ActivityRunning {
		t.Fatalf("revived activity reused old lifecycle evidence: %#v", snapshot.AgentActivity)
	}
	run, events, _, _, err := registry.IntelligenceSnapshot(lease.Run.ID, 32)
	if err != nil {
		t.Fatal(err)
	}
	preview := agentrun.BuildHandoffPreview(run, events)
	if len(preview.IncludedEventIDs) != 0 {
		t.Fatalf("revived handoff reused old events: %#v", preview)
	}
}

func TestCanonicalLifecycleEventSurfacesWorkspaceNotification(t *testing.T) {
	app, registry, lease, _, _ := ownershipTestFixture(t, nil)
	if _, err := registry.RecordProviderEvent(lease.Run.ID, lease.LeaseToken, agentrun.ProviderEvent{
		ModelVersion: agentrun.ProviderEventModelVersion,
		ID:           "canonical-completion", Sequence: 1, Type: "lifecycle.completed",
	}); err != nil {
		t.Fatal(err)
	}
	snapshot, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	if len(snapshot.AgentActivity.Notifications) != 1 ||
		snapshot.AgentActivity.Notifications[0].Kind != agentrun.NotificationCompletion {
		t.Fatalf("canonical notifications = %#v", snapshot.AgentActivity.Notifications)
	}
}
