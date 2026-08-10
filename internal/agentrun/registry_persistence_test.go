package agentrun

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/association"
)

func TestRunHistorySurvivesRestart(t *testing.T) {
	statePath := filepath.Join(t.TempDir(), "agent-runs.json")
	clock := &fakeClock{now: time.Date(2026, 8, 1, 12, 0, 0, 0, time.UTC)}

	first := NewRegistry(Config{
		ProjectRoot: "/project",
		Now:         clock.Now,
		NewTicker:   clock.NewTicker,
		StatePath:   statePath,
	})
	launched, err := first.RegisterLaunched(Registration{
		Profile: "agent-codex", Provider: "codex", PID: 4242,
		TerminalID: "terminal-1", CWD: "/project",
	})
	if err != nil {
		t.Fatalf("RegisterLaunched: %v", err)
	}
	lease, err := first.RegisterExternal(Registration{
		Profile: "wrapper", Provider: "external-test", PID: 99, CWD: "/project",
	})
	if err != nil {
		t.Fatalf("RegisterExternal: %v", err)
	}
	if err := first.Shutdown(context.Background()); err != nil {
		t.Fatalf("Shutdown: %v", err)
	}

	// The history file must be private.
	info, err := os.Stat(statePath)
	if err != nil {
		t.Fatalf("history file missing: %v", err)
	}
	if info.Mode().Perm()&0o077 != 0 {
		t.Fatalf("history file perms = %o, want private", info.Mode().Perm())
	}

	second := NewRegistry(Config{
		ProjectRoot: "/project",
		Now:         clock.Now,
		NewTicker:   clock.NewTicker,
		StatePath:   statePath,
	})
	t.Cleanup(func() { _ = second.Shutdown(context.Background()) })

	snapshot := second.Snapshot(10)
	if len(snapshot) != 2 {
		t.Fatalf("restored %d runs, want 2", len(snapshot))
	}
	byID := make(map[string]Run, len(snapshot))
	for _, run := range snapshot {
		byID[run.ID] = run
	}

	// A launched run that was interrupted by the restart is stale: its host
	// terminal died with the previous app instance.
	restoredLaunched, ok := byID[launched.ID]
	if !ok {
		t.Fatalf("launched run %s not restored", launched.ID)
	}
	if restoredLaunched.State != StateStale ||
		restoredLaunched.ProcessState != ProcessUnknown {
		t.Fatalf("restored launched run = %#v, want stale/unknown", restoredLaunched)
	}

	// An external run keeps its state and lease: a still-alive agent can
	// resume heartbeating after the app restarts.
	restoredExternal, ok := byID[lease.Run.ID]
	if !ok {
		t.Fatalf("external run %s not restored", lease.Run.ID)
	}
	if restoredExternal.State != StateRunning ||
		restoredExternal.LeaseState != LeaseActive {
		t.Fatalf("restored external run = %#v, want running/active", restoredExternal)
	}
	if err := second.Heartbeat(lease.Run.ID, lease.LeaseToken); err != nil {
		t.Fatalf("Heartbeat with restored lease: %v", err)
	}
	if err := second.Heartbeat(lease.Run.ID, "wrong-token"); !errors.Is(err, ErrInvalidLease) {
		t.Fatalf("wrong-token heartbeat = %v, want ErrInvalidLease", err)
	}
}

func TestRunHistoryRestoreMarksInterruptedLaunchedOnly(t *testing.T) {
	statePath := filepath.Join(t.TempDir(), "agent-runs.json")
	clock := &fakeClock{now: time.Now()}

	first := NewRegistry(Config{
		ProjectRoot: "/project",
		Now:         clock.Now,
		NewTicker:   clock.NewTicker,
		StatePath:   statePath,
	})
	run, err := first.RegisterLaunched(Registration{
		Profile: "agent-codex", Provider: "codex", PID: 7,
		TerminalID: "terminal-1", CWD: "/project",
	})
	if err != nil {
		t.Fatalf("RegisterLaunched: %v", err)
	}
	if !first.RecordTerminalExit("terminal-1", 3, "failed") {
		t.Fatal("RecordTerminalExit did not find run")
	}
	if err := first.Shutdown(context.Background()); err != nil {
		t.Fatalf("Shutdown: %v", err)
	}

	second := NewRegistry(Config{
		ProjectRoot: "/project",
		Now:         clock.Now,
		NewTicker:   clock.NewTicker,
		StatePath:   statePath,
	})
	t.Cleanup(func() { _ = second.Shutdown(context.Background()) })
	restored := second.Snapshot(1)
	if len(restored) != 1 || restored[0].ID != run.ID {
		t.Fatalf("restored = %#v", restored)
	}
	// An already-exited run must not be rewritten as stale on restore.
	if restored[0].State != StateExited || restored[0].Exit == nil ||
		restored[0].Exit.Code != 3 {
		t.Fatalf("restored exited run = %#v", restored[0])
	}
}

func TestRunHistoryCorruptFileStartsEmpty(t *testing.T) {
	statePath := filepath.Join(t.TempDir(), "agent-runs.json")
	if err := os.WriteFile(statePath, []byte("{not json"), 0o600); err != nil {
		t.Fatal(err)
	}
	registry := NewRegistry(Config{
		ProjectRoot: "/project",
		Now:         time.Now,
		StatePath:   statePath,
	})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	if got := registry.Snapshot(10); len(got) != 0 {
		t.Fatalf("restored %d runs from corrupt history, want 0", len(got))
	}
}

func TestRunHistoryBoundedByMaxRecords(t *testing.T) {
	statePath := filepath.Join(t.TempDir(), "agent-runs.json")
	clock := &fakeClock{now: time.Date(2026, 8, 1, 12, 0, 0, 0, time.UTC)}

	first := NewRegistry(Config{
		ProjectRoot: "/project",
		Now:         clock.Now,
		NewTicker:   clock.NewTicker,
		MaxRecords:  4,
		StatePath:   statePath,
	})
	for range 3 {
		if _, err := first.RegisterExternal(Registration{
			Profile: "wrapper", Provider: "external-test", PID: 1, CWD: "/project",
		}); err != nil {
			t.Fatalf("RegisterExternal: %v", err)
		}
		clock.Advance(time.Second)
	}
	if err := first.Shutdown(context.Background()); err != nil {
		t.Fatalf("Shutdown: %v", err)
	}

	contents, err := os.ReadFile(statePath)
	if err != nil {
		t.Fatalf("read history: %v", err)
	}
	var state persistedRegistryState
	if err := json.Unmarshal(contents, &state); err != nil {
		t.Fatalf("decode history: %v", err)
	}
	if state.Version != persistedStateVersion {
		t.Fatalf("history version = %d, want %d", state.Version, persistedStateVersion)
	}
	if len(state.Runs) != 3 {
		t.Fatalf("history holds %d runs, want 3", len(state.Runs))
	}
	// Persisted order is most-recently-active first.
	for i := 1; i < len(state.Runs); i++ {
		if state.Runs[i-1].Run.LastActivityAt.Before(state.Runs[i].Run.LastActivityAt) {
			t.Fatal("history is not sorted by last activity descending")
		}
	}
}

func TestRunHistoryMigratesV1DetachedAndNeverPersistsLiveAssociation(t *testing.T) {
	projectRoot := t.TempDir()
	statePath := filepath.Join(t.TempDir(), "agent-runs.json")
	legacy := []byte(`{
  "version": 1,
  "savedAt": "2026-08-01T12:00:00Z",
  "runs": [{
    "run": {
      "id": "legacy-run",
      "profile": "wrapper",
      "provider": "external",
      "projectRoot": "` + projectRoot + `",
      "planId": 2,
      "taskId": 9,
      "cwd": "` + projectRoot + `",
      "state": "running",
      "processState": "unknown",
      "leaseState": "active",
      "registrationKind": "external",
      "lastActivityAt": "2026-08-01T12:00:00Z",
      "lastHeartbeatAt": "2026-08-01T12:00:00Z"
    },
    "leaseToken": "legacy-lease"
  }]
}`)
	if err := os.WriteFile(statePath, legacy, 0o600); err != nil {
		t.Fatal(err)
	}
	registry := NewRegistry(Config{ProjectRoot: projectRoot, StatePath: statePath})
	snapshot := registry.Snapshot(1)
	if len(snapshot) != 1 || snapshot[0].ID != "legacy-run" || snapshot[0].Association != nil {
		t.Fatalf("migrated snapshot = %#v", snapshot)
	}
	host, err := association.NewHost(projectRoot, 8, registryAssociationCatalog{})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := registry.Associate("legacy-run", host, association.PointerV1{
		Version: association.VersionV1, PlanID: 2, TaskID: 9,
	}); err != nil {
		t.Fatal(err)
	}
	if err := registry.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}
	persisted, err := os.ReadFile(statePath)
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(persisted, []byte("association")) ||
		bytes.Contains(persisted, []byte("planId")) ||
		bytes.Contains(persisted, []byte("taskId")) {
		t.Fatalf("history persisted live association: %s", persisted)
	}
	var state persistedRegistryState
	if err := json.Unmarshal(persisted, &state); err != nil {
		t.Fatal(err)
	}
	if state.Version != 2 {
		t.Fatalf("migrated history version = %d, want 2", state.Version)
	}
}

func TestRunHistoryRejectsRecordsOutsideCurrentProject(t *testing.T) {
	projectRoot := t.TempDir()
	statePath := filepath.Join(t.TempDir(), "agent-runs.json")
	state := persistedRegistryState{
		Version: persistedStateVersion,
		Runs: []persistedRecord{
			{Run: Run{ID: "wrong-root", ProjectRoot: t.TempDir(), CWD: projectRoot}},
			{Run: Run{ID: "wrong-cwd", ProjectRoot: projectRoot, CWD: t.TempDir()}},
		},
	}
	contents, err := json.Marshal(state)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(statePath, contents, 0o600); err != nil {
		t.Fatal(err)
	}
	registry := NewRegistry(Config{ProjectRoot: projectRoot, StatePath: statePath})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	if snapshot := registry.Snapshot(10); len(snapshot) != 0 {
		t.Fatalf("restored cross-project records: %#v", snapshot)
	}
}
