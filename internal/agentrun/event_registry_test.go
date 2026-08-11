package agentrun

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

func newEventRegistryForTest(t *testing.T, config Config) *Registry {
	t.Helper()
	registry := NewRegistry(config)
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	return registry
}

type enteredAwaitContext struct {
	context.Context
	entered chan struct{}
	once    sync.Once
}

func (c *enteredAwaitContext) Done() <-chan struct{} {
	c.once.Do(func() { close(c.entered) })
	return c.Context.Done()
}

func enableEventSummaries(config Config) Config {
	policy := DefaultEventPrivacyPolicy()
	policy.AllowSummaries = true
	config.EventPolicy = &policy
	return config
}

func TestRegistryRecordsNormalizedBoundedEventsForAuthenticatedRun(t *testing.T) {
	now := time.Date(2026, time.August, 10, 21, 0, 0, 0, time.UTC)
	registry := newEventRegistryForTest(t, enableEventSummaries(Config{ProjectRoot: "/project", Now: func() time.Time { return now }}))
	lease, err := registry.RegisterExternal(Registration{
		Profile: "agent-codex", Provider: "codex", CWD: "/project",
	})
	if err != nil {
		t.Fatal(err)
	}
	observation := EventObservation{
		ModelVersion:   EventModelVersion,
		SourceID:       "summary-1",
		SourceSequence: 7,
		Kind:           EventSummary,
		Phase:          EventCompleted,
		Summary:        "done token=RAW_SECRET_CANARY",
	}
	event, err := registry.RecordEvent(lease.Run.ID, lease.LeaseToken, observation)
	if err != nil {
		t.Fatal(err)
	}
	if event.RunID != lease.Run.ID || event.Provider != "codex" ||
		event.HostSequence != 1 || event.SourceSequence != 7 || event.ID == "" {
		t.Fatalf("host-stamped event = %#v", event)
	}
	if strings.Contains(event.Summary, "RAW_SECRET_CANARY") {
		t.Fatalf("event retained secret: %#v", event)
	}

	events, total, err := registry.EventSnapshot(lease.Run.ID, 1)
	if err != nil || total != 1 || len(events) != 1 {
		t.Fatalf("event snapshot = %#v total=%d err=%v", events, total, err)
	}
	events[0].Paths = append(events[0].Paths, "mutated")
	again, _, _ := registry.EventSnapshot(lease.Run.ID, 1)
	if len(again[0].Paths) != 0 {
		t.Fatal("event snapshot aliases registry state")
	}

	observation.SourceID = "summary-2"
	observation.SourceSequence = 6
	if _, err := registry.RecordEvent(lease.Run.ID, lease.LeaseToken, observation); !errors.Is(err, ErrEventOrder) {
		t.Fatalf("out-of-order event error = %v", err)
	}
	if _, err := registry.RecordEvent(lease.Run.ID, "wrong-token", observation); !errors.Is(err, ErrInvalidLease) {
		t.Fatalf("wrong-token event error = %v", err)
	}
	observation.SourceID = "summary-1"
	observation.SourceSequence = 8
	if _, err := registry.RecordEvent(lease.Run.ID, lease.LeaseToken, observation); !errors.Is(err, ErrEventOrder) {
		t.Fatalf("duplicate source identity error = %v", err)
	}
}

func TestRegistryPersistsOnlyAdapterRecognizedNotificationKind(t *testing.T) {
	directory := t.TempDir()
	statePath := filepath.Join(directory, "agent-runs.json")
	config := Config{ProjectRoot: directory, StatePath: statePath}
	first := NewRegistry(config)
	lease, err := first.RegisterExternal(Registration{
		Profile: "wrapper", Provider: "codex", CWD: directory,
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := first.RecordEvent(lease.Run.ID, lease.LeaseToken, EventObservation{
		ModelVersion: EventModelVersion, SourceID: "forged-1", SourceSequence: 1,
		Kind: EventLifecycle, Phase: EventWaiting,
		Notification: NotificationApprovalRequested,
	}); err == nil {
		t.Fatal("direct observation self-asserted an approval request")
	}
	recorded, err := first.RecordProviderEvent(lease.Run.ID, lease.LeaseToken, ProviderEvent{
		ModelVersion: ProviderEventModelVersion, ID: "approval-1", Sequence: 1,
		Type: "PermissionRequest", Summary: "QUESTION_SECRET_CANARY",
	})
	if err != nil {
		t.Fatal(err)
	}
	if recorded.Notification != NotificationApprovalRequested || recorded.Summary != "" {
		t.Fatalf("recorded notification = %#v", recorded)
	}
	if err := first.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}
	contents, err := os.ReadFile(statePath)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(contents), "QUESTION_SECRET_CANARY") {
		t.Fatal("notification history retained provider content")
	}
	second := newEventRegistryForTest(t, config)
	events, total, err := second.EventSnapshot(lease.Run.ID, 10)
	if err != nil || total != 1 || len(events) != 1 ||
		events[0].Notification != NotificationApprovalRequested {
		t.Fatalf("restored notifications = %#v total=%d err=%v", events, total, err)
	}
}

func TestRegistryRestoresSanitizedEventsAndRejectsReplayedSequence(t *testing.T) {
	directory := t.TempDir()
	statePath := filepath.Join(directory, "agent-runs.json")
	now := time.Date(2026, time.August, 10, 22, 0, 0, 0, time.UTC)
	config := Config{
		ProjectRoot: directory,
		StatePath:   statePath,
		Now:         func() time.Time { return now },
	}
	config = enableEventSummaries(config)
	first := NewRegistry(config)
	lease, err := first.RegisterExternal(Registration{
		Profile: "wrapper", Provider: "codex", CWD: directory,
	})
	if err != nil {
		t.Fatal(err)
	}
	recorded, err := first.RecordEvent(lease.Run.ID, lease.LeaseToken, EventObservation{
		ModelVersion: EventModelVersion, SourceID: "summary-1", SourceSequence: 9,
		Kind: EventSummary, Phase: EventCompleted,
		Summary: "done token=RESTART_SECRET_CANARY",
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := first.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}

	second := newEventRegistryForTest(t, config)
	events, total, err := second.EventSnapshot(lease.Run.ID, 10)
	if err != nil || total != 1 || len(events) != 1 {
		t.Fatalf("restored events = %#v total=%d err=%v", events, total, err)
	}
	if events[0].ID != recorded.ID || strings.Contains(events[0].Summary, "RESTART_SECRET_CANARY") {
		t.Fatalf("restored event = %#v", events[0])
	}
	_, err = second.RecordEvent(lease.Run.ID, lease.LeaseToken, EventObservation{
		ModelVersion: EventModelVersion, SourceID: "summary-2", SourceSequence: 9,
		Kind: EventSummary, Phase: EventCompleted, Summary: "replay",
	})
	if !errors.Is(err, ErrEventOrder) {
		t.Fatalf("replayed source sequence error = %v", err)
	}
}

func TestRegistryRejectsEventsAfterLeaseExpiryBeforeContentValidation(t *testing.T) {
	now := time.Date(2026, time.August, 10, 23, 0, 0, 0, time.UTC)
	clock := &fakeClock{now: now}
	registry := newEventRegistryForTest(t, Config{
		ProjectRoot: "/project", LeaseDuration: time.Second,
		Now: clock.Now, NewTicker: clock.NewTicker,
	})
	lease, err := registry.RegisterExternal(Registration{
		Profile: "wrapper", Provider: "codex", CWD: "/project",
	})
	if err != nil {
		t.Fatal(err)
	}
	clock.Advance(2 * time.Second)
	registry.SweepExpired()
	_, err = registry.RecordEvent(lease.Run.ID, lease.LeaseToken, EventObservation{
		ModelVersion: 999,
	})
	if !errors.Is(err, ErrInvalidLease) {
		t.Fatalf("stale-run event error = %v", err)
	}
}

func TestRegistryNeverPersistsRawExitResultOrEventSecrets(t *testing.T) {
	directory := t.TempDir()
	statePath := filepath.Join(directory, "agent-runs.json")
	registry := newEventRegistryForTest(t, enableEventSummaries(Config{ProjectRoot: directory, StatePath: statePath}))
	lease, err := registry.RegisterExternal(Registration{
		Profile: "agent-claude", Provider: "claude", CWD: directory,
	})
	if err != nil {
		t.Fatal(err)
	}
	_, err = registry.RecordEvent(lease.Run.ID, lease.LeaseToken, EventObservation{
		ModelVersion:   EventModelVersion,
		SourceID:       "summary-1",
		SourceSequence: 1,
		Kind:           EventSummary,
		Phase:          EventCompleted,
		Summary:        "Authorization=EVENT_SECRET_CANARY",
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := registry.ExitExternal(lease.Run.ID, lease.LeaseToken, 9, "EXIT_RESULT_SECRET_CANARY"); err != nil {
		t.Fatal(err)
	}
	contents, err := os.ReadFile(statePath)
	if err != nil {
		t.Fatal(err)
	}
	for _, secret := range []string{"EVENT_SECRET_CANARY", "EXIT_RESULT_SECRET_CANARY"} {
		if strings.Contains(string(contents), secret) {
			t.Fatalf("history retained secret canary %q", secret)
		}
	}
	var state persistedRegistryState
	if err := json.Unmarshal(contents, &state); err != nil {
		t.Fatal(err)
	}
	if len(state.Runs) != 1 || state.Runs[0].Run.Exit == nil ||
		state.Runs[0].Run.Exit.Result != "failed" || len(state.Runs[0].Events) != 1 {
		t.Fatalf("persisted sanitized record = %#v", state.Runs)
	}
}

func TestRegistrySweepDurablyPrunesExpiredEvents(t *testing.T) {
	directory := t.TempDir()
	statePath := filepath.Join(directory, "agent-runs.json")
	now := time.Date(2026, time.August, 10, 10, 0, 0, 0, time.UTC)
	clock := &fakeClock{now: now}
	policy := DefaultEventPrivacyPolicy()
	policy.AllowSummaries = true
	policy.RetainFor = time.Hour
	registry := newEventRegistryForTest(t, Config{
		ProjectRoot: directory, StatePath: statePath,
		LeaseDuration: 24 * time.Hour, Now: clock.Now,
		NewTicker: clock.NewTicker, EventPolicy: &policy,
	})
	lease, err := registry.RegisterExternal(Registration{
		Profile: "wrapper", Provider: "codex", CWD: directory,
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := registry.RecordEvent(lease.Run.ID, lease.LeaseToken, EventObservation{
		ModelVersion: EventModelVersion, SourceID: "summary-1", SourceSequence: 1,
		Kind: EventSummary, Phase: EventCompleted, Summary: "safe final summary",
	}); err != nil {
		t.Fatal(err)
	}
	clock.Advance(2 * time.Hour)
	registry.SweepExpired()
	contents, err := os.ReadFile(statePath)
	if err != nil {
		t.Fatal(err)
	}
	var state persistedRegistryState
	if err := json.Unmarshal(contents, &state); err != nil {
		t.Fatal(err)
	}
	if len(state.Runs) != 1 || len(state.Runs[0].Events) != 0 {
		t.Fatalf("expired events remained in history: %#v", state.Runs)
	}
}

func TestRegistryRestartSweepDurablyRewritesExpiredEvents(t *testing.T) {
	directory := t.TempDir()
	statePath := filepath.Join(directory, "agent-runs.json")
	clock := &fakeClock{now: time.Date(2026, time.August, 10, 10, 0, 0, 0, time.UTC)}
	policy := DefaultEventPrivacyPolicy()
	policy.AllowSummaries = true
	policy.RetainFor = time.Hour
	config := Config{
		ProjectRoot: directory, StatePath: statePath, LeaseDuration: 24 * time.Hour,
		Now: clock.Now, NewTicker: clock.NewTicker, EventPolicy: &policy,
	}
	first := NewRegistry(config)
	lease, err := first.RegisterExternal(Registration{
		Profile: "wrapper", Provider: "codex", CWD: directory,
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := first.RecordEvent(lease.Run.ID, lease.LeaseToken, EventObservation{
		ModelVersion: EventModelVersion, SourceID: "summary-1", SourceSequence: 1,
		Kind: EventSummary, Phase: EventCompleted, Summary: "safe final summary",
	}); err != nil {
		t.Fatal(err)
	}
	if err := first.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}

	clock.Advance(2 * time.Hour)
	second := newEventRegistryForTest(t, config)
	second.SweepExpired()
	contents, err := os.ReadFile(statePath)
	if err != nil {
		t.Fatal(err)
	}
	var state persistedRegistryState
	if err := json.Unmarshal(contents, &state); err != nil {
		t.Fatal(err)
	}
	if len(state.Runs) != 1 || len(state.Runs[0].Events) != 0 {
		t.Fatalf("restart left expired events in history: %#v", state.Runs)
	}
}

func TestTerminalRevocationClosesEveryMatchingLaunchedToken(t *testing.T) {
	registry := newEventRegistryForTest(t, Config{ProjectRoot: "/project"})
	tokens := make([]string, 0, 2)
	for index := 0; index < 2; index++ {
		token, err := registry.IssueLaunchedEventToken()
		if err != nil {
			t.Fatal(err)
		}
		run, err := registry.RegisterLaunched(Registration{
			Profile: "agent-codex", Provider: "codex", PID: index + 1,
			TerminalID: "terminal-shared", CWD: "/project",
		})
		if err != nil {
			t.Fatal(err)
		}
		if err := registry.BindLaunchedEventToken(token, run.ID); err != nil {
			t.Fatal(err)
		}
		tokens = append(tokens, token)
	}
	if !registry.RevokeLaunchedEventTokenForTerminal("terminal-shared") {
		t.Fatal("terminal revocation did not report bound tokens")
	}
	for _, token := range tokens {
		if err := registry.AuthenticateLaunchedEventToken(token); !errors.Is(err, ErrInvalidEventToken) {
			t.Fatalf("matching token remained live: %v", err)
		}
	}
}

func TestPendingLaunchedTokenWaitsForHostBinding(t *testing.T) {
	registry := newEventRegistryForTest(t, Config{ProjectRoot: "/project"})
	token, err := registry.IssueLaunchedEventToken()
	if err != nil {
		t.Fatal(err)
	}
	deadlineContext, cancelWait := context.WithTimeout(context.Background(), time.Second)
	defer cancelWait()
	waitContext := &enteredAwaitContext{
		Context: deadlineContext,
		entered: make(chan struct{}),
	}
	waited := make(chan error, 1)
	go func() {
		waited <- registry.AwaitLaunchedEventToken(waitContext, token)
	}()
	<-waitContext.entered
	select {
	case err := <-waited:
		t.Fatalf("pending token returned before host binding: %v", err)
	default:
	}
	run, err := registry.RegisterLaunched(Registration{
		Profile: "agent-codex", Provider: "codex", PID: 1,
		TerminalID: "terminal-starting", CWD: "/project",
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := registry.BindLaunchedEventToken(token, run.ID); err != nil {
		t.Fatal(err)
	}
	if err := <-waited; err != nil {
		t.Fatalf("pending token did not become usable after binding: %v", err)
	}
	registry.RevokeLaunchedEventToken(token)
	if err := registry.AwaitLaunchedEventToken(context.Background(), token); !errors.Is(err, ErrInvalidEventToken) {
		t.Fatalf("revoked token wait error = %v", err)
	}
	timeoutToken, err := registry.IssueLaunchedEventToken()
	if err != nil {
		t.Fatal(err)
	}
	timedOut, cancelTimeout := context.WithCancel(context.Background())
	cancelTimeout()
	if err := registry.AwaitLaunchedEventToken(timedOut, timeoutToken); !errors.Is(err, ErrInvalidEventToken) {
		t.Fatalf("timed-out pending token error = %v", err)
	}
	registry.RevokeLaunchedEventToken(timeoutToken)
}

func TestLaunchedEventTokenIsNeverPersisted(t *testing.T) {
	directory := t.TempDir()
	statePath := filepath.Join(directory, "agent-runs.json")
	registry := newEventRegistryForTest(t, Config{
		ProjectRoot: directory, StatePath: statePath,
	})
	token, err := registry.IssueLaunchedEventToken()
	if err != nil {
		t.Fatal(err)
	}
	run, err := registry.RegisterLaunched(Registration{
		Profile: "agent-codex", Provider: "codex", PID: os.Getpid(),
		TerminalID: "terminal-1", CWD: directory,
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := registry.BindLaunchedEventToken(token, run.ID); err != nil {
		t.Fatal(err)
	}
	if _, err := registry.RecordLaunchedProviderEvent(token, ProviderEvent{
		ModelVersion: ProviderEventModelVersion, ID: "turn-1", Sequence: 1,
		Type: "turn.started",
	}); err != nil {
		t.Fatal(err)
	}
	contents, err := os.ReadFile(statePath)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(contents), token) || strings.Contains(string(contents), "eventToken") {
		t.Fatalf("launched event credential was persisted: %s", contents)
	}
}

func TestRegistryRestoreRejectsFutureObservedEvents(t *testing.T) {
	directory := t.TempDir()
	statePath := filepath.Join(directory, "agent-runs.json")
	now := time.Date(2026, time.August, 10, 10, 0, 0, 0, time.UTC)
	run := Run{
		ID: "run-1", Profile: "wrapper", Provider: "codex",
		ProjectRoot: directory, CWD: directory, Kind: RegistrationExternal,
		State: StateRunning, ProcessState: ProcessUnknown, LeaseState: LeaseActive,
		StartedAt: now, LastActivityAt: now, LastHeartbeatAt: now,
	}
	state := persistedRegistryState{
		Version: persistedStateVersion,
		Runs: []persistedRecord{{
			Run: run, LeaseToken: "lease",
			Events: []Event{{
				ModelVersion: EventModelVersion, ID: "host-1", RunID: run.ID,
				Provider: run.Provider, SourceID: "event-1", SourceSequence: 1,
				HostSequence: 1, Kind: EventLifecycle, Phase: EventProgress,
				OccurredAt: now.Add(time.Hour), ObservedAt: now.Add(time.Hour),
				Correlation: EventCorrelation{ProjectRoot: directory},
			}},
		}},
	}
	contents, err := json.Marshal(state)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(statePath, contents, 0o600); err != nil {
		t.Fatal(err)
	}
	registry := newEventRegistryForTest(t, Config{
		ProjectRoot: directory, StatePath: statePath, Now: func() time.Time { return now },
	})
	events, total, err := registry.EventSnapshot(run.ID, 10)
	if err != nil || total != 0 || len(events) != 0 {
		t.Fatalf("future observed events = %#v total=%d err=%v", events, total, err)
	}
}

func TestFutureRunHistoryIsNeverClobbered(t *testing.T) {
	directory := t.TempDir()
	statePath := filepath.Join(directory, "agent-runs.json")
	future := []byte(`{"version":999,"savedAt":"2026-08-10T00:00:00Z","runs":[],"futureCanary":"PRESERVE"}`)
	if err := os.WriteFile(statePath, future, 0o600); err != nil {
		t.Fatal(err)
	}
	registry := newEventRegistryForTest(t, Config{ProjectRoot: directory, StatePath: statePath})
	if _, err := registry.RegisterExternal(Registration{
		Profile: "future", Provider: "future", CWD: directory,
	}); err != nil {
		t.Fatal(err)
	}
	contents, err := os.ReadFile(statePath)
	if err != nil {
		t.Fatal(err)
	}
	if string(contents) != string(future) {
		t.Fatalf("future history was clobbered: %s", contents)
	}
}

func TestRestoredHistorySanitizesLegacyRawExitResult(t *testing.T) {
	directory := t.TempDir()
	statePath := filepath.Join(directory, "agent-runs.json")
	state := persistedRegistryState{
		Version: 2,
		Runs: []persistedRecord{{Run: Run{
			ID: "legacy", Profile: "legacy", Provider: "legacy",
			ProjectRoot: directory, CWD: directory, Kind: RegistrationExternal,
			State: StateExited, ProcessState: ProcessExited, LeaseState: LeaseExpired,
			Exit: &Exit{Code: 2, Result: "LEGACY_EXIT_SECRET_CANARY", OccurredAt: time.Now()},
		}}},
	}
	contents, err := json.Marshal(state)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(statePath, contents, 0o600); err != nil {
		t.Fatal(err)
	}
	registry := newEventRegistryForTest(t, Config{ProjectRoot: directory, StatePath: statePath})
	runs := registry.Snapshot(10)
	if len(runs) != 1 || runs[0].Exit == nil || runs[0].Exit.Result != "failed" {
		t.Fatalf("restored legacy exit = %#v", runs)
	}
	if err := registry.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}
	persisted, err := os.ReadFile(statePath)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(persisted), "LEGACY_EXIT_SECRET_CANARY") {
		t.Fatalf("legacy raw result survived migration: %s", persisted)
	}
}

func TestRegistryCanDisableEventCollection(t *testing.T) {
	policy := DefaultEventPrivacyPolicy()
	policy.CollectionEnabled = false
	registry := newEventRegistryForTest(t, Config{ProjectRoot: "/project", EventPolicy: &policy})
	lease, err := registry.RegisterExternal(Registration{
		Profile: "agent", Provider: "generic", CWD: "/project",
	})
	if err != nil {
		t.Fatal(err)
	}
	_, err = registry.RecordEvent(lease.Run.ID, lease.LeaseToken, EventObservation{
		ModelVersion: EventModelVersion, SourceID: "event-1", SourceSequence: 1,
		Kind: EventLifecycle, Phase: EventProgress,
	})
	if !errors.Is(err, ErrEventCollectionDisabled) {
		t.Fatalf("disabled collection error = %v", err)
	}
}
