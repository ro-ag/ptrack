package tui

import (
	"path/filepath"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

func newTestModel(t *testing.T) (dashboard, *store.Store) {
	t.Helper()
	s, err := store.Open(filepath.Join(t.TempDir(), "ptrack.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = s.Close() })
	d, err := newModel(s, "unused")
	if err != nil {
		t.Fatal(err)
	}
	return d, s
}

func runes(s string) tea.KeyMsg { return tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune(s)} }

func send(t *testing.T, d dashboard, msg tea.Msg) dashboard {
	t.Helper()
	m, _ := d.Update(msg)
	return m.(dashboard)
}

func TestAddPlanViaKeys(t *testing.T) {
	d, s := newTestModel(t)
	d = send(t, d, runes("a")) // focusPlans -> add plan
	if d.purpose != inputAddPlan {
		t.Fatalf("purpose = %v want inputAddPlan", d.purpose)
	}
	d = send(t, d, runes("Alpha"))
	d = send(t, d, tea.KeyMsg{Type: tea.KeyEnter})

	plans, _ := s.ListPlans()
	if len(plans) != 1 || plans[0].Title != "Alpha" {
		t.Fatalf("plans = %+v want one 'Alpha'", plans)
	}
}

func TestAddTaskAndStatusViaKeys(t *testing.T) {
	d, s := newTestModel(t)
	p, _ := s.AddPlan("P")
	_ = d.reload()

	d = send(t, d, tea.KeyMsg{Type: tea.KeyTab}) // focus tasks
	if d.focus != focusTasks {
		t.Fatal("tab did not switch focus to tasks")
	}
	d = send(t, d, runes("a")) // add task
	d = send(t, d, runes("build it"))
	d = send(t, d, tea.KeyMsg{Type: tea.KeyEnter})

	tasks, _ := s.ListTasksByPlan(p.ID)
	if len(tasks) != 1 || tasks[0].Title != "build it" {
		t.Fatalf("tasks = %+v want one 'build it'", tasks)
	}

	d = send(t, d, runes("s")) // start -> doing
	got, _ := s.GetTask(tasks[0].ID)
	if got.Status != model.TaskDoing {
		t.Errorf("status = %q want doing", got.Status)
	}
	d = send(t, d, runes("d")) // done
	got, _ = s.GetTask(tasks[0].ID)
	if got.Status != model.TaskDone {
		t.Errorf("status = %q want done", got.Status)
	}
}

func TestEditGoalViaKeys(t *testing.T) {
	d, s := newTestModel(t)
	d = send(t, d, runes("g"))
	d = send(t, d, runes("New Goal"))
	send(t, d, tea.KeyMsg{Type: tea.KeyEnter})

	m, _ := s.GetMeta()
	if m.Goal != "New Goal" {
		t.Errorf("goal = %q want 'New Goal'", m.Goal)
	}
}

func TestSetActivePlanViaKeys(t *testing.T) {
	d, s := newTestModel(t)
	p, _ := s.AddPlan("P")
	_ = d.reload()
	d = send(t, d, runes("u")) // set active
	m, _ := s.GetMeta()
	if m.ActivePlan != p.ID {
		t.Errorf("active plan = %d want %d", m.ActivePlan, p.ID)
	}
}

func TestBoardModeMoveCard(t *testing.T) {
	d, s := newTestModel(t)
	p, _ := s.AddPlan("P")
	tk, _ := s.AddTask(p.ID, "card") // todo
	_ = d.reload()

	d = send(t, d, runes("v")) // enter board
	if d.mode != modeBoard {
		t.Fatal("v did not enter board mode")
	}
	// card is in the todo column (col 0); move it right to doing.
	d = send(t, d, runes("L"))
	got, _ := s.GetTask(tk.ID)
	if got.Status != model.TaskDoing {
		t.Fatalf("after move-right status = %q want doing", got.Status)
	}
	if d.boardCol != 1 {
		t.Errorf("boardCol = %d want 1", d.boardCol)
	}
	// move right again -> blocked.
	d = send(t, d, runes("L"))
	got, _ = s.GetTask(tk.ID)
	if got.Status != model.TaskBlocked {
		t.Errorf("status = %q want blocked", got.Status)
	}
	// back to list.
	d = send(t, d, runes("v"))
	if d.mode != modeList {
		t.Errorf("v did not return to list mode")
	}
}

func TestCancelInput(t *testing.T) {
	d, _ := newTestModel(t)
	d = send(t, d, runes("a"))
	d = send(t, d, tea.KeyMsg{Type: tea.KeyEsc})
	if d.purpose != inputNone {
		t.Errorf("purpose = %v want inputNone after esc", d.purpose)
	}
}
