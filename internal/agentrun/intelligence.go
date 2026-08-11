package agentrun

import (
	"sort"
	"time"
)

// IntelligenceState is a conservative interpretation of observable evidence.
// It is descriptive only and never changes a p-track task or run lifecycle.
type IntelligenceState string

const (
	IntelligenceUnknown             IntelligenceState = "unknown"
	IntelligenceWorking             IntelligenceState = "working"
	IntelligenceWaiting             IntelligenceState = "waiting"
	IntelligenceBlocked             IntelligenceState = "blocked"
	IntelligenceCompleted           IntelligenceState = "completed"
	IntelligenceFailed              IntelligenceState = "failed"
	IntelligencePotentiallyDrifting IntelligenceState = "potentiallyDrifting"
)

type IntelligenceConfidence string

const (
	ConfidenceLow    IntelligenceConfidence = "low"
	ConfidenceMedium IntelligenceConfidence = "medium"
	ConfidenceHigh   IntelligenceConfidence = "high"
)

// IntelligenceEvidence exposes only stable evidence metadata, never event
// summaries, paths, subjects, provider payloads, terminal content, or errors.
type IntelligenceEvidence struct {
	EventID    string     `json:"eventId,omitempty"`
	Kind       EventKind  `json:"kind,omitempty"`
	Phase      EventPhase `json:"phase,omitempty"`
	ObservedAt time.Time  `json:"observedAt,omitempty"`
	Reason     string     `json:"reason"`
}

type RunIntelligence struct {
	RunID       string                 `json:"runId"`
	State       IntelligenceState      `json:"state"`
	Confidence  IntelligenceConfidence `json:"confidence"`
	Evidence    []IntelligenceEvidence `json:"evidence"`
	EventCount  int                    `json:"eventCount"`
	LastEventAt time.Time              `json:"lastEventAt,omitempty"`
}

// DeriveRunIntelligence reduces ordered structured evidence without treating
// silence as waiting/completion, an operation failure as a run failure, or an
// unrelated file/commit as drift. A successful process exit alone likewise
// does not claim that the agent completed its objective.
func DeriveRunIntelligence(run Run, events []Event) RunIntelligence {
	ordered := currentRunEvents(run, events)
	result := RunIntelligence{
		RunID:      run.ID,
		State:      IntelligenceUnknown,
		Evidence:   []IntelligenceEvidence{},
		EventCount: len(ordered),
	}
	if len(ordered) > 0 {
		result.LastEventAt = ordered[len(ordered)-1].ObservedAt
	}
	if run.Exit != nil && run.Exit.Code != 0 {
		return withRunEvidence(result, IntelligenceFailed, ConfidenceHigh,
			IntelligenceEvidence{Reason: "nonzero_process_exit"})
	}

	live := runIsActive(run)
	for index := len(ordered) - 1; index >= 0; index-- {
		event := ordered[index]
		evidence := eventIntelligenceEvidence(event, "explicit_event")
		switch {
		case event.Kind == EventLifecycle && event.Phase == EventFailed:
			evidence.Reason = "explicit_lifecycle_failure"
			return withRunEvidence(result, IntelligenceFailed, ConfidenceHigh, evidence)
		case event.Kind == EventLifecycle && event.Phase == EventCompleted:
			evidence.Reason = "explicit_lifecycle_completion"
			return withRunEvidence(result, IntelligenceCompleted, ConfidenceHigh, evidence)
		case event.Kind == EventError && fatalEventClass(event.ErrorClass):
			evidence.Reason = "explicit_fatal_error"
			return withRunEvidence(result, IntelligenceFailed, ConfidenceHigh, evidence)
		case event.Phase == EventBlocked && live:
			evidence.Reason = "explicit_blocked_event"
			return withRunEvidence(result, IntelligenceBlocked, ConfidenceMedium, evidence)
		case event.Phase == EventWaiting && live:
			evidence.Reason = "explicit_waiting_event"
			return withRunEvidence(result, IntelligenceWaiting, ConfidenceMedium, evidence)
		case live && driftEventClass(event.ErrorClass) && eventCorrelationIsCurrent(run, event):
			evidence.Reason = "explicit_scope_mismatch"
			return withRunEvidence(result, IntelligencePotentiallyDrifting, ConfidenceMedium, evidence)
		case live && (event.Phase == EventStarted || event.Phase == EventProgress ||
			event.Phase == EventCompleted || event.Phase == EventFailed):
			if event.Phase == EventFailed {
				evidence.Reason = "operation_failure_while_run_live"
			} else {
				evidence.Reason = "recent_observable_activity"
			}
			return withRunEvidence(result, IntelligenceWorking, ConfidenceMedium, evidence)
		}
	}
	if live {
		return withRunEvidence(result, IntelligenceWorking, ConfidenceLow,
			IntelligenceEvidence{Reason: "live_run_without_structured_progress"})
	}
	return result
}

func currentRunEvents(run Run, events []Event) []Event {
	ordered := make([]Event, 0, len(events))
	for _, event := range events {
		if event.ModelVersion != EventModelVersion || event.RunID != run.ID ||
			event.Provider != run.Provider || event.HostSequence == 0 ||
			event.LifecycleRevision != run.LifecycleRevision {
			continue
		}
		ordered = append(ordered, cloneEvent(event))
	}
	sort.SliceStable(ordered, func(i, j int) bool {
		if ordered[i].HostSequence != ordered[j].HostSequence {
			return ordered[i].HostSequence < ordered[j].HostSequence
		}
		return ordered[i].ObservedAt.Before(ordered[j].ObservedAt)
	})
	return ordered
}

func eventIntelligenceEvidence(event Event, reason string) IntelligenceEvidence {
	return IntelligenceEvidence{
		EventID:    event.ID,
		Kind:       event.Kind,
		Phase:      event.Phase,
		ObservedAt: event.ObservedAt,
		Reason:     reason,
	}
}

func withRunEvidence(
	result RunIntelligence,
	state IntelligenceState,
	confidence IntelligenceConfidence,
	evidence IntelligenceEvidence,
) RunIntelligence {
	result.State = state
	result.Confidence = confidence
	result.Evidence = []IntelligenceEvidence{evidence}
	return result
}

func fatalEventClass(class string) bool {
	switch class {
	case "fatal", "fatal_error", "session_failure", "process_failure":
		return true
	default:
		return false
	}
}

func driftEventClass(class string) bool {
	switch class {
	case "scope_mismatch", "task_mismatch", "repository_mismatch":
		return true
	default:
		return false
	}
}

func eventCorrelationIsCurrent(run Run, event Event) bool {
	current := run.Association
	correlation := event.Correlation
	return current != nil && correlation.TaskID != 0 &&
		current.ProjectRoot == run.ProjectRoot && current.LiveID == run.ID &&
		correlation.ProjectRoot == run.ProjectRoot &&
		correlation.TerminalID == run.TerminalID &&
		correlation.PlanID == current.Target.PlanID &&
		correlation.TaskID == current.Target.TaskID &&
		correlation.Generation == current.Generation &&
		correlation.AssociationRevision == current.Revision
}
