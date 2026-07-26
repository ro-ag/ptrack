package store

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
)

func TestBoundedTrackingReadsReturnTotalsAndMore(t *testing.T) {
	s, err := Open(filepath.Join(t.TempDir(), "ptrack.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	for index := range 5 {
		plan, addErr := s.AddPlan(fmt.Sprintf("plan-%d", index))
		if addErr != nil {
			t.Fatal(addErr)
		}
		for taskIndex := range 4 {
			task, taskErr := s.AddTask(plan.ID, fmt.Sprintf("task-%d-%d", index, taskIndex))
			if taskErr != nil {
				t.Fatal(taskErr)
			}
			if taskIndex%2 == 0 {
				if err := s.SetTaskStatus(task.ID, model.TaskBlocked); err != nil {
					t.Fatal(err)
				}
			}
		}
	}
	plans, err := s.ListPlansBounded(3)
	if err != nil {
		t.Fatal(err)
	}
	if len(plans.Items) != 3 || plans.Total != 5 || plans.More != 2 {
		t.Fatalf("plans = %#v", plans)
	}
	tasks, err := s.ListTasksByPlanBounded(2, 2)
	if err != nil {
		t.Fatal(err)
	}
	if len(tasks.Items) != 2 || tasks.Total != 4 || tasks.More != 2 {
		t.Fatalf("tasks = %#v", tasks)
	}
	blockers, err := s.ListBlockedTasksBounded(4)
	if err != nil {
		t.Fatal(err)
	}
	if len(blockers.Items) != 4 || blockers.Total != 10 || blockers.More != 6 {
		t.Fatalf("blockers = %#v", blockers)
	}
}

func TestBoundedRecentNotesCommitsAndIssuesAreNewestFirst(t *testing.T) {
	s, err := Open(filepath.Join(t.TempDir(), "ptrack.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	for index := range 5 {
		if _, err := s.AddNote(model.TargetProject, 0, fmt.Sprintf("note-%d", index)); err != nil {
			t.Fatal(err)
		}
		if _, err := s.AddCommit(fmt.Sprintf("sha-%d", index), fmt.Sprintf("commit-%d", index), 0, 0); err != nil {
			t.Fatal(err)
		}
		if _, err := s.AddIssue(fmt.Sprintf("issue-%d", index), "", model.SeverityMedium, 0); err != nil {
			t.Fatal(err)
		}
	}
	notes, err := s.RecentNotesBounded(2)
	if err != nil {
		t.Fatal(err)
	}
	if notes.Total != 5 || notes.More != 3 || notes.Items[0].Body != "note-4" {
		t.Fatalf("notes = %#v", notes)
	}
	commits, err := s.RecentCommitsBounded(2)
	if err != nil {
		t.Fatal(err)
	}
	if commits.Total != 5 || commits.More != 3 || commits.Items[0].Subject != "commit-4" {
		t.Fatalf("commits = %#v", commits)
	}
	issues, err := s.ListOpenIssuesBounded(2)
	if err != nil {
		t.Fatal(err)
	}
	if issues.Total != 5 || issues.More != 3 || issues.Items[0].Title != "issue-4" {
		t.Fatalf("issues = %#v", issues)
	}
}

func TestBoundedReadsRejectInvalidLimits(t *testing.T) {
	s, err := Open(filepath.Join(t.TempDir(), "ptrack.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	if _, err := s.ListPlansBounded(0); err == nil {
		t.Fatal("ListPlansBounded accepted zero limit")
	}
	if _, err := s.RecentNotesBounded(-1); err == nil {
		t.Fatal("RecentNotesBounded accepted negative limit")
	}
}

func TestPlanTaskProgressCountsBeyondReturnedTaskLimit(t *testing.T) {
	s, err := Open(filepath.Join(t.TempDir(), "ptrack.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	plan, err := s.AddPlan("large plan")
	if err != nil {
		t.Fatal(err)
	}
	for index := range 5 {
		task, addErr := s.AddTask(plan.ID, fmt.Sprintf("task-%d", index))
		if addErr != nil {
			t.Fatal(addErr)
		}
		if index >= 2 {
			if err := s.SetTaskStatus(task.ID, model.TaskDone); err != nil {
				t.Fatal(err)
			}
		}
	}
	progress, err := s.PlanTaskProgress(plan.ID)
	if err != nil {
		t.Fatal(err)
	}
	if progress.Total != 5 || progress.Done != 3 {
		t.Fatalf("progress = %#v", progress)
	}
}

func TestContextAwareBoundedScansHonorCancellation(t *testing.T) {
	s, err := Open(filepath.Join(t.TempDir(), "ptrack.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := s.ListBlockedTasksBoundedContext(ctx, 10); !errors.Is(err, context.Canceled) {
		t.Fatalf("blocked task scan = %v, want cancellation", err)
	}
	if _, err := s.TaskAssociationsContext(
		ctx,
		map[uint64]bool{1: true},
	); !errors.Is(err, context.Canceled) {
		t.Fatalf("association scan = %v, want cancellation", err)
	}
}
