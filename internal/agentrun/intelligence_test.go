package agentrun

import (
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/association"
)

func intelligenceRun() Run {
	return Run{
		ID: "run-1", Provider: "codex", Kind: RegistrationExternal,
		State: StateRunning, ProcessState: ProcessUnknown, LeaseState: LeaseActive,
		ProjectRoot: "/project", CWD: "/project",
	}
}

func intelligenceEvent(sequence uint64, kind EventKind, phase EventPhase) Event {
	return Event{
		ModelVersion: EventModelVersion, ID: "event-" + string(rune('0'+sequence)),
		RunID: "run-1", Provider: "codex", HostSequence: sequence,
		Kind: kind, Phase: phase, ObservedAt: time.Unix(int64(sequence), 0),
		Correlation: EventCorrelation{ProjectRoot: "/project"},
	}
}

func TestDeriveRunIntelligenceDecisionTable(t *testing.T) {
	tests := []struct {
		name       string
		mutateRun  func(*Run)
		events     []Event
		state      IntelligenceState
		confidence IntelligenceConfidence
	}{
		{name: "live evidence floor is working not waiting", state: IntelligenceWorking, confidence: ConfidenceLow},
		{name: "explicit waiting", events: []Event{intelligenceEvent(1, EventLifecycle, EventWaiting)}, state: IntelligenceWaiting, confidence: ConfidenceMedium},
		{name: "explicit blocked", events: []Event{intelligenceEvent(1, EventTool, EventBlocked)}, state: IntelligenceBlocked, confidence: ConfidenceMedium},
		{name: "explicit completion", events: []Event{intelligenceEvent(1, EventLifecycle, EventCompleted)}, state: IntelligenceCompleted, confidence: ConfidenceHigh},
		{name: "explicit lifecycle failure", events: []Event{intelligenceEvent(1, EventLifecycle, EventFailed)}, state: IntelligenceFailed, confidence: ConfidenceHigh},
		{
			name: "nonzero exit", mutateRun: func(run *Run) {
				run.State = StateExited
				run.LeaseState = LeaseExpired
				run.Exit = &Exit{Code: 2, Result: "failed"}
			},
			events: []Event{intelligenceEvent(1, EventLifecycle, EventCompleted)},
			state:  IntelligenceFailed, confidence: ConfidenceHigh,
		},
		{
			name: "successful exit alone is unknown", mutateRun: func(run *Run) {
				run.State = StateExited
				run.LeaseState = LeaseExpired
				run.Exit = &Exit{Code: 0, Result: "completed"}
			},
			state: IntelligenceUnknown,
		},
		{
			name:   "test failure is not run failure",
			events: []Event{intelligenceEvent(1, EventTest, EventFailed)},
			state:  IntelligenceWorking, confidence: ConfidenceMedium,
		},
		{
			name: "stale silence is unknown", mutateRun: func(run *Run) {
				run.State = StateStale
				run.LeaseState = LeaseExpired
			},
			state: IntelligenceUnknown,
		},
		{
			name: "stale waiting evidence is not a current wait", mutateRun: func(run *Run) {
				run.State = StateStale
				run.LeaseState = LeaseExpired
			},
			events: []Event{intelligenceEvent(1, EventLifecycle, EventWaiting)},
			state:  IntelligenceUnknown,
		},
		{
			name: "stale blocked evidence is not a current block", mutateRun: func(run *Run) {
				run.State = StateStale
				run.LeaseState = LeaseExpired
			},
			events: []Event{intelligenceEvent(1, EventLifecycle, EventBlocked)},
			state:  IntelligenceUnknown,
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			run := intelligenceRun()
			if test.mutateRun != nil {
				test.mutateRun(&run)
			}
			got := DeriveRunIntelligence(run, test.events)
			if got.State != test.state || got.Confidence != test.confidence {
				t.Fatalf("intelligence = %#v, want state=%q confidence=%q", got, test.state, test.confidence)
			}
		})
	}
}

func TestRegistryIntelligenceAppliesRetentionBeforeDerivation(t *testing.T) {
	now := time.Date(2026, time.August, 10, 12, 0, 0, 0, time.UTC)
	clock := &fakeClock{now: now}
	policy := DefaultEventPrivacyPolicy()
	policy.RetainFor = time.Second
	registry := newEventRegistryForTest(t, Config{
		ProjectRoot: "/project", LeaseDuration: time.Hour,
		Now: clock.Now, NewTicker: clock.NewTicker, EventPolicy: &policy,
	})
	lease, err := registry.RegisterExternal(Registration{
		Profile: "wrapper", Provider: "codex", CWD: "/project",
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := registry.RecordEvent(lease.Run.ID, lease.LeaseToken, EventObservation{
		ModelVersion: EventModelVersion, SourceID: "wait-1", SourceSequence: 1,
		Kind: EventLifecycle, Phase: EventWaiting,
	}); err != nil {
		t.Fatal(err)
	}
	clock.Advance(2 * time.Second)
	intelligence, err := registry.Intelligence(lease.Run.ID)
	if err != nil {
		t.Fatal(err)
	}
	if intelligence.State != IntelligenceWorking ||
		intelligence.Confidence != ConfidenceLow || intelligence.EventCount != 0 {
		t.Fatalf("expired evidence intelligence = %#v", intelligence)
	}
}

func TestDeriveRunIntelligenceRequiresCurrentExplicitDriftEvidence(t *testing.T) {
	run := intelligenceRun()
	run.Association = &association.AssociationV1{
		Version: association.VersionV1, ProjectRoot: "/project", Generation: 4,
		LiveID: run.ID, Target: association.TargetV1{PlanID: 2, TaskID: 9}, Revision: 3,
	}
	drift := intelligenceEvent(1, EventError, EventProgress)
	drift.ErrorClass = "scope_mismatch"
	drift.Correlation = EventCorrelation{
		ProjectRoot: "/project", PlanID: 2, TaskID: 9,
		Generation: 4, AssociationRevision: 3,
	}
	if got := DeriveRunIntelligence(run, []Event{drift}); got.State != IntelligencePotentiallyDrifting {
		t.Fatalf("explicit drift intelligence = %#v", got)
	}

	unrelated := intelligenceEvent(2, EventFile, EventCompleted)
	unrelated.Paths = []string{"unrelated/file.go"}
	if got := DeriveRunIntelligence(run, []Event{unrelated}); got.State != IntelligenceWorking {
		t.Fatalf("unrelated file implied drift: %#v", got)
	}

	drift.Correlation.AssociationRevision = 2
	if got := DeriveRunIntelligence(run, []Event{drift}); got.State == IntelligencePotentiallyDrifting {
		t.Fatalf("stale correlation implied current drift: %#v", got)
	}
}

func TestDeriveRunIntelligenceUsesNewestDecisiveEvidence(t *testing.T) {
	blocked := intelligenceEvent(1, EventTool, EventBlocked)
	progress := intelligenceEvent(2, EventCommand, EventProgress)
	got := DeriveRunIntelligence(intelligenceRun(), []Event{progress, blocked})
	if got.State != IntelligenceWorking || len(got.Evidence) != 1 || got.Evidence[0].EventID != progress.ID {
		t.Fatalf("newest evidence intelligence = %#v", got)
	}
}
