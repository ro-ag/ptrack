package agentrun

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"
)

func TestRegistryTracksLaunchedRunAndTerminalExit(t *testing.T) {
	now := time.Date(2026, 7, 26, 12, 0, 0, 0, time.UTC)
	clock := &fakeClock{now: now}
	registry := NewRegistry(Config{
		ProjectRoot: "/project",
		Now:         clock.Now,
		NewTicker:   clock.NewTicker,
	})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })

	run, err := registry.RegisterLaunched(Registration{
		Profile:    "agent-codex",
		Provider:   "codex",
		PID:        4242,
		TerminalID: "terminal-1",
		CWD:        "/project",
		PlanID:     2,
		TaskID:     9,
	})
	if err != nil {
		t.Fatalf("RegisterLaunched: %v", err)
	}
	if run.ID == "" || run.Kind != RegistrationLaunched ||
		run.ProcessState != ProcessRunning || run.LeaseState != LeaseNone ||
		run.ProjectRoot != "/project" {
		t.Fatalf("launched run = %#v", run)
	}
	clock.Advance(time.Minute)
	if !registry.RecordTerminalActivity("terminal-1") {
		t.Fatal("RecordTerminalActivity did not find run")
	}
	if !registry.RecordTerminalExit("terminal-1", 7, "failed") {
		t.Fatal("RecordTerminalExit did not find run")
	}
	snapshot := registry.Snapshot(10)
	if len(snapshot) != 1 || snapshot[0].State != StateExited ||
		snapshot[0].ProcessState != ProcessExited || snapshot[0].Exit == nil ||
		snapshot[0].Exit.Code != 7 || snapshot[0].LastActivityAt != clock.Now() {
		t.Fatalf("snapshot = %#v", snapshot)
	}
}

func TestRegistryAcceptsOwnedTerminalActivityTimeWithoutRegressing(t *testing.T) {
	start := time.Date(2026, 7, 26, 12, 0, 0, 0, time.UTC)
	clock := &fakeClock{now: start}
	registry := NewRegistry(Config{
		ProjectRoot: "/project",
		Now:         clock.Now,
		NewTicker:   clock.NewTicker,
	})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	run, err := registry.RegisterLaunched(Registration{
		Profile: "agent-codex", Provider: "codex", PID: 42,
		TerminalID: "terminal-1", CWD: "/project",
	})
	if err != nil {
		t.Fatalf("RegisterLaunched: %v", err)
	}
	activity := start.Add(15 * time.Second)
	if !registry.RecordTerminalActivityAt("terminal-1", activity) {
		t.Fatal("RecordTerminalActivityAt did not find run")
	}
	if !registry.RecordTerminalActivityAt("terminal-1", start.Add(time.Second)) {
		t.Fatal("RecordTerminalActivityAt did not find run for older signal")
	}
	got := registry.Snapshot(1)[0]
	if got.ID != run.ID || got.LastActivityAt != activity {
		t.Fatalf("last activity = %v, want %v", got.LastActivityAt, activity)
	}
}

func TestExternalLeaseHeartbeatAuthenticationAndExpiry(t *testing.T) {
	now := time.Date(2026, 7, 26, 12, 0, 0, 0, time.UTC)
	clock := &fakeClock{now: now}
	registry := NewRegistry(Config{
		ProjectRoot:   "/project",
		LeaseDuration: 30 * time.Second,
		Now:           clock.Now,
		NewTicker:     clock.NewTicker,
	})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })

	lease, err := registry.RegisterExternal(Registration{
		Profile:  "wrapper",
		Provider: "external-test",
		PID:      99,
		CWD:      "/project/subdir",
	})
	if err != nil {
		t.Fatalf("RegisterExternal: %v", err)
	}
	if lease.Run.ID == "" || lease.LeaseToken == "" ||
		lease.Run.State != StateRunning || lease.Run.ProcessState != ProcessUnknown ||
		lease.Run.LeaseState != LeaseActive {
		t.Fatalf("external lease = %#v", lease)
	}
	if err := registry.Heartbeat(lease.Run.ID, "wrong-token"); !errors.Is(err, ErrInvalidLease) {
		t.Fatalf("wrong heartbeat = %v want ErrInvalidLease", err)
	}
	clock.Advance(20 * time.Second)
	if err := registry.Heartbeat(lease.Run.ID, lease.LeaseToken); err != nil {
		t.Fatalf("Heartbeat: %v", err)
	}
	clock.Advance(31 * time.Second)
	registry.SweepExpired()
	run := registry.Snapshot(1)[0]
	if run.State != StateStale || run.LeaseState != LeaseExpired ||
		run.ProcessState != ProcessUnknown || registry.ActiveCount() != 0 {
		t.Fatalf("expired run = %#v active=%d", run, registry.ActiveCount())
	}
}

func TestRegistrySnapshotIsBoundedOrderedAndDoesNotExposeTokens(t *testing.T) {
	clock := &fakeClock{now: time.Now()}
	registry := NewRegistry(Config{
		ProjectRoot: "/project",
		Now:         clock.Now,
		NewTicker:   clock.NewTicker,
	})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	for _, provider := range []string{"one", "two", "three"} {
		if _, err := registry.RegisterExternal(Registration{
			Profile: provider, Provider: provider, CWD: "/project",
		}); err != nil {
			t.Fatal(err)
		}
		clock.Advance(time.Second)
	}
	snapshot := registry.Snapshot(2)
	if len(snapshot) != 2 || snapshot[0].Provider != "three" ||
		snapshot[1].Provider != "two" {
		t.Fatalf("snapshot order/bound = %#v", snapshot)
	}
}

func TestRegistryCapsRecordsWithoutEvictingActiveRuns(t *testing.T) {
	registry := NewRegistry(Config{ProjectRoot: "/project", MaxRecords: 2})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	first, err := registry.RegisterExternal(Registration{
		Profile: "one", Provider: "one", CWD: "/project",
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := registry.RegisterExternal(Registration{
		Profile: "two", Provider: "two", CWD: "/project",
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := registry.RegisterExternal(Registration{
		Profile: "three", Provider: "three", CWD: "/project",
	}); !errors.Is(err, ErrRegistryFull) {
		t.Fatalf("registration with all records active = %v, want ErrRegistryFull", err)
	}
	if err := registry.ExitExternal(first.Run.ID, first.LeaseToken, 0, "done"); err != nil {
		t.Fatal(err)
	}
	if _, err := registry.RegisterExternal(Registration{
		Profile: "three", Provider: "three", CWD: "/project",
	}); err != nil {
		t.Fatalf("registration after inactive eviction: %v", err)
	}
	snapshot := registry.Snapshot(10)
	if len(snapshot) != 2 {
		t.Fatalf("snapshot length = %d, want capped at 2", len(snapshot))
	}
	for _, run := range snapshot {
		if run.ID == first.Run.ID {
			t.Fatal("oldest inactive record was not evicted")
		}
	}
}

func TestRegistryValidatesImmutableRegistration(t *testing.T) {
	registry := NewRegistry(Config{ProjectRoot: "/project"})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	for _, registration := range []Registration{
		{Provider: "codex", CWD: "/project"},
		{Profile: "profile", CWD: "/project"},
		{Profile: "profile", Provider: "codex", CWD: "/other"},
	} {
		if _, err := registry.RegisterExternal(registration); err == nil {
			t.Fatalf("accepted registration %#v", registration)
		}
	}
}

func TestRegistryAdmissionFenceBlocksNewRunsUntilReleased(t *testing.T) {
	registry := NewRegistry(Config{ProjectRoot: "/project"})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	release := registry.FenceAdmission()
	if _, err := registry.RegisterExternal(Registration{
		Profile: "wrapper", Provider: "test", CWD: "/project",
	}); !errors.Is(err, ErrAdmissionFenced) {
		t.Fatalf("registration during fence = %v want ErrAdmissionFenced", err)
	}
	release()
	if _, err := registry.RegisterExternal(Registration{
		Profile: "wrapper", Provider: "test", CWD: "/project",
	}); err != nil {
		t.Fatalf("registration after release: %v", err)
	}
}

func TestRegistryShutdownIsIdempotent(t *testing.T) {
	clock := &fakeClock{now: time.Now()}
	registry := NewRegistry(Config{
		ProjectRoot: "/project",
		Now:         clock.Now,
		NewTicker:   clock.NewTicker,
	})
	const callers = 8
	results := make(chan error, callers)
	for range callers {
		go func() { results <- registry.Shutdown(context.Background()) }()
	}
	for range callers {
		if err := <-results; err != nil {
			t.Fatalf("Shutdown: %v", err)
		}
	}
	if got := clock.StopCount(); got != 1 {
		t.Fatalf("ticker Stop calls = %d want 1", got)
	}
}

type fakeClock struct {
	mu        sync.Mutex
	now       time.Time
	ticker    *fakeTicker
	stopCount int
}

func (c *fakeClock) Now() time.Time {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.now
}

func (c *fakeClock) Advance(duration time.Duration) {
	c.mu.Lock()
	c.now = c.now.Add(duration)
	c.mu.Unlock()
}

func (c *fakeClock) NewTicker(time.Duration) Ticker {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.ticker = &fakeTicker{
		channel: make(chan time.Time),
		stop: func() {
			c.mu.Lock()
			c.stopCount++
			c.mu.Unlock()
		},
	}
	return c.ticker
}

func (c *fakeClock) StopCount() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.stopCount
}

type fakeTicker struct {
	channel chan time.Time
	once    sync.Once
	stop    func()
}

func (t *fakeTicker) Channel() <-chan time.Time { return t.channel }
func (t *fakeTicker) Stop()                     { t.once.Do(t.stop) }
