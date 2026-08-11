package agentrun

import (
	"encoding/json"
	"reflect"
	"strings"
	"testing"
	"time"
)

func TestEventKindContractIsClosed(t *testing.T) {
	kinds := []EventKind{
		EventLifecycle,
		EventTool,
		EventCommand,
		EventFile,
		EventTest,
		EventCommit,
		EventError,
		EventSummary,
	}
	want := []string{
		"lifecycle", "tool", "command", "file",
		"test", "commit", "error", "summary",
	}
	if len(kinds) != len(want) {
		t.Fatalf("kind count = %d, want %d", len(kinds), len(want))
	}
	for i, kind := range kinds {
		if !kind.Valid() || string(kind) != want[i] {
			t.Fatalf("kind[%d] = %q valid=%v, want %q", i, kind, kind.Valid(), want[i])
		}
	}
	for _, invalid := range []EventKind{"", "prompt", "transcript", "reasoning", "future"} {
		if invalid.Valid() {
			t.Fatalf("unexpected valid kind %q", invalid)
		}
	}
}

func TestEventPhaseAndOutcomeContractsAreClosed(t *testing.T) {
	phases := []EventPhase{
		EventStarted,
		EventProgress,
		EventWaiting,
		EventBlocked,
		EventCompleted,
		EventFailed,
	}
	for _, phase := range phases {
		if !phase.Valid() {
			t.Fatalf("phase %q is invalid", phase)
		}
	}
	for _, invalid := range []EventPhase{"", "idle", "done", "thinking"} {
		if invalid.Valid() {
			t.Fatalf("unexpected valid phase %q", invalid)
		}
	}
	for _, outcome := range []EventOutcome{EventSucceeded, EventUnsuccessful} {
		if !outcome.Valid() {
			t.Fatalf("outcome %q is invalid", outcome)
		}
	}
	for _, invalid := range []EventOutcome{"", "unknown", "partial"} {
		if invalid.Valid() {
			t.Fatalf("unexpected valid outcome %q", invalid)
		}
	}
}

func TestEventJSONRoundTripPreservesStructuredEvidence(t *testing.T) {
	exitCode := 1
	now := time.Date(2026, time.August, 10, 18, 0, 0, 0, time.UTC)
	want := Event{
		ModelVersion:   EventModelVersion,
		ID:             "host-event-1",
		RunID:          "run-1",
		Provider:       "codex",
		SourceID:       "provider-event-9",
		SourceSequence: 9,
		HostSequence:   4,
		Kind:           EventTest,
		Phase:          EventFailed,
		Outcome:        EventUnsuccessful,
		Subject:        "go-test",
		Paths:          []string{"internal/agentrun/event_test.go"},
		CommitSHA:      "0123456789abcdef0123456789abcdef01234567",
		ExitCode:       &exitCode,
		ErrorClass:     "test_failure",
		Summary:        "One bounded provider summary.",
		OccurredAt:     now.Add(-time.Second),
		ObservedAt:     now,
	}
	encoded, err := json.Marshal(want)
	if err != nil {
		t.Fatal(err)
	}
	var got Event
	if err := json.Unmarshal(encoded, &got); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("round trip mismatch\n got: %#v\nwant: %#v", got, want)
	}
}

func TestEventContractsContainNoRawContentOrAuthorityFields(t *testing.T) {
	forbidden := []string{
		"prompt", "message", "reasoning", "chainOfThought", "transcript",
		"args", "arguments", "input", "result", "output", "stdout", "stderr",
		"environment", "headers", "body", "token", "credential", "capability",
		"projectRoot", "planId", "taskId", "terminalId", "cwd",
	}
	for _, contract := range []reflect.Type{
		reflect.TypeOf(EventObservation{}),
		reflect.TypeOf(Event{}),
	} {
		for i := 0; i < contract.NumField(); i++ {
			field := contract.Field(i)
			if field.Type.Kind() == reflect.Map {
				t.Fatalf("%s.%s uses an open-ended map", contract.Name(), field.Name)
			}
			name := strings.Split(field.Tag.Get("json"), ",")[0]
			for _, denied := range forbidden {
				if name == denied {
					t.Fatalf("%s exposes forbidden JSON field %q", contract.Name(), name)
				}
			}
		}
	}
}
