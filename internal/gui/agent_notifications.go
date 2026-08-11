package gui

import (
	"sort"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
)

const (
	agentNotificationLimit        = 64
	agentNotificationPerRunEvents = 32
)

type AgentNotification struct {
	ID             string                         `json:"id"`
	RunID          string                         `json:"runId"`
	Kind           agentrun.EventNotificationKind `json:"kind"`
	ObservedAt     string                         `json:"observedAt"`
	TerminalBacked bool                           `json:"terminalBacked"`
	Association    *RuntimeAssociation            `json:"association,omitempty"`
}

type notificationDedupKey struct {
	runID    string
	kind     agentrun.EventNotificationKind
	planID   uint64
	taskID   uint64
	revision uint64
}

func buildAgentNotifications(
	workspace *WorkspaceContext,
	projection runtimeProjection,
) ([]AgentNotification, BoundedSnapshot, bool) {
	registry, ok := workspace.agents.(agentIntelligenceRegistry)
	if !ok {
		return []AgentNotification{}, BoundedSnapshot{}, true
	}
	incomplete := projection.agentBounds.More > 0 || projection.agentAnalysisIncomplete
	latest := make(map[notificationDedupKey]AgentNotification)
	for _, projectedRun := range projection.agents {
		expected, exact := projection.exactAgentRuns[projectedRun.RunID]
		run, events, total, _, err := registry.IntelligenceSnapshot(
			projectedRun.RunID,
			agentNotificationPerRunEvents,
		)
		if err != nil || !exact || !exactAgentEvidenceSnapshot(expected, run) {
			incomplete = true
			continue
		}
		if total > len(events) {
			incomplete = true
		}
		for _, event := range events {
			if !event.Notification.Valid() || !notificationAssociationIsCurrent(
				workspace.Generation(), run, event, projectedRun.Association,
			) {
				continue
			}
			key := notificationDedupKey{runID: run.ID, kind: event.Notification}
			if projectedRun.Association != nil {
				key.planID = projectedRun.Association.PlanID
				key.taskID = projectedRun.Association.TaskID
				key.revision = projectedRun.Association.Revision
			}
			candidate := AgentNotification{
				ID: event.ID, RunID: run.ID, Kind: event.Notification,
				ObservedAt:     event.ObservedAt.UTC().Format(time.RFC3339Nano),
				TerminalBacked: projectedRun.TerminalBacked,
				Association:    cloneRuntimeAssociation(projectedRun.Association),
			}
			current, exists := latest[key]
			if !exists || candidate.ObservedAt > current.ObservedAt ||
				(candidate.ObservedAt == current.ObservedAt && candidate.ID > current.ID) {
				latest[key] = candidate
			}
		}
	}
	notifications := make([]AgentNotification, 0, len(latest))
	for _, notification := range latest {
		notifications = append(notifications, notification)
	}
	sort.Slice(notifications, func(i, j int) bool {
		if notifications[i].ObservedAt != notifications[j].ObservedAt {
			return notifications[i].ObservedAt > notifications[j].ObservedAt
		}
		if notifications[i].RunID != notifications[j].RunID {
			return notifications[i].RunID < notifications[j].RunID
		}
		return notifications[i].Kind < notifications[j].Kind
	})
	total := len(notifications)
	if len(notifications) > agentNotificationLimit {
		notifications = notifications[:agentNotificationLimit]
		incomplete = true
	}
	return notifications, snapshotBound(len(notifications), total), incomplete
}

func notificationAssociationIsCurrent(
	generation uint64,
	run agentrun.Run,
	event agentrun.Event,
	current *RuntimeAssociation,
) bool {
	correlation := event.Correlation
	if event.LifecycleRevision != run.LifecycleRevision ||
		correlation.ProjectRoot != run.ProjectRoot ||
		correlation.TerminalID != run.TerminalID {
		return false
	}
	if current == nil {
		return correlation.Generation == 0 && correlation.AssociationRevision == 0 &&
			correlation.PlanID == 0 && correlation.TaskID == 0
	}
	return correlation.Generation == generation &&
		correlation.PlanID == current.PlanID && correlation.TaskID == current.TaskID &&
		correlation.AssociationRevision == current.Revision
}
