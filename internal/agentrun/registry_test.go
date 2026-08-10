package agentrun

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/association"
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

type registryAssociationCatalog struct{}

func (registryAssociationCatalog) ValidatePlan(planID uint64) error {
	if planID != 2 {
		return errors.New("not found")
	}
	return nil
}

func (registryAssociationCatalog) TaskPlan(taskID uint64) (uint64, error) {
	if taskID != 9 {
		return 0, errors.New("not found")
	}
	return 2, nil
}

func TestRegistryAssociationsAreHostOwnedMonotonicAndSnapshotSafe(t *testing.T) {
	projectRoot := t.TempDir()
	registry := NewRegistry(Config{ProjectRoot: projectRoot})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	lease, err := registry.RegisterExternal(Registration{
		Profile: "wrapper", Provider: "external", CWD: projectRoot,
	})
	if err != nil {
		t.Fatal(err)
	}
	if lease.Run.Association != nil {
		t.Fatalf("registration association = %#v", lease.Run.Association)
	}
	host, err := association.NewHost(projectRoot, 4, registryAssociationCatalog{})
	if err != nil {
		t.Fatal(err)
	}
	first, err := registry.Associate(lease.Run.ID, host, association.PointerV1{
		Version: association.VersionV1, PlanID: 2, TaskID: 9,
	})
	if err != nil {
		t.Fatal(err)
	}
	second, err := registry.Associate(lease.Run.ID, host, association.PointerV1{
		Version: association.VersionV1, PlanID: 2,
	})
	if err != nil {
		t.Fatal(err)
	}
	if first.Revision != 1 || second.Revision != 2 || second.LiveID != lease.Run.ID {
		t.Fatalf("associations = first %#v second %#v", first, second)
	}
	snapshot := registry.Snapshot(1)
	if snapshot[0].Association == nil || snapshot[0].Association.Revision != 2 {
		t.Fatalf("snapshot = %#v", snapshot[0])
	}
	snapshot[0].Association.Revision = 99
	if got := registry.Snapshot(1)[0].Association.Revision; got != 2 {
		t.Fatalf("snapshot mutation changed registry revision to %d", got)
	}
}

func TestRegisterLinkedLaunchedIsAtomicAndRollbackIsTerminalScoped(t *testing.T) {
	projectRoot := t.TempDir()
	registry := NewRegistry(Config{ProjectRoot: projectRoot})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	host, err := association.NewHost(projectRoot, 4, registryAssociationCatalog{})
	if err != nil {
		t.Fatal(err)
	}
	registration := Registration{
		Profile: "agent-codex", Provider: "codex", PID: 42,
		TerminalID: "linked-terminal", CWD: projectRoot,
	}
	if _, err := registry.RegisterLinkedLaunched(
		registration,
		host,
		association.PointerV1{Version: 2},
	); !errors.Is(err, association.ErrUnsupportedVersion) {
		t.Fatalf("invalid linked registration = %v", err)
	}
	if len(registry.Snapshot(8)) != 0 {
		t.Fatal("failed binding published a detached run")
	}
	run, err := registry.RegisterLinkedLaunched(
		registration,
		host,
		association.PointerV1{Version: association.VersionV1, PlanID: 2, TaskID: 9},
	)
	if err != nil {
		t.Fatal(err)
	}
	if run.Association == nil || run.Association.Revision != 1 ||
		run.Association.Target.PlanID != 2 || run.Association.Target.TaskID != 9 {
		t.Fatalf("linked run = %#v", run)
	}
	if !registry.HasLinkedTerminal("linked-terminal") {
		t.Fatal("linked terminal provenance was not recorded")
	}
	if registry.RollbackLinkedLaunched(run.ID, "another-terminal") {
		t.Fatal("rollback removed a run owned by another terminal")
	}
	ordinary, err := registry.RegisterLaunched(Registration{
		Profile: "agent-codex", Provider: "codex", PID: 43,
		TerminalID: "ordinary-terminal", CWD: projectRoot,
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := registry.Associate(
		ordinary.ID,
		host,
		association.PointerV1{
			Version: association.VersionV1,
			PlanID:  2,
			TaskID:  9,
		},
	); err != nil {
		t.Fatal(err)
	}
	if removed := registry.RollbackLinkedTerminal("ordinary-terminal"); removed != 0 {
		t.Fatalf("linked rollback removed %d associated ordinary runs", removed)
	}
	if registry.HasLinkedTerminal("ordinary-terminal") {
		t.Fatal("associated ordinary run gained linked-launch provenance")
	}
	if removed := registry.RollbackLinkedTerminal("linked-terminal"); removed != 1 {
		t.Fatalf("terminal rollback removed %d runs", removed)
	}
	if registry.HasLinkedTerminal("linked-terminal") {
		t.Fatal("linked terminal provenance survived rollback")
	}
	snapshot := registry.Snapshot(8)
	if len(snapshot) != 1 || snapshot[0].ID != ordinary.ID {
		t.Fatalf("terminal rollback result = %#v", snapshot)
	}
}

func TestLinkedAssociationChangeTracksTerminalCASAndPreservesProvenance(t *testing.T) {
	projectRoot := t.TempDir()
	registry := NewRegistry(Config{ProjectRoot: projectRoot})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	host, err := association.NewHost(projectRoot, 4, registryAssociationCatalog{})
	if err != nil {
		t.Fatal(err)
	}
	pointer := association.PointerV1{
		Version: association.VersionV1, PlanID: 2, TaskID: 9,
	}
	terminalAssociation, err := host.Bind("linked-terminal", pointer, nil)
	if err != nil {
		t.Fatal(err)
	}
	run, err := registry.RegisterLinkedLaunched(Registration{
		Profile: "agent", Provider: "test", PID: 42,
		TerminalID: "linked-terminal", CWD: projectRoot,
	}, host, pointer)
	if err != nil {
		t.Fatal(err)
	}
	terminalNext, err := host.Bind(
		"linked-terminal",
		association.PointerV1{Version: association.VersionV1, PlanID: 2},
		&terminalAssociation,
	)
	if err != nil {
		t.Fatal(err)
	}
	change, found, err := registry.PrepareLinkedTerminalAssociationChange(
		"linked-terminal",
		&terminalAssociation,
		terminalNext,
		host,
		association.PointerV1{Version: association.VersionV1, PlanID: 2},
	)
	if err != nil || !found {
		t.Fatalf("prepare = found %t err %v", found, err)
	}
	if current := registry.Snapshot(1)[0].Association; current == nil ||
		current.Target.TaskID != 9 || current.Revision != 1 {
		t.Fatalf("prepare changed run = %#v", current)
	}
	if err := registry.CommitLinkedAssociationChange(change); err != nil {
		t.Fatal(err)
	}
	current := registry.Snapshot(1)[0]
	if current.ID != run.ID || current.Association == nil ||
		current.Association.Target.TaskID != 0 || current.Association.Revision != 2 {
		t.Fatalf("committed run = %#v", current)
	}
	if err := registry.CommitLinkedAssociationChange(change); !errors.Is(err, ErrAssociationMismatch) {
		t.Fatalf("replayed commit = %v", err)
	}
	if err := registry.RollbackLinkedAssociationChange(change); err != nil {
		t.Fatal(err)
	}
	if !registry.IsLinkedLaunchRun(run.ID) ||
		!registry.HasLinkedTerminal("linked-terminal") {
		t.Fatal("association rollback lost immutable linked-launch provenance")
	}
}

func TestLinkedAssociationChangeFailsClosedOnCorrespondenceMismatch(t *testing.T) {
	projectRoot := t.TempDir()
	registry := NewRegistry(Config{ProjectRoot: projectRoot})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	host, err := association.NewHost(projectRoot, 4, registryAssociationCatalog{})
	if err != nil {
		t.Fatal(err)
	}
	pointer := association.PointerV1{
		Version: association.VersionV1, PlanID: 2, TaskID: 9,
	}
	terminalAssociation, err := host.Bind("linked-terminal", pointer, nil)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := registry.RegisterLinkedLaunched(Registration{
		Profile: "agent", Provider: "test", PID: 42,
		TerminalID: "linked-terminal", CWD: projectRoot,
	}, host, pointer); err != nil {
		t.Fatal(err)
	}
	mismatch := terminalAssociation
	mismatch.Revision++
	if _, found, err := registry.PrepareLinkedTerminalAssociationChange(
		"linked-terminal",
		&mismatch,
		mismatch,
		host,
		association.PointerV1{Version: association.VersionV1, PlanID: 2},
	); found || !errors.Is(err, ErrAssociationMismatch) {
		t.Fatalf("mismatch = found %t err %v", found, err)
	}
	if _, found, err := registry.PrepareLinkedTerminalAssociationChange(
		"ordinary-terminal",
		nil,
		terminalAssociation,
		host,
		association.PointerV1{Version: association.VersionV1, PlanID: 2},
	); err != nil || found {
		t.Fatalf("ordinary terminal = found %t err %v", found, err)
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
