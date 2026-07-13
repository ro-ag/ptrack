package tui

import (
	"path/filepath"
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

// newTestModel creates an initialized project and a model over it, without
// holding the store open (the model opens transiently).
func newTestModel(t *testing.T) (dashboard, string) {
	t.Helper()
	dbPath := filepath.Join(t.TempDir(), "ptrack.db")
	s, err := store.Open(dbPath)
	if err != nil {
		t.Fatal(err)
	}
	_ = s.Close()
	d, err := newModel(dbPath)
	if err != nil {
		t.Fatal(err)
	}
	d.width, d.height = 120, 40
	return d, dbPath
}

// withStore opens the project store transiently for setup or assertions.
func withStore(t *testing.T, dbPath string, fn func(*store.Store)) {
	t.Helper()
	s, err := store.Open(dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	fn(s)
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
	d, dbPath := newTestModel(t)
	d = send(t, d, runes("a"))
	d = typeAndEnter(t, d, "Storage")

	var planID uint64
	withStore(t, dbPath, func(s *store.Store) {
		plans, _ := s.ListPlans()
		if len(plans) != 1 || plans[0].Title != "Storage" {
			t.Fatalf("plans = %+v", plans)
		}
		planID = plans[0].ID
	})

	d = send(t, d, runes("l"))
	if d.focus != focusTasks {
		t.Fatal("expected tasks focus")
	}
	d = send(t, d, runes("a"))
	d = typeAndEnter(t, d, "buckets")

	var taskID uint64
	withStore(t, dbPath, func(s *store.Store) {
		tasks, _ := s.ListTasksByPlan(planID)
		if len(tasks) != 1 || tasks[0].Title != "buckets" {
			t.Fatalf("tasks = %+v", tasks)
		}
		taskID = tasks[0].ID
	})

	d = send(t, d, runes("s"))
	withStore(t, dbPath, func(s *store.Store) {
		got, _ := s.GetTask(taskID)
		if got.Status != model.TaskDoing {
			t.Errorf("status = %q want doing", got.Status)
		}
	})
}

func TestAddMilestone(t *testing.T) {
	d, dbPath := newTestModel(t)
	d = send(t, d, runes("3"))
	d = send(t, d, runes("a"))
	d = typeAndEnter(t, d, "v1.0")

	var id uint64
	withStore(t, dbPath, func(s *store.Store) {
		ms, _ := s.ListMilestones()
		if len(ms) != 1 || ms[0].Title != "v1.0" {
			t.Fatalf("milestones = %+v", ms)
		}
		id = ms[0].ID
	})
	send(t, d, runes("x"))
	withStore(t, dbPath, func(s *store.Store) {
		got, _ := s.GetMilestone(id)
		if got.Status != model.MilestoneDone {
			t.Errorf("status = %q want done", got.Status)
		}
	})
}

func TestAddIssueAndClose(t *testing.T) {
	d, dbPath := newTestModel(t)
	d = send(t, d, runes("4"))
	d = send(t, d, runes("a"))
	d = typeAndEnter(t, d, "crash")

	var id uint64
	withStore(t, dbPath, func(s *store.Store) {
		issues, _ := s.ListIssues()
		if len(issues) != 1 || issues[0].Title != "crash" {
			t.Fatalf("issues = %+v", issues)
		}
		id = issues[0].ID
	})
	send(t, d, runes("c"))
	withStore(t, dbPath, func(s *store.Store) {
		got, _ := s.GetIssue(id)
		if got.Status != model.IssueClosed {
			t.Errorf("status = %q want closed", got.Status)
		}
	})
}

func TestBoardMoveCard(t *testing.T) {
	d, dbPath := newTestModel(t)
	var taskID uint64
	withStore(t, dbPath, func(s *store.Store) {
		p, _ := s.AddPlan("P")
		tk, _ := s.AddTask(p.ID, "card")
		taskID = tk.ID
	})
	_ = d.reload()

	d = send(t, d, runes("2"))
	d = send(t, d, runes("L"))
	withStore(t, dbPath, func(s *store.Store) {
		got, _ := s.GetTask(taskID)
		if got.Status != model.TaskDoing {
			t.Fatalf("status = %q want doing", got.Status)
		}
	})
	if d.boardCol != 1 {
		t.Errorf("boardCol = %d want 1", d.boardCol)
	}
}

func TestEditGoal(t *testing.T) {
	d, dbPath := newTestModel(t)
	d = send(t, d, runes("g"))
	typeAndEnter(t, d, "New Goal")
	withStore(t, dbPath, func(s *store.Store) {
		m, _ := s.GetMeta()
		if m.Goal != "New Goal" {
			t.Errorf("goal = %q", m.Goal)
		}
	})
}

func TestRenamePlanViaKeys(t *testing.T) {
	d, dbPath := newTestModel(t)
	var planID uint64
	withStore(t, dbPath, func(s *store.Store) {
		p, _ := s.AddPlan("Pending: reducer")
		planID = p.ID
	})
	_ = d.reload()
	d = send(t, d, runes("e"))
	if d.purpose != inputRename {
		t.Fatalf("purpose = %v want inputRename", d.purpose)
	}
	for range "Pending: reducer" {
		d = send(t, d, key(tea.KeyBackspace))
	}
	typeAndEnter(t, d, "reducer")
	withStore(t, dbPath, func(s *store.Store) {
		got, _ := s.GetPlan(planID)
		if got.Title != "reducer" {
			t.Errorf("title = %q want reducer", got.Title)
		}
	})
}

func TestDetailShowsNotes(t *testing.T) {
	d, dbPath := newTestModel(t)
	withStore(t, dbPath, func(s *store.Store) {
		p, _ := s.AddPlan("plan")
		tk, _ := s.AddTask(p.ID, "task")
		s.AddNote(model.TargetTask, tk.ID, "agent decided to use X")
	})
	_ = d.reload()
	d = send(t, d, runes("l"))
	d = send(t, d, key(tea.KeyEnter))
	if !d.showDetail {
		t.Fatal("enter did not open detail")
	}
	joined := strings.Join(d.detailLines, "\n")
	if !strings.Contains(joined, "agent decided to use X") {
		t.Errorf("detail missing note:\n%s", joined)
	}
	d = send(t, d, key(tea.KeyEsc))
	if d.showDetail {
		t.Error("esc did not close detail")
	}
}

func TestViewRendersWithoutPanic(t *testing.T) {
	d, dbPath := newTestModel(t)
	withStore(t, dbPath, func(s *store.Store) {
		m, _ := s.AddMilestone("v1")
		p, _ := s.AddPlan("plan")
		s.SetPlanMilestone(p.ID, m.ID)
		s.AddTask(p.ID, "t1")
		s.AddIssue("bug", "", model.SeverityHigh, 0)
	})
	_ = d.reload()
	for _, tb := range []tab{tabOverview, tabBoard, tabMilestones, tabIssues} {
		d.tab = tb
		if got := d.View(); got == "" {
			t.Errorf("empty view for tab %v", tb)
		}
	}
}

// TestIdleModelHoldsNoLock is the whole point: while the dashboard is alive but
// idle, the database is not locked, so an agent (another handle) can open it.
func TestIdleModelHoldsNoLock(t *testing.T) {
	d, dbPath := newTestModel(t)
	_ = d // model is alive and idle

	// A concurrent open must succeed immediately (not block on a lock).
	s, err := store.Open(dbPath)
	if err != nil {
		t.Fatalf("db is locked while the TUI is idle: %v", err)
	}
	// And that concurrent handle can write while the model exists.
	if _, err := s.AddPlan("agent-added"); err != nil {
		t.Errorf("concurrent write failed: %v", err)
	}
	_ = s.Close()

	// The model still refreshes fine afterward.
	if err := d.reload(); err != nil {
		t.Errorf("reload after concurrent write failed: %v", err)
	}
	if len(d.plans) != 1 {
		t.Errorf("model did not see the concurrently-added plan: %d plans", len(d.plans))
	}
}
