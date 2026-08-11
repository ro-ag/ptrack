package gui

import (
	"context"
	"errors"
	"fmt"
	"testing"

	"github.com/ro-ag/ptrack/internal/gitinfo"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

type fakeGitSnapshotter struct {
	snapshot gitinfo.Snapshot
	err      error
}

func (f fakeGitSnapshotter) Capture(context.Context, string) (gitinfo.Snapshot, error) {
	return f.snapshot, f.err
}

func TestWorkspaceSnapshotCombinesBoundedProjectIntelligence(t *testing.T) {
	app := seedApp(t)
	app.gitSnapshots = fakeGitSnapshotter{snapshot: gitinfo.Snapshot{
		State: gitinfo.RepositoryReady,
		Status: gitinfo.Status{
			Branch:    "feat/project-workspace",
			Staged:    2,
			Untracked: 1,
		},
	}}

	snapshot, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatalf("GetWorkspaceSnapshot: %v", err)
	}
	if snapshot.Generation != 1 || snapshot.Tracking.Board.Goal != "Ship the GUI" {
		t.Fatalf("snapshot identity = %#v", snapshot)
	}
	if snapshot.Project.Root == "" || !snapshot.Project.Storage.Exists ||
		snapshot.Project.Storage.FormatVersion != store.CurrentFormat {
		t.Fatalf("project storage = %#v", snapshot.Project)
	}
	if snapshot.Tracking.Board.PlanTitle != "Desktop board" ||
		len(snapshot.Tracking.Notes) != 1 ||
		len(snapshot.Tracking.Issues) != 1 {
		t.Fatalf("tracking snapshot = %#v", snapshot.Tracking)
	}
	todo := snapshot.Tracking.Board.Columns[0].Tasks
	if len(todo) != 1 || todo[0].LatestNote == "" {
		t.Fatalf("task latest note = %#v, want preserved board context", todo)
	}
	if snapshot.Git.State != SnapshotReady ||
		snapshot.Git.Snapshot.Status.Branch != "feat/project-workspace" {
		t.Fatalf("git snapshot = %#v", snapshot.Git)
	}
	if snapshot.Terminals.State != SnapshotReady ||
		snapshot.AgentRuns.State != SnapshotReady ||
		snapshot.AgentActivity.State != SnapshotReady {
		t.Fatalf("runtime sections = terminals %#v agents %#v", snapshot.Terminals, snapshot.AgentRuns)
	}
	if snapshot.AgentActivity.Bounds != snapshot.AgentRuns.Bounds {
		t.Fatalf("agent activity bounds = %#v, runs = %#v", snapshot.AgentActivity.Bounds, snapshot.AgentRuns.Bounds)
	}
}

func TestWorkspaceSnapshotKeepsTrackingWhenGitFails(t *testing.T) {
	app := seedApp(t)
	app.gitSnapshots = fakeGitSnapshotter{err: errors.New("git timed out")}

	snapshot, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatalf("GetWorkspaceSnapshot: %v", err)
	}
	if snapshot.Git.State != SnapshotError || snapshot.Git.Error == "" {
		t.Fatalf("git section = %#v, want an explicit partial error", snapshot.Git)
	}
	if snapshot.Tracking.Board.Goal == "" {
		t.Fatal("tracking data was discarded with the Git error")
	}
}

func TestWorkspaceSnapshotTaskAssociationsAreNotLimitedToRecentActivity(t *testing.T) {
	app := seedApp(t)
	s, err := store.Open(app.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	for index := range snapshotNoteLimit + 1 {
		if _, err := s.AddNote(
			model.TargetProject,
			0,
			fmt.Sprintf("newer project note %d", index),
		); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	app.gitSnapshots = fakeGitSnapshotter{
		snapshot: gitinfo.Snapshot{State: gitinfo.RepositoryNotFound},
	}

	snapshot, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	task := snapshot.Tracking.Board.Columns[0].Tasks[0]
	if task.NoteCount != 1 || task.CommitCount != 1 ||
		task.IssueCount != 1 || task.LatestNote == "" {
		t.Fatalf("task associations = %#v, want complete selected-task counts", task)
	}
}

func TestWorkspaceSnapshotSupportsProjectWithoutActivePlan(t *testing.T) {
	dbPath := projectTestDBPath(t)
	s, err := store.Open(dbPath)
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	if err := s.SetGoal("Choose the next plan"); err != nil {
		t.Fatalf("set goal: %v", err)
	}
	if err := s.Close(); err != nil {
		t.Fatalf("close store: %v", err)
	}
	app := newApp(dbPath, 0)
	app.gitSnapshots = fakeGitSnapshotter{
		snapshot: gitinfo.Snapshot{State: gitinfo.RepositoryNotFound},
	}

	snapshot, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatalf("GetWorkspaceSnapshot: %v", err)
	}
	if snapshot.Tracking.Board.PlanID != 0 ||
		snapshot.Tracking.Board.Goal != "Choose the next plan" ||
		len(snapshot.Tracking.Board.Columns) != len(statuses) {
		t.Fatalf("planless board = %#v", snapshot.Tracking.Board)
	}
}

func TestPlanSummariesCarryPerPlanTaskProgress(t *testing.T) {
	app := seedApp(t)
	s, err := store.Open(app.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	other, err := s.AddPlan("CLI polish")
	if err != nil {
		t.Fatal(err)
	}
	doneTask, err := s.AddTask(other.ID, "Ship search")
	if err != nil {
		t.Fatal(err)
	}
	if err := s.SetTaskStatus(doneTask.ID, model.TaskDone); err != nil {
		t.Fatal(err)
	}
	openTask, err := s.AddTask(other.ID, "Polish help text")
	if err != nil {
		t.Fatal(err)
	}
	_ = openTask
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}

	assertProgress := func(plans []PlanSummary) {
		t.Helper()
		if len(plans) != 2 {
			t.Fatalf("plans = %#v, want the seeded and added plans", plans)
		}
		if plans[0].TasksTotal != 2 || plans[0].TasksDone != 0 || !plans[0].IsActive {
			t.Fatalf("active plan summary = %#v, want 2 tasks, 0 done", plans[0])
		}
		if plans[1].TasksTotal != 2 || plans[1].TasksDone != 1 || plans[1].IsActive {
			t.Fatalf("second plan summary = %#v, want 2 tasks, 1 done", plans[1])
		}
	}

	board, err := app.GetBoard(0)
	if err != nil {
		t.Fatalf("GetBoard: %v", err)
	}
	assertProgress(board.Plans)

	app.gitSnapshots = fakeGitSnapshotter{
		snapshot: gitinfo.Snapshot{State: gitinfo.RepositoryNotFound},
	}
	snapshot, err := app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatalf("GetWorkspaceSnapshot: %v", err)
	}
	assertProgress(snapshot.Tracking.Board.Plans)
}

func TestWorkspaceSnapshotRejectsStaleGeneration(t *testing.T) {
	app := seedApp(t)
	if _, err := app.GetWorkspaceSnapshot(2, 0); !errors.Is(err, errStaleWorkspaceGeneration) {
		t.Fatalf("GetWorkspaceSnapshot stale generation = %v", err)
	}
}
