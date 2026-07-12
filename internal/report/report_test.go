package report

import (
	"encoding/json"
	"path/filepath"
	"strings"
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

func seed(t *testing.T) *store.Store {
	t.Helper()
	s, err := store.Open(filepath.Join(t.TempDir(), "ptrack.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = s.Close() })
	s.SetGoal("Ship the widget service")
	s.SetSummary("Storage layer landed; wiring CLI")
	p, _ := s.AddPlan("Build CLI")
	s.SetActivePlan(p.ID)
	doing, _ := s.AddTask(p.ID, "context command")
	s.SetTaskStatus(doing.ID, model.TaskDoing)
	done, _ := s.AddTask(p.ID, "init command")
	s.SetTaskStatus(done.ID, model.TaskDone)
	blocked, _ := s.AddTask(p.ID, "publish release")
	s.SetTaskStatus(blocked.ID, model.TaskBlocked)
	s.AddNote(model.TargetProject, 0, "decided bbolt over badger")
	return s
}

func TestContextMarkdown(t *testing.T) {
	d, err := Context(seed(t))
	if err != nil {
		t.Fatal(err)
	}
	md := d.Markdown()
	for _, w := range []string{
		"Ship the widget service", "Storage layer landed", "Build CLI",
		"context command", "publish release", "decided bbolt over badger",
		"Blocked (project-wide)", "Inventory", "Drill deeper",
	} {
		if !strings.Contains(md, w) {
			t.Errorf("context markdown missing %q\n---\n%s", w, md)
		}
	}
	if strings.Contains(md, "init command") {
		t.Errorf("done task 'init command' should not appear in open tasks\n%s", md)
	}
}

func TestContextJSONShapeAndInventory(t *testing.T) {
	d, err := Context(seed(t))
	if err != nil {
		t.Fatal(err)
	}
	if d.Inventory.Tasks != 3 || d.Inventory.TasksDone != 1 || d.Inventory.TasksBlocked != 1 || d.Inventory.TasksOpen != 2 {
		t.Errorf("inventory = %+v", d.Inventory)
	}
	b, err := json.Marshal(d)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(b), `"inventory"`) || !strings.Contains(string(b), `"active_plan"`) {
		t.Errorf("json missing keys: %s", b)
	}
}

func TestNext(t *testing.T) {
	s := seed(t)
	v, err := Next(s)
	if err != nil {
		t.Fatal(err)
	}
	// doing beats todo/blocked: "context command" is doing.
	if v.Task == nil || v.Task.Title != "context command" {
		t.Fatalf("next = %+v want 'context command'", v.Task)
	}
	if !strings.Contains(v.Markdown(), "context command") {
		t.Errorf("next markdown: %s", v.Markdown())
	}
}

func TestShowPlanAndTask(t *testing.T) {
	s := seed(t)
	pv, err := ShowPlan(s, 1)
	if err != nil {
		t.Fatal(err)
	}
	if pv.Plan.Title != "Build CLI" || len(pv.Tasks) != 3 {
		t.Errorf("plan show = %+v", pv)
	}
	tv, err := ShowTask(s, 1)
	if err != nil {
		t.Fatal(err)
	}
	if tv.Plan == nil || tv.Plan.Title != "Build CLI" {
		t.Errorf("task show parent plan = %+v", tv.Plan)
	}
}

func TestSearch(t *testing.T) {
	v, err := Search(seed(t), "command")
	if err != nil {
		t.Fatal(err)
	}
	// matches task titles "context command" and "init command".
	if len(v.Tasks) != 2 {
		t.Errorf("search tasks = %d want 2 (%+v)", len(v.Tasks), v.Tasks)
	}
	v2, _ := Search(seed(t), "badger")
	if len(v2.Notes) != 1 {
		t.Errorf("search notes = %d want 1", len(v2.Notes))
	}
	v3, _ := Search(seed(t), "zzz-no-match")
	if !strings.Contains(v3.Markdown(), "no matches") {
		t.Errorf("expected no-matches message: %s", v3.Markdown())
	}
}

func TestBoard(t *testing.T) {
	b, err := BoardFor(seed(t), 1)
	if err != nil {
		t.Fatal(err)
	}
	if len(b.Todo) != 0 || len(b.Doing) != 1 || len(b.Blocked) != 1 || len(b.Done) != 1 {
		t.Errorf("board columns wrong: %+v", b)
	}
	md := b.Markdown()
	for _, w := range []string{"Todo (0)", "Doing (1)", "Blocked (1)", "Done (1)"} {
		if !strings.Contains(md, w) {
			t.Errorf("board markdown missing %q\n%s", w, md)
		}
	}
}
