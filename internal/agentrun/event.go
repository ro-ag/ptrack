package agentrun

import "time"

// EventModelVersion identifies the closed provider-neutral event contract.
// It is independent of both the project database and AgentRun history formats.
const EventModelVersion uint = 1

// EventKind identifies the observable work product represented by an event.
type EventKind string

const (
	EventLifecycle EventKind = "lifecycle"
	EventTool      EventKind = "tool"
	EventCommand   EventKind = "command"
	EventFile      EventKind = "file"
	EventTest      EventKind = "test"
	EventCommit    EventKind = "commit"
	EventError     EventKind = "error"
	EventSummary   EventKind = "summary"
)

// Valid reports whether the kind belongs to the version 1 contract.
func (k EventKind) Valid() bool {
	switch k {
	case EventLifecycle, EventTool, EventCommand, EventFile,
		EventTest, EventCommit, EventError, EventSummary:
		return true
	default:
		return false
	}
}

// EventPhase describes observable progress without claiming task status.
type EventPhase string

const (
	EventStarted   EventPhase = "started"
	EventProgress  EventPhase = "progress"
	EventWaiting   EventPhase = "waiting"
	EventBlocked   EventPhase = "blocked"
	EventCompleted EventPhase = "completed"
	EventFailed    EventPhase = "failed"
)

// Valid reports whether the phase belongs to the version 1 contract.
func (p EventPhase) Valid() bool {
	switch p {
	case EventStarted, EventProgress, EventWaiting, EventBlocked,
		EventCompleted, EventFailed:
		return true
	default:
		return false
	}
}

// EventOutcome is an optional, explicitly observed operation outcome.
type EventOutcome string

const (
	EventSucceeded    EventOutcome = "succeeded"
	EventUnsuccessful EventOutcome = "failed"
)

// Valid reports whether the outcome belongs to the version 1 contract.
func (o EventOutcome) Valid() bool {
	return o == EventSucceeded || o == EventUnsuccessful
}

// EventNotificationKind is a closed, content-free reason the workspace may
// need the user's attention. ApprovalRequested never represents approval or
// grants any permission.
type EventNotificationKind string

const (
	NotificationApprovalRequested EventNotificationKind = "approvalRequested"
	NotificationQuestion          EventNotificationKind = "question"
	NotificationFailure           EventNotificationKind = "failure"
	NotificationCompletion        EventNotificationKind = "completion"
)

func (k EventNotificationKind) Valid() bool {
	switch k {
	case NotificationApprovalRequested, NotificationQuestion,
		NotificationFailure, NotificationCompletion:
		return true
	default:
		return false
	}
}

// EventObservation is the narrow provider-normalized input contract. It has
// no project, plan, task, terminal, capability, or credential authority. The
// host validates and normalizes these allowlisted fields before creating an
// Event. Summary is explicitly untrusted agent-provided text; later ingestion
// policy bounds and redacts it before any retention.
//
// Subject is a short metadata label such as a tool name, command executable,
// test target, or file operation. It is never a command line, tool input,
// prompt, model response, terminal output, or hidden reasoning.
type EventObservation struct {
	ModelVersion   uint                  `json:"modelVersion"`
	SourceID       string                `json:"sourceId"`
	SourceSequence uint64                `json:"sourceSequence"`
	Kind           EventKind             `json:"kind"`
	Phase          EventPhase            `json:"phase"`
	Outcome        EventOutcome          `json:"outcome,omitempty"`
	Subject        string                `json:"subject,omitempty"`
	Paths          []string              `json:"paths,omitempty"`
	CommitSHA      string                `json:"commitSha,omitempty"`
	ExitCode       *int                  `json:"exitCode,omitempty"`
	ErrorClass     string                `json:"errorClass,omitempty"`
	Summary        string                `json:"summary,omitempty"`
	OccurredAt     time.Time             `json:"occurredAt,omitempty"`
	Notification   EventNotificationKind `json:"notification,omitempty"`
	// recognizedNotification is set only by a provider adapter or trusted
	// history restore. Direct observations cannot self-assert attention.
	recognizedNotification bool
}

// Event is the canonical host-stamped record. RunID, Provider, HostSequence,
// and ObservedAt are assigned from host-owned runtime state; a provider
// observation can never supply or override them.
type Event struct {
	ModelVersion   uint   `json:"modelVersion"`
	ID             string `json:"id"`
	RunID          string `json:"runId"`
	Provider       string `json:"provider"`
	SourceID       string `json:"sourceId"`
	SourceSequence uint64 `json:"sourceSequence"`
	HostSequence   uint64 `json:"hostSequence"`
	// LifecycleRevision is a host-owned epoch. Evidence from a stale lease
	// cannot describe a later heartbeat-revived lifecycle of the same run ID.
	LifecycleRevision uint64                `json:"lifecycleRevision"`
	Kind              EventKind             `json:"kind"`
	Phase             EventPhase            `json:"phase"`
	Outcome           EventOutcome          `json:"outcome,omitempty"`
	Subject           string                `json:"subject,omitempty"`
	Paths             []string              `json:"paths,omitempty"`
	CommitSHA         string                `json:"commitSha,omitempty"`
	ExitCode          *int                  `json:"exitCode,omitempty"`
	ErrorClass        string                `json:"errorClass,omitempty"`
	Summary           string                `json:"summary,omitempty"`
	OccurredAt        time.Time             `json:"occurredAt,omitempty"`
	ObservedAt        time.Time             `json:"observedAt"`
	Correlation       EventCorrelation      `json:"correlation"`
	Notification      EventNotificationKind `json:"notification,omitempty"`
}
