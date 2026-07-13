package store

import (
	"path/filepath"
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
)

func openTemp(t *testing.T) *Store {
	t.Helper()
	s, err := Open(filepath.Join(t.TempDir(), "ptrack.db"))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

func TestMetaLifecycle(t *testing.T) {
	s := openTemp(t)
	m, err := s.GetMeta()
	if err != nil {
		t.Fatal(err)
	}
	if m.CreatedAt.IsZero() {
		t.Error("CreatedAt should be set on open")
	}
	if err := s.SetGoal("ship widget"); err != nil {
		t.Fatal(err)
	}
	if err := s.SetSummary("phase 1"); err != nil {
		t.Fatal(err)
	}
	m, _ = s.GetMeta()
	if m.Goal != "ship widget" || m.Summary != "phase 1" {
		t.Errorf("meta not persisted: %+v", m)
	}
}

func TestPlanCRUD(t *testing.T) {
	s := openTemp(t)
	p1, err := s.AddPlan("storage")
	if err != nil {
		t.Fatal(err)
	}
	p2, _ := s.AddPlan("cli")
	if p1.ID == p2.ID {
		t.Fatal("ids must be unique")
	}
	if p1.Status != model.PlanActive {
		t.Errorf("new plan status = %q want active", p1.Status)
	}
	plans, _ := s.ListPlans()
	if len(plans) != 2 || plans[0].ID != p1.ID || plans[1].ID != p2.ID {
		t.Errorf("ListPlans order wrong: %+v", plans)
	}
	if err := s.SetPlanStatus(p1.ID, model.PlanDone); err != nil {
		t.Fatal(err)
	}
	got, _ := s.GetPlan(p1.ID)
	if got.Status != model.PlanDone {
		t.Errorf("status = %q want done", got.Status)
	}
	if _, err := s.GetPlan(999); err != ErrNotFound {
		t.Errorf("want ErrNotFound, got %v", err)
	}
}

func TestActivePlanRequiresExisting(t *testing.T) {
	s := openTemp(t)
	if err := s.SetActivePlan(42); err != ErrNotFound {
		t.Errorf("want ErrNotFound for missing plan, got %v", err)
	}
	p, _ := s.AddPlan("x")
	if err := s.SetActivePlan(p.ID); err != nil {
		t.Fatal(err)
	}
	m, _ := s.GetMeta()
	if m.ActivePlan != p.ID {
		t.Errorf("active plan = %d want %d", m.ActivePlan, p.ID)
	}
}

func TestTaskCRUD(t *testing.T) {
	s := openTemp(t)
	if _, err := s.AddTask(1, "orphan"); err != ErrNotFound {
		t.Errorf("adding task to missing plan should fail, got %v", err)
	}
	p, _ := s.AddPlan("plan")
	t1, _ := s.AddTask(p.ID, "a")
	t2, _ := s.AddTask(p.ID, "b")
	if t1.Status != model.TaskTodo {
		t.Errorf("new task status = %q want todo", t1.Status)
	}
	tasks, _ := s.ListTasksByPlan(p.ID)
	if len(tasks) != 2 || tasks[0].ID != t1.ID || tasks[1].ID != t2.ID {
		t.Errorf("task order wrong: %+v", tasks)
	}
	if err := s.SetTaskStatus(t1.ID, model.TaskDoing); err != nil {
		t.Fatal(err)
	}
	got, _ := s.GetTask(t1.ID)
	if got.Status != model.TaskDoing {
		t.Errorf("status = %q want doing", got.Status)
	}

	p2, _ := s.AddPlan("other")
	s.AddTask(p2.ID, "c")
	all, _ := s.ListTasks()
	if len(all) != 3 {
		t.Errorf("ListTasks = %d want 3", len(all))
	}
	only, _ := s.ListTasksByPlan(p.ID)
	if len(only) != 2 {
		t.Errorf("ListTasksByPlan = %d want 2", len(only))
	}
}

func TestNotes(t *testing.T) {
	s := openTemp(t)
	s.AddNote(model.TargetProject, 0, "first")
	s.AddNote(model.TargetPlan, 1, "second")
	s.AddNote(model.TargetTask, 2, "third")
	all, _ := s.ListNotes()
	if len(all) != 3 || all[0].Body != "first" || all[2].Body != "third" {
		t.Errorf("ListNotes order wrong: %+v", all)
	}
	recent, _ := s.RecentNotes(2)
	if len(recent) != 2 || recent[0].Body != "third" || recent[1].Body != "second" {
		t.Errorf("RecentNotes wrong: %+v", recent)
	}
	if got, _ := s.RecentNotes(0); len(got) != 3 {
		t.Errorf("RecentNotes(0) = %d want all 3", len(got))
	}
}

func TestRenameSetters(t *testing.T) {
	s := openTemp(t)
	p, _ := s.AddPlan("old plan")
	tk, _ := s.AddTask(p.ID, "old task")
	if err := s.SetPlanTitle(p.ID, "new plan"); err != nil {
		t.Fatal(err)
	}
	if err := s.SetTaskTitle(tk.ID, "new task"); err != nil {
		t.Fatal(err)
	}
	gp, _ := s.GetPlan(p.ID)
	gt, _ := s.GetTask(tk.ID)
	if gp.Title != "new plan" || gt.Title != "new task" {
		t.Errorf("rename failed: %q / %q", gp.Title, gt.Title)
	}
	if err := s.SetPlanTitle(999, "x"); err != ErrNotFound {
		t.Errorf("rename missing plan want ErrNotFound, got %v", err)
	}
}
