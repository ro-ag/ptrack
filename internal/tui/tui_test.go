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
	d.width, d.height = 120, 40
	return d, s
}

func runes(s string) tea.KeyMsg    { return tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune(s)} }
func key(t tea.KeyType) tea.KeyMsg { return tea.KeyMsg{Type: t} }

func send(t *testing.T, d dashboard, msg tea.Msg) dashboard {
	t.Helper()
	m, _ := d.Update(msg)
	return m.(dashboard)
}

func typeAndEnter(t *testing.T, d dashboard, text string) dashboard {
	d = send(t, d, runes(text))
	return send(t, d, key(tea.KeyEnter))
}

func TestTabSwitching(t *testing.T) {
	d, _ := newTestModel(t)
	if d.tab != tabOverview {
		t.Fatal("default tab should be overview")
	}
	d = send(t, d, runes("3"))
	if d.tab != tabMilestones {
		t.Errorf("'3' should jump to milestones, got %v", d.tab)
	}
	d = send(t, d, key(tea.KeyTab))
	if d.tab != tabIssues {
		t.Errorf("tab should advance to issues, got %v", d.tab)
	}
	d = send(t, d, key(tea.KeyTab))
	if d.tab != tabOverview {
		t.Errorf("tab should wrap to overview, got %v", d.tab)
	}
}

func TestAddPlanAndTask(t *testing.T) {
	d, s := newTestModel(t)
	d = send(t, d, runes("a")) // overview/plans focus -> add plan
	d = typeAndEnter(t, d, "Storage")
	plans, _ := s.ListPlans()
	if len(plans) != 1 || plans[0].Title != "Storage" {
		t.Fatalf("plans = %+v", plans)
	}
	// switch to tasks pane, add a task
	d = send(t, d, runes("l")) // toggle pane
	if d.focus != focusTasks {
		t.Fatal("expected tasks focus")
	}
	d = send(t, d, runes("a"))
	d = typeAndEnter(t, d, "buckets")
	tasks, _ := s.ListTasksByPlan(plans[0].ID)
	if len(tasks) != 1 || tasks[0].Title != "buckets" {
		t.Fatalf("tasks = %+v", tasks)
	}
	// status change
	d = send(t, d, runes("s"))
	got, _ := s.GetTask(tasks[0].ID)
	if got.Status != model.TaskDoing {
		t.Errorf("status = %q want doing", got.Status)
	}
}

func TestAddMilestone(t *testing.T) {
	d, s := newTestModel(t)
	d = send(t, d, runes("3")) // milestones tab
	d = send(t, d, runes("a"))
	d = typeAndEnter(t, d, "v1.0")
	ms, _ := s.ListMilestones()
	if len(ms) != 1 || ms[0].Title != "v1.0" {
		t.Fatalf("milestones = %+v", ms)
	}
	send(t, d, runes("x")) // mark done
	got, _ := s.GetMilestone(ms[0].ID)
	if got.Status != model.MilestoneDone {
		t.Errorf("status = %q want done", got.Status)
	}
}

func TestAddIssueAndClose(t *testing.T) {
	d, s := newTestModel(t)
	d = send(t, d, runes("4")) // issues tab
	d = send(t, d, runes("a"))
	d = typeAndEnter(t, d, "crash")
	issues, _ := s.ListIssues()
	if len(issues) != 1 || issues[0].Title != "crash" {
		t.Fatalf("issues = %+v", issues)
	}
	send(t, d, runes("c")) // close
	got, _ := s.GetIssue(issues[0].ID)
	if got.Status != model.IssueClosed {
		t.Errorf("status = %q want closed", got.Status)
	}
}

func TestBoardMoveCard(t *testing.T) {
	d, s := newTestModel(t)
	p, _ := s.AddPlan("P")
	tk, _ := s.AddTask(p.ID, "card") // todo
	_ = d.reload()

	d = send(t, d, runes("2")) // board tab
	d = send(t, d, runes("L")) // move right todo->doing
	got, _ := s.GetTask(tk.ID)
	if got.Status != model.TaskDoing {
		t.Fatalf("status = %q want doing", got.Status)
	}
	if d.boardCol != 1 {
		t.Errorf("boardCol = %d want 1", d.boardCol)
	}
}

func TestEditGoal(t *testing.T) {
	d, s := newTestModel(t)
	d = send(t, d, runes("g"))
	typeAndEnter(t, d, "New Goal")
	m, _ := s.GetMeta()
	if m.Goal != "New Goal" {
		t.Errorf("goal = %q", m.Goal)
	}
}

func TestViewRendersWithoutPanic(t *testing.T) {
	d, s := newTestModel(t)
	m, _ := s.AddMilestone("v1")
	p, _ := s.AddPlan("plan")
	s.SetPlanMilestone(p.ID, m.ID)
	s.AddTask(p.ID, "t1")
	s.AddIssue("bug", "", model.SeverityHigh, 0)
	_ = d.reload()
	for _, tb := range []tab{tabOverview, tabBoard, tabMilestones, tabIssues} {
		d.tab = tb
		if got := d.View(); got == "" {
			t.Errorf("empty view for tab %v", tb)
		}
	}
}

func TestRenamePlanViaKeys(t *testing.T) {
	d, s := newTestModel(t)
	p, _ := s.AddPlan("Pending: reducer")
	_ = d.reload()
	d = send(t, d, runes("e")) // rename selected plan
	if d.purpose != inputRename {
		t.Fatalf("purpose = %v want inputRename", d.purpose)
	}
	// clear + type new title
	for range "Pending: reducer" {
		d = send(t, d, key(tea.KeyBackspace))
	}
	d = typeAndEnter(t, d, "reducer")
	got, _ := s.GetPlan(p.ID)
	if got.Title != "reducer" {
		t.Errorf("title = %q want reducer", got.Title)
	}
}
