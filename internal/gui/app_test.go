package gui

import (
	"path/filepath"
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

func seedApp(t *testing.T) *App {
	t.Helper()
	dbPath := filepath.Join(t.TempDir(), "ptrack.db")
	s, err := store.Open(dbPath)
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer s.Close()
	if err := s.SetGoal("Ship the GUI"); err != nil {
		t.Fatalf("set goal: %v", err)
	}
	if err := s.SetSummary("Backend bindings are ready for visual polish."); err != nil {
		t.Fatalf("set summary: %v", err)
	}
	p, err := s.AddPlan("Desktop board")
	if err != nil {
		t.Fatalf("add plan: %v", err)
	}
	if err := s.SetActivePlan(p.ID); err != nil {
		t.Fatalf("set active plan: %v", err)
	}
	todo, err := s.AddTask(p.ID, "Build cards")
	if err != nil {
		t.Fatalf("add todo: %v", err)
	}
	doing, err := s.AddTask(p.ID, "Wire backend")
	if err != nil {
		t.Fatalf("add doing: %v", err)
	}
	if err := s.SetTaskStatus(doing.ID, model.TaskDoing); err != nil {
		t.Fatalf("start task: %v", err)
	}
	if _, err := s.AddNote(model.TargetTask, todo.ID, "Keep the card context concise."); err != nil {
		t.Fatalf("add note: %v", err)
	}
	if _, err := s.AddCommit("1234567890abcdef", "Render richer cards", p.ID, todo.ID); err != nil {
		t.Fatalf("add commit: %v", err)
	}
	if _, err := s.AddIssue("Card focus is too subtle", "", model.SeverityHigh, todo.ID); err != nil {
		t.Fatalf("add issue: %v", err)
	}
	return newApp(dbPath, 0)
}

func TestGetBoardGroupsTasks(t *testing.T) {
	app := seedApp(t)
	board, err := app.GetBoard(0)
	if err != nil {
		t.Fatalf("GetBoard: %v", err)
	}
	if board.Goal != "Ship the GUI" || board.Summary == "" || board.PlanTitle != "Desktop board" {
		t.Fatalf("unexpected board: %#v", board)
	}
	if len(board.Columns) != 4 {
		t.Fatalf("columns = %d, want 4", len(board.Columns))
	}
	if len(board.Columns[0].Tasks) != 1 || board.Columns[0].Tasks[0].Title != "Build cards" {
		t.Fatalf("todo column = %#v", board.Columns[0].Tasks)
	}
	todo := board.Columns[0].Tasks[0]
	if todo.NoteCount != 1 || todo.CommitCount != 1 || todo.IssueCount != 1 || todo.LatestNote == "" {
		t.Fatalf("todo context = %#v", todo)
	}
	if len(board.Columns[1].Tasks) != 1 || board.Columns[1].Tasks[0].Title != "Wire backend" {
		t.Fatalf("doing column = %#v", board.Columns[1].Tasks)
	}
	if len(board.Activity) != 2 || len(board.OpenIssues) != 1 {
		t.Fatalf("memory context = activity %#v, issues %#v", board.Activity, board.OpenIssues)
	}
}

func TestBoardMutations(t *testing.T) {
	app := seedApp(t)
	board, err := app.GetBoard(0)
	if err != nil {
		t.Fatalf("GetBoard: %v", err)
	}
	added, err := app.AddTask(board.PlanID, "  Polish board  ")
	if err != nil {
		t.Fatalf("AddTask: %v", err)
	}
	if err := app.RenameTask(added.ID, "Polish desktop board"); err != nil {
		t.Fatalf("RenameTask: %v", err)
	}
	if err := app.MoveTask(added.ID, "done"); err != nil {
		t.Fatalf("MoveTask: %v", err)
	}
	if err := app.AddTaskNote(added.ID, "Verified through the desktop board."); err != nil {
		t.Fatalf("AddTaskNote: %v", err)
	}
	refreshed, err := app.GetBoard(board.PlanID)
	if err != nil {
		t.Fatalf("GetBoard after mutations: %v", err)
	}
	done := refreshed.Columns[3].Tasks
	if len(done) != 1 || done[0].Title != "Polish desktop board" || done[0].NoteCount != 1 {
		t.Fatalf("done column = %#v", done)
	}
}

func TestBoardRejectsInvalidInput(t *testing.T) {
	app := seedApp(t)
	if _, err := app.AddTask(1, " "); err == nil {
		t.Fatal("AddTask accepted an empty title")
	}
	if err := app.MoveTask(1, "unknown"); err == nil {
		t.Fatal("MoveTask accepted an invalid status")
	}
	if err := app.AddTaskNote(1, " "); err == nil {
		t.Fatal("AddTaskNote accepted an empty note")
	}
}
