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
	open, _ := s.AddTask(p.ID, "context command")
	s.SetTaskStatus(open.ID, model.TaskDoing)
	done, _ := s.AddTask(p.ID, "init command")
	s.SetTaskStatus(done.ID, model.TaskDone)
	s.AddNote(model.TargetProject, 0, "decided bbolt over badger")
	return s
}

func TestMarkdown(t *testing.T) {
	md, err := Markdown(seed(t))
	if err != nil {
		t.Fatal(err)
	}
	wantContains := []string{
		"Ship the widget service",
		"Storage layer landed",
		"Build CLI",
		"context command",
		"decided bbolt over badger",
	}
	for _, w := range wantContains {
		if !strings.Contains(md, w) {
			t.Errorf("markdown missing %q\n---\n%s", w, md)
		}
	}
	if strings.Contains(md, "init command") {
		t.Errorf("markdown should exclude done task 'init command'\n---\n%s", md)
	}
}

func TestJSON(t *testing.T) {
	data, err := JSON(seed(t))
	if err != nil {
		t.Fatal(err)
	}
	var d struct {
		Goal       string `json:"goal"`
		ActivePlan *struct {
			Title string `json:"title"`
			Tasks []struct {
				Title  string `json:"title"`
				Status string `json:"status"`
			} `json:"open_tasks"`
		} `json:"active_plan"`
		Notes []struct {
			Body string `json:"body"`
		} `json:"recent_notes"`
	}
	if err := json.Unmarshal(data, &d); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if d.Goal != "Ship the widget service" {
		t.Errorf("goal = %q", d.Goal)
	}
	if d.ActivePlan == nil || d.ActivePlan.Title != "Build CLI" {
		t.Fatalf("active plan wrong: %+v", d.ActivePlan)
	}
	if len(d.ActivePlan.Tasks) != 1 || d.ActivePlan.Tasks[0].Status != "doing" {
		t.Errorf("open tasks wrong: %+v", d.ActivePlan.Tasks)
	}
	if len(d.Notes) != 1 || d.Notes[0].Body != "decided bbolt over badger" {
		t.Errorf("notes wrong: %+v", d.Notes)
	}
}
