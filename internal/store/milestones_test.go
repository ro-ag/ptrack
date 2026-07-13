package store

import (
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
)

func TestMilestoneCRUDAndPlanLink(t *testing.T) {
	s := openTemp(t)
	m1, err := s.AddMilestone("v1.0")
	if err != nil {
		t.Fatal(err)
	}
	if m1.Status != model.MilestoneOpen {
		t.Errorf("new milestone status = %q want open", m1.Status)
	}
	m2, _ := s.AddMilestone("v2.0")
	if m1.ID == m2.ID {
		t.Fatal("milestone ids must differ")
	}

	due := time.Date(2026, 12, 1, 0, 0, 0, 0, time.UTC)
	if err := s.SetMilestoneDue(m1.ID, due); err != nil {
		t.Fatal(err)
	}
	got, _ := s.GetMilestone(m1.ID)
	if !got.Due.Equal(due) {
		t.Errorf("due = %v want %v", got.Due, due)
	}

	p, _ := s.AddPlan("storage")
	if err := s.SetPlanMilestone(p.ID, m1.ID); err != nil {
		t.Fatal(err)
	}
	plans, _ := s.ListPlansByMilestone(m1.ID)
	if len(plans) != 1 || plans[0].ID != p.ID {
		t.Errorf("ListPlansByMilestone = %+v", plans)
	}
	if err := s.SetPlanMilestone(p.ID, 999); err != ErrNotFound {
		t.Errorf("linking to missing milestone should fail, got %v", err)
	}

	if err := s.SetMilestoneStatus(m1.ID, model.MilestoneDone); err != nil {
		t.Fatal(err)
	}
	got, _ = s.GetMilestone(m1.ID)
	if got.Status != model.MilestoneDone {
		t.Errorf("status = %q want done", got.Status)
	}
	if _, err := s.GetMilestone(999); err != ErrNotFound {
		t.Errorf("want ErrNotFound, got %v", err)
	}
}

func TestIssueCRUD(t *testing.T) {
	s := openTemp(t)
	p, _ := s.AddPlan("p")
	tk, _ := s.AddTask(p.ID, "t")

	is, err := s.AddIssue("panic on nil", "stack trace here", "", tk.ID)
	if err != nil {
		t.Fatal(err)
	}
	if is.Status != model.IssueOpen {
		t.Errorf("new issue status = %q want open", is.Status)
	}
	if is.Severity != model.SeverityMedium {
		t.Errorf("default severity = %q want medium", is.Severity)
	}
	if is.TaskID != tk.ID {
		t.Errorf("task link = %d want %d", is.TaskID, tk.ID)
	}

	if _, err := s.AddIssue("orphan", "", model.SeverityHigh, 999); err != ErrNotFound {
		t.Errorf("linking to missing task should fail, got %v", err)
	}

	if err := s.SetIssueSeverity(is.ID, model.SeverityCritical); err != nil {
		t.Fatal(err)
	}
	if err := s.SetIssueStatus(is.ID, model.IssueClosed); err != nil {
		t.Fatal(err)
	}
	got, _ := s.GetIssue(is.ID)
	if got.Severity != model.SeverityCritical || got.Status != model.IssueClosed {
		t.Errorf("issue = %+v", got)
	}

	all, _ := s.ListIssues()
	if len(all) != 1 {
		t.Errorf("ListIssues = %d want 1", len(all))
	}
}

func TestCountsIncludeMilestonesAndIssues(t *testing.T) {
	s := openTemp(t)
	m, _ := s.AddMilestone("m")
	s.SetMilestoneStatus(m.ID, model.MilestoneDone)
	s.AddMilestone("m2")
	i1, _ := s.AddIssue("a", "", "", 0)
	s.AddIssue("b", "", "", 0)
	s.SetIssueStatus(i1.ID, model.IssueClosed)

	c, err := s.Counts()
	if err != nil {
		t.Fatal(err)
	}
	if c.Milestones != 2 || c.MilestonesDone != 1 {
		t.Errorf("milestone counts = %d/%d want 2/1", c.Milestones, c.MilestonesDone)
	}
	if c.Issues != 2 || c.IssuesOpen != 1 {
		t.Errorf("issue counts = %d/%d want 2/1", c.Issues, c.IssuesOpen)
	}
}

func TestV1DBMigratesToV2(t *testing.T) {
	// A database stamped format 1 opens cleanly under this v2 build and is
	// adopted to CurrentFormat, with the new buckets usable.
	path := t.TempDir() + "/ptrack.db"
	s, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	setFormat(t, s, 1)
	_ = s.Close()

	s2, err := Open(path)
	if err != nil {
		t.Fatalf("reopen v1 db: %v", err)
	}
	defer s2.Close()
	m, _ := s2.GetMeta()
	if m.FormatVersion != CurrentFormat {
		t.Errorf("format = %d want %d", m.FormatVersion, CurrentFormat)
	}
	if _, err := s2.AddMilestone("works"); err != nil {
		t.Errorf("milestone bucket unusable after migration: %v", err)
	}
}
