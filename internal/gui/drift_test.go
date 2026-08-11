package gui

import (
	"context"
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/gitinfo"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

func TestWorkspaceDriftUsesOnlyExactLinksOwnershipAndCurrentEventEvidence(t *testing.T) {
	app, root := newTerminalBindingTestApp(t, &fakeGUITerminalManager{}, nil)
	planID, taskID, otherPlanID := seedAssociationCatalog(t, root)
	s, err := store.Open(app.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	otherTask, err := s.AddTask(otherPlanID, "Other task")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.AddCommit("linked-sha", "ignored subject", planID, taskID); err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	registry := agentrun.NewRegistry(agentrun.Config{ProjectRoot: root})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	app.workspace.agents = &workspaceAgentResources{registry: registry}
	first, err := registry.RegisterExternal(agentrun.Registration{Profile: "one", Provider: "codex", CWD: root})
	if err != nil {
		t.Fatal(err)
	}
	second, err := registry.RegisterExternal(agentrun.Registration{Profile: "two", Provider: "codex", CWD: root})
	if err != nil {
		t.Fatal(err)
	}
	for _, binding := range []struct {
		runID          string
		planID, taskID uint64
	}{
		{first.Run.ID, planID, taskID},
		{second.Run.ID, otherPlanID, otherTask.ID},
	} {
		if _, err := app.AssociateAgentRunV2(1, binding.runID, association.PointerV1{
			Version: association.VersionV1, PlanID: binding.planID, TaskID: binding.taskID,
		}); err != nil {
			t.Fatal(err)
		}
		if _, err := app.SetAgentTaskOwnershipV2(1, binding.runID, 1, true); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := registry.RecordEvent(first.Run.ID, first.LeaseToken, agentrun.EventObservation{
		ModelVersion: agentrun.EventModelVersion, SourceID: "drift-1", SourceSequence: 1,
		Kind: agentrun.EventError, Phase: agentrun.EventProgress,
		Paths: []string{"shared/current.go"}, ErrorClass: "scope_mismatch",
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := registry.RecordEvent(second.Run.ID, second.LeaseToken, agentrun.EventObservation{
		ModelVersion: agentrun.EventModelVersion, SourceID: "file-1", SourceSequence: 1,
		Kind: agentrun.EventFile, Phase: agentrun.EventProgress,
		Paths: []string{"shared/current.go"},
	}); err != nil {
		t.Fatal(err)
	}
	app.gitSnapshots = fakeGitSnapshotter{snapshot: gitinfo.Snapshot{
		State: gitinfo.RepositoryReady,
		Status: gitinfo.Status{
			ChangedPaths: []string{"tracked.go"}, UntrackedPaths: []string{"new.go"},
			ChangedPathBounds:   gitinfo.PathBounds{Shown: 1, Total: 1},
			UntrackedPathBounds: gitinfo.PathBounds{Shown: 1, Total: 1},
		},
		RecentCommits: []gitinfo.Commit{
			{SHA: "linked-sha", Date: time.Now().UTC().Format(time.RFC3339)},
			{SHA: "unlinked-sha", Date: time.Now().UTC().Format(time.RFC3339)},
		},
		UnpushedCommits: []gitinfo.Commit{{SHA: "unlinked-sha", Date: time.Now().UTC().Format(time.RFC3339)}},
	}}
	snapshot, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	want := map[DriftFindingKind]bool{
		DriftCheckoutChangedPath: false, DriftUntrackedFile: false,
		DriftUnlinkedCommit: false, DriftCrossTaskPath: false, DriftTaskSignal: false,
	}
	unlinked := 0
	for _, finding := range snapshot.Drift.Findings {
		want[finding.Kind] = true
		if finding.Kind == DriftUnlinkedCommit {
			unlinked++
			if finding.SHA != "unlinked-sha" {
				t.Fatalf("unlinked commit = %#v", finding)
			}
		}
		if finding.Kind == DriftCrossTaskPath &&
			(finding.Path != "shared/current.go" || len(finding.TaskIDs) != 2) {
			t.Fatalf("overlap = %#v", finding)
		}
	}
	for kind, found := range want {
		if !found {
			t.Fatalf("missing drift kind %q: %#v", kind, snapshot.Drift)
		}
	}
	if unlinked != 1 {
		t.Fatalf("unlinked findings = %d", unlinked)
	}
	encoded, _ := json.Marshal(snapshot.Drift)
	for _, forbidden := range []string{`"projectRoot"`, `"subject"`, `"summary"`, root} {
		if strings.Contains(string(encoded), forbidden) {
			t.Fatalf("drift DTO contains %q: %s", forbidden, encoded)
		}
	}

	if _, err := app.AssociateAgentRunV2(1, second.Run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: otherPlanID, TaskID: otherTask.ID,
	}); err != nil {
		t.Fatal(err)
	}
	refreshed, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	for _, finding := range refreshed.Drift.Findings {
		if finding.Kind == DriftCrossTaskPath {
			t.Fatalf("stale event/ownership produced overlap: %#v", finding)
		}
	}
}

func TestDriftSnapshotBoundsProjectStatusFindings(t *testing.T) {
	paths := make([]string, driftFindingLimit+5)
	for index := range paths {
		paths[index] = "path/file-" + string(rune('a'+index))
	}
	workspace := newWorkspaceContext(workspaceContextConfig{generation: 1})
	result := buildDriftSnapshot(workspace, runtimeProjection{}, AgentActivitySnapshot{}, GitSnapshot{
		State: SnapshotReady,
		Snapshot: gitinfo.Snapshot{State: gitinfo.RepositoryReady, Status: gitinfo.Status{
			UntrackedPaths:      paths,
			UntrackedPathBounds: gitinfo.PathBounds{Shown: len(paths), Total: len(paths)},
		}},
	}, nil, time.Time{})
	if len(result.Findings) != driftFindingLimit || !result.Incomplete || result.Bounds.More != 5 {
		t.Fatalf("drift bounds = %#v findings=%d", result.Bounds, len(result.Findings))
	}
}

func TestDriftSnapshotIgnoresPreTrackingCommitsAndPrioritizesWarnings(t *testing.T) {
	trackingStartedAt := time.Date(2026, time.August, 10, 12, 0, 0, 0, time.UTC)
	result := buildDriftSnapshot(
		newWorkspaceContext(workspaceContextConfig{generation: 1}),
		runtimeProjection{},
		AgentActivitySnapshot{},
		GitSnapshot{State: SnapshotReady, Snapshot: gitinfo.Snapshot{
			State: gitinfo.RepositoryReady,
			Status: gitinfo.Status{
				ChangedPaths:        []string{"tracked.go"},
				UntrackedPaths:      []string{"new.go"},
				ChangedPathBounds:   gitinfo.PathBounds{Shown: 1, Total: 1},
				UntrackedPathBounds: gitinfo.PathBounds{Shown: 1, Total: 1},
			},
			RecentCommits: []gitinfo.Commit{{
				SHA: "historical", Date: trackingStartedAt.Add(-time.Hour).Format(time.RFC3339),
			}},
		}},
		nil,
		trackingStartedAt,
	)
	if len(result.Findings) != 2 || result.Findings[0].Kind != DriftUntrackedFile {
		t.Fatalf("warning-first findings = %#v", result.Findings)
	}
	for _, finding := range result.Findings {
		if finding.Kind == DriftUnlinkedCommit {
			t.Fatalf("pre-tracking commit was flagged: %#v", finding)
		}
	}
}

func TestDriftMatchesOnlyUnambiguousAbbreviatedLinkedCommit(t *testing.T) {
	full := "abc1234" + strings.Repeat("0", 33)
	linked := linkedObservedCommitSHAs(
		[]model.Commit{{SHA: "abc1234"}},
		[]gitinfo.Commit{{SHA: full}, {SHA: "def5678" + strings.Repeat("0", 33)}},
	)
	if !linked[full] || len(linked) != 1 {
		t.Fatalf("abbreviated links = %#v", linked)
	}
	ambiguous := linkedObservedCommitSHAs(
		[]model.Commit{{SHA: "abc1234"}},
		[]gitinfo.Commit{{SHA: full}, {SHA: "abc1234" + strings.Repeat("1", 33)}},
	)
	if len(ambiguous) != 0 {
		t.Fatalf("ambiguous abbreviated link was accepted: %#v", ambiguous)
	}
}
