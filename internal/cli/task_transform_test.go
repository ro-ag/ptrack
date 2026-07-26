package cli

import (
	"errors"
	"strings"
	"testing"

	"github.com/ro-ag/ptrack/internal/store"
)

func TestTaskMoveAndConvertCommands(t *testing.T) {
	seedProject(t)
	mustRun(t, "plan", "add", "Destination")

	out := mustRun(t, "task", "move", "2", "--plan", "2")
	if !strings.Contains(out, "task #2 moved to plan 2") {
		t.Errorf("move output = %q", out)
	}

	s := openTestStore(t)
	task, err := s.GetTask(2)
	if err != nil {
		t.Fatal(err)
	}
	if task.PlanID != 2 {
		t.Errorf("task plan = %d, want 2", task.PlanID)
	}
	_ = s.Close()

	out = mustRun(t, "task", "convert", "2")
	if !strings.Contains(out, "task #2 converted to plan #3 crud") {
		t.Errorf("convert output = %q", out)
	}

	s = openTestStore(t)
	defer s.Close()
	if _, err := s.GetTask(2); !errors.Is(err, store.ErrNotFound) {
		t.Errorf("converted task still exists: %v", err)
	}
	plan, err := s.GetPlan(3)
	if err != nil {
		t.Fatal(err)
	}
	if plan.Title != "crud" {
		t.Errorf("plan title = %q, want crud", plan.Title)
	}
}

func TestTaskMoveRequiresExistingTargetPlan(t *testing.T) {
	seedProject(t)
	if _, err := runCmd(t, "task", "move", "1", "--plan", "404"); !errors.Is(err, store.ErrNotFound) {
		t.Errorf("move error = %v, want ErrNotFound", err)
	}

	s := openTestStore(t)
	defer s.Close()
	task, err := s.GetTask(1)
	if err != nil {
		t.Fatal(err)
	}
	if task.PlanID != 1 {
		t.Errorf("failed move changed task plan to %d", task.PlanID)
	}
}
