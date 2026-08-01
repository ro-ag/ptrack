package agentrun

import (
	"context"
	"errors"
	"os"
	"testing"
	"time"
)

func TestProcessAlive(t *testing.T) {
	if !ProcessAlive(os.Getpid()) {
		t.Fatal("ProcessAlive(self) = false, want true")
	}
	if ProcessAlive(0) || ProcessAlive(-1) {
		t.Fatal("ProcessAlive must reject non-positive PIDs")
	}
	// Far beyond any platform's PID maximum, so it cannot collide with a
	// real process.
	if ProcessAlive(0x3FFFFFFF) {
		t.Fatal("ProcessAlive(huge pid) = true, want false")
	}
}

func TestReadIntegrationDescriptorLifecycle(t *testing.T) {
	home := t.TempDir()
	root := t.TempDir()

	if _, err := ReadIntegrationDescriptor(home, root); !errors.Is(err, ErrDescriptorNotFound) {
		t.Fatalf("missing descriptor = %v, want ErrDescriptorNotFound", err)
	}

	registry := NewRegistry(Config{ProjectRoot: root})
	server, err := StartIntegrationServer(registry, IntegrationConfig{
		GlobalHome:  home,
		ProjectRoot: root,
		Generation:  1,
	})
	if err != nil {
		t.Fatalf("StartIntegrationServer: %v", err)
	}
	t.Cleanup(func() {
		_ = server.Shutdown(context.Background())
		_ = registry.Shutdown(context.Background())
	})

	descriptor, err := ReadIntegrationDescriptor(home, root)
	if err != nil {
		t.Fatalf("ReadIntegrationDescriptor: %v", err)
	}
	if descriptor.PID != os.Getpid() {
		t.Fatalf("descriptor PID = %d, want %d", descriptor.PID, os.Getpid())
	}
	if descriptor.URL == "" || descriptor.RegistrationToken == "" ||
		descriptor.Generation != 1 {
		t.Fatalf("descriptor = %#v", descriptor)
	}
}

func TestReadIntegrationDescriptorStaleAfterOwnerDies(t *testing.T) {
	home := t.TempDir()
	root := t.TempDir()

	// Publish a descriptor whose owner PID cannot exist.
	path, err := writeIntegrationDescriptor(home, IntegrationDescriptor{
		ProjectRoot:       root,
		URL:               "http://127.0.0.1:1",
		Generation:        7,
		RegistrationToken: "token",
		PID:               0x3FFFFFFF,
	})
	if err != nil {
		t.Fatalf("writeIntegrationDescriptor: %v", err)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatalf("descriptor not published: %v", err)
	}

	_, err = ReadIntegrationDescriptor(home, root)
	if !errors.Is(err, ErrDescriptorStale) {
		t.Fatalf("stale descriptor = %v, want ErrDescriptorStale", err)
	}
}

func TestRecordTerminalExitCoversEveryRunOnTerminal(t *testing.T) {
	clock := &fakeClock{now: time.Now()}
	registry := NewRegistry(Config{
		ProjectRoot: "/project",
		Now:         clock.Now,
		NewTicker:   clock.NewTicker,
	})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })

	registration := Registration{
		Profile: "agent-codex", Provider: "codex", PID: 1,
		TerminalID: "terminal-1", CWD: "/project",
	}
	firstRun, err := registry.RegisterLaunched(registration)
	if err != nil {
		t.Fatalf("RegisterLaunched: %v", err)
	}
	if !registry.RecordTerminalExit("terminal-1", 0, "session restarted") {
		t.Fatal("first RecordTerminalExit did not find run")
	}

	// A restarted session reuses the terminal ID; the already-exited record
	// must never shadow the still-running one regardless of map order.
	registration.PID = 2
	secondRun, err := registry.RegisterLaunched(registration)
	if err != nil {
		t.Fatalf("RegisterLaunched (restart): %v", err)
	}
	if !registry.RecordTerminalExit("terminal-1", 9, "killed") {
		t.Fatal("second RecordTerminalExit did not find run")
	}

	byID := make(map[string]Run)
	for _, run := range registry.Snapshot(10) {
		byID[run.ID] = run
	}
	if byID[firstRun.ID].State != StateExited ||
		byID[firstRun.ID].Exit.Result != "session restarted" {
		t.Fatalf("first run = %#v", byID[firstRun.ID])
	}
	second := byID[secondRun.ID]
	if second.State != StateExited || second.Exit == nil ||
		second.Exit.Code != 9 || second.Exit.Result != "killed" {
		t.Fatalf("second run = %#v, want exited with latest result", second)
	}
}
