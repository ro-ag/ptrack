package store

import (
	"errors"
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
)

func TestConvertTaskToPlanPreservesRelatedData(t *testing.T) {
	s := openTemp(t)
	m, _ := s.AddMilestone("release")
	parent, _ := s.AddPlan("parent")
	if err := s.SetPlanMilestone(parent.ID, m.ID); err != nil {
		t.Fatal(err)
	}
	task, _ := s.AddTask(parent.ID, "promote me")
	note, _ := s.AddNote(model.TargetTask, task.ID, "keep this decision")
	commit, _ := s.AddCommit("abc123", "task work", parent.ID, task.ID)
	issue, _ := s.AddIssue("follow-up", "", model.SeverityHigh, task.ID)

	plan, err := s.ConvertTaskToPlan(task.ID)
	if err != nil {
		t.Fatal(err)
	}
	if plan.Title != task.Title {
		t.Errorf("plan title = %q, want %q", plan.Title, task.Title)
	}
	if plan.Status != model.PlanActive {
		t.Errorf("plan status = %q, want active", plan.Status)
	}
	if plan.MilestoneID != m.ID {
		t.Errorf("plan milestone = %d, want %d", plan.MilestoneID, m.ID)
	}
	if !plan.CreatedAt.Equal(task.CreatedAt) {
		t.Errorf("plan created at = %v, want %v", plan.CreatedAt, task.CreatedAt)
	}
	if _, err := s.GetTask(task.ID); !errors.Is(err, ErrNotFound) {
		t.Errorf("converted task still exists: %v", err)
	}

	notes, _ := s.NotesByPlan(plan.ID)
	if len(notes) != 1 || notes[0].ID != note.ID {
		t.Errorf("plan notes = %+v, want note #%d", notes, note.ID)
	}
	commits, _ := s.CommitsByPlan(plan.ID)
	if len(commits) != 1 || commits[0].ID != commit.ID || commits[0].TaskID != 0 {
		t.Errorf("plan commits = %+v, want converted commit #%d", commits, commit.ID)
	}
	gotIssue, _ := s.GetIssue(issue.ID)
	if gotIssue.TaskID != 0 {
		t.Errorf("issue task = %d, want unlinked", gotIssue.TaskID)
	}
}

func TestConvertDoneTaskCreatesDonePlan(t *testing.T) {
	s := openTemp(t)
	parent, _ := s.AddPlan("parent")
	task, _ := s.AddTask(parent.ID, "completed workstream")
	if err := s.SetTaskStatus(task.ID, model.TaskDone); err != nil {
		t.Fatal(err)
	}

	plan, err := s.ConvertTaskToPlan(task.ID)
	if err != nil {
		t.Fatal(err)
	}
	if plan.Status != model.PlanDone {
		t.Errorf("plan status = %q, want done", plan.Status)
	}
}

func TestConvertMissingTaskDoesNotCreatePlan(t *testing.T) {
	s := openTemp(t)
	if _, err := s.ConvertTaskToPlan(404); !errors.Is(err, ErrNotFound) {
		t.Fatalf("convert missing task error = %v, want ErrNotFound", err)
	}
	plans, err := s.ListPlans()
	if err != nil {
		t.Fatal(err)
	}
	if len(plans) != 0 {
		t.Errorf("plans = %+v, want none", plans)
	}
}
