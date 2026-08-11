package agentrun

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/association"
)

func TestRecordedEventsUseHostOwnedEventTimeCorrelation(t *testing.T) {
	projectRoot := t.TempDir()
	if err := os.Mkdir(filepath.Join(projectRoot, ".git"), 0o700); err != nil {
		t.Fatal(err)
	}
	now := time.Now().UTC()
	registry := newEventRegistryForTest(t, Config{
		ProjectRoot: projectRoot,
		Now:         func() time.Time { return now },
	})
	lease, err := registry.RegisterExternal(Registration{
		Profile: "wrapper", Provider: "codex", CWD: projectRoot,
	})
	if err != nil {
		t.Fatal(err)
	}
	host, err := association.NewHost(projectRoot, 7, registryAssociationCatalog{})
	if err != nil {
		t.Fatal(err)
	}
	firstAssociation, err := registry.Associate(lease.Run.ID, host, association.PointerV1{
		Version: association.VersionV1, PlanID: 2, TaskID: 9,
	})
	if err != nil {
		t.Fatal(err)
	}
	first, err := registry.RecordEvent(lease.Run.ID, lease.LeaseToken, EventObservation{
		ModelVersion: EventModelVersion, SourceID: "file-1", SourceSequence: 1,
		Kind: EventFile, Phase: EventProgress, Subject: "write",
		Paths: []string{"internal/agentrun/correlation.go"},
	})
	if err != nil {
		t.Fatal(err)
	}
	canonicalRoot := canonicalRegistryPath(projectRoot)
	if first.Correlation.ProjectRoot != canonicalRoot ||
		first.Correlation.RepositoryRoot != canonicalRoot ||
		first.Correlation.PlanID != 2 || first.Correlation.TaskID != 9 ||
		first.Correlation.Generation != 7 ||
		first.Correlation.AssociationRevision != firstAssociation.Revision {
		t.Fatalf("first correlation = %#v", first.Correlation)
	}

	secondAssociation, err := registry.Associate(lease.Run.ID, host, association.PointerV1{
		Version: association.VersionV1, PlanID: 2,
	})
	if err != nil {
		t.Fatal(err)
	}
	second, err := registry.RecordEvent(lease.Run.ID, lease.LeaseToken, EventObservation{
		ModelVersion: EventModelVersion, SourceID: "tool-2", SourceSequence: 2,
		Kind: EventTool, Phase: EventCompleted, Subject: "test",
	})
	if err != nil {
		t.Fatal(err)
	}
	if second.Correlation.PlanID != 2 || second.Correlation.TaskID != 0 ||
		second.Correlation.AssociationRevision != secondAssociation.Revision {
		t.Fatalf("second correlation = %#v", second.Correlation)
	}
	events, _, err := registry.EventSnapshot(lease.Run.ID, 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(events) != 2 || events[0].Correlation.TaskID != 9 ||
		events[0].Correlation.AssociationRevision != firstAssociation.Revision {
		t.Fatalf("reassociation rewrote historical correlation: %#v", events)
	}
}

func TestEventCorrelationIncludesOwnedTerminalWithoutInspectingContent(t *testing.T) {
	projectRoot := t.TempDir()
	host, err := association.NewHost(projectRoot, 3, registryAssociationCatalog{})
	if err != nil {
		t.Fatal(err)
	}
	run := Run{
		ID: "run-1", ProjectRoot: host.ProjectRoot(), TerminalID: "terminal-1",
		Kind: RegistrationLaunched,
	}
	bound, err := host.Bind(run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: 2, TaskID: 9,
	}, nil)
	if err != nil {
		t.Fatal(err)
	}
	run.Association = &bound
	correlation := eventCorrelationForRun(run, projectRoot)
	if correlation.TerminalID != "terminal-1" || correlation.PlanID != 2 ||
		correlation.TaskID != 9 || correlation.Generation != 3 {
		t.Fatalf("terminal correlation = %#v", correlation)
	}
}

func TestEventCorrelationFindsRepositoryContainingNestedProject(t *testing.T) {
	repositoryRoot := t.TempDir()
	if err := os.WriteFile(filepath.Join(repositoryRoot, ".git"), []byte("gitdir: elsewhere\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	projectRoot := filepath.Join(repositoryRoot, "projects", "alpha")
	if err := os.MkdirAll(projectRoot, 0o700); err != nil {
		t.Fatal(err)
	}
	registry := newEventRegistryForTest(t, Config{ProjectRoot: projectRoot})
	lease, err := registry.RegisterExternal(Registration{
		Profile: "wrapper", Provider: "codex", CWD: projectRoot,
	})
	if err != nil {
		t.Fatal(err)
	}
	event, err := registry.RecordEvent(lease.Run.ID, lease.LeaseToken, EventObservation{
		ModelVersion: EventModelVersion, SourceID: "event-1", SourceSequence: 1,
		Kind: EventLifecycle, Phase: EventProgress,
	})
	if err != nil {
		t.Fatal(err)
	}
	if event.Correlation.RepositoryRoot != canonicalRegistryPath(repositoryRoot) {
		t.Fatalf("repository root = %q, want %q", event.Correlation.RepositoryRoot, repositoryRoot)
	}
}
