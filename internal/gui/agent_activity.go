package gui

import (
	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/gitinfo"
)

// AgentActivityCounts describes only the bounded rows in Items. Bounds keeps
// omitted history explicit instead of pretending the counts are exhaustive.
type AgentActivityCounts struct {
	Running   int `json:"running"`
	Waiting   int `json:"waiting"`
	Blocked   int `json:"blocked"`
	Completed int `json:"completed"`
	Failed    int `json:"failed"`
	Stale     int `json:"stale"`
	Unknown   int `json:"unknown"`
}

// AgentActivity is a content-free, presentation-safe projection of one run.
// A terminal-backed run remains one agent row; its corresponding terminal is
// metadata, never a second worker.
type AgentActivity struct {
	RunID                 string                          `json:"runId"`
	State                 agentrun.ActivityState          `json:"state"`
	RegistrationKind      agentrun.RegistrationKind       `json:"registrationKind"`
	TerminalBacked        bool                            `json:"terminalBacked"`
	TerminalPresent       bool                            `json:"terminalPresent"`
	CorrespondingTerminal bool                            `json:"correspondingTerminal"`
	Live                  bool                            `json:"live"`
	Association           *RuntimeAssociation             `json:"association,omitempty"`
	Confidence            agentrun.IntelligenceConfidence `json:"confidence,omitempty"`
	EvidenceCount         int                             `json:"evidenceCount"`
	EventCount            int                             `json:"eventCount"`
	LastEventAt           string                          `json:"lastEventAt,omitempty"`
	Ownership             *AgentTaskOwnership             `json:"ownership,omitempty"`
	Worktree              *AgentWorktreeAssociation       `json:"worktree,omitempty"`
}

type AgentActivityConflict struct {
	PlanID     uint64          `json:"planId"`
	TaskID     uint64          `json:"taskId"`
	AgentCount int             `json:"agentCount"`
	OwnerCount int             `json:"ownerCount"`
	RunIDs     []string        `json:"runIds"`
	Bounds     BoundedSnapshot `json:"bounds"`
}

type AgentActivitySnapshot struct {
	State                     SnapshotState              `json:"state"`
	Items                     []AgentActivity            `json:"items"`
	Counts                    AgentActivityCounts        `json:"counts"`
	Bounds                    BoundedSnapshot            `json:"bounds"`
	Conflicts                 []AgentActivityConflict    `json:"conflicts"`
	ConflictBounds            BoundedSnapshot            `json:"conflictBounds"`
	AnalysisIncomplete        bool                       `json:"analysisIncomplete"`
	Notifications             []AgentNotification        `json:"notifications"`
	NotificationBounds        BoundedSnapshot            `json:"notificationBounds"`
	NotificationsIncomplete   bool                       `json:"notificationsIncomplete"`
	Handoffs                  AgentHandoffInbox          `json:"handoffs"`
	Worktrees                 []gitinfo.ExistingWorktree `json:"worktrees"`
	WorktreeBounds            gitinfo.WorktreeBounds     `json:"worktreeBounds"`
	WorktreesIncomplete       bool                       `json:"worktreesIncomplete"`
	Workflows                 AgentWorkflowInbox         `json:"workflows"`
	WorkflowTargets           []string                   `json:"workflowTargets"`
	WorkflowTargetsIncomplete bool                       `json:"workflowTargetsIncomplete"`
}

func buildAgentActivitySnapshot(projection runtimeProjection) AgentActivitySnapshot {
	activity := AgentActivitySnapshot{
		State:         SnapshotReady,
		Items:         make([]AgentActivity, 0, len(projection.agents)),
		Conflicts:     []AgentActivityConflict{},
		Notifications: []AgentNotification{},
		Bounds:        projection.agentBounds,
	}
	for _, run := range projection.agents {
		item := AgentActivity{
			RunID:                 run.RunID,
			State:                 run.ActivityState,
			RegistrationKind:      run.RegistrationKind,
			TerminalBacked:        run.TerminalBacked,
			TerminalPresent:       run.TerminalPresent,
			CorrespondingTerminal: run.CorrespondingTerminal,
			Live:                  run.Live,
			Association:           cloneRuntimeAssociation(run.Association),
		}
		if run.Intelligence != nil {
			item.Confidence = run.Intelligence.Confidence
			item.EvidenceCount = run.Intelligence.EvidenceCount
			item.EventCount = run.Intelligence.EventCount
			item.LastEventAt = run.Intelligence.LastEventAt
		}
		activity.Items = append(activity.Items, item)
		incrementAgentActivityCount(&activity.Counts, item.State)
	}
	return activity
}

func incrementAgentActivityCount(counts *AgentActivityCounts, state agentrun.ActivityState) {
	if counts == nil {
		return
	}
	switch state {
	case agentrun.ActivityRunning:
		counts.Running++
	case agentrun.ActivityWaiting:
		counts.Waiting++
	case agentrun.ActivityBlocked:
		counts.Blocked++
	case agentrun.ActivityCompleted:
		counts.Completed++
	case agentrun.ActivityFailed:
		counts.Failed++
	case agentrun.ActivityStale:
		counts.Stale++
	default:
		counts.Unknown++
	}
}

func cloneRuntimeAssociation(current *RuntimeAssociation) *RuntimeAssociation {
	if current == nil {
		return nil
	}
	copy := *current
	return &copy
}
