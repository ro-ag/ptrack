package gui

import (
	"errors"
	"strings"
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

func TestGetTaskDetailV2ReturnsFullTaskContext(t *testing.T) {
	app := seedApp(t)
	s, err := store.Open(app.dbPath)
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	if _, err := s.AddNote(model.TargetTask, 1, "Second, newer decision."); err != nil {
		t.Fatalf("add second note: %v", err)
	}
	if err := s.Close(); err != nil {
		t.Fatalf("close store: %v", err)
	}

	detail, err := app.GetTaskDetailV2(1, 1)
	if err != nil {
		t.Fatalf("GetTaskDetailV2: %v", err)
	}
	if detail.Generation != 1 {
		t.Fatalf("generation = %d, want 1", detail.Generation)
	}
	if detail.Task.ID != 1 || detail.Task.Title != "Build cards" ||
		detail.Task.Status != string(model.TaskTodo) {
		t.Fatalf("task = %#v", detail.Task)
	}
	if detail.Task.NoteCount != 2 || detail.Task.CommitCount != 1 ||
		detail.Task.IssueCount != 1 {
		t.Fatalf("task counts = %#v", detail.Task)
	}

	if len(detail.Notes) != 2 {
		t.Fatalf("notes = %#v, want 2 entries", detail.Notes)
	}
	if detail.Notes[0].Body != "Second, newer decision." ||
		detail.Notes[1].Body != "Keep the card context concise." {
		t.Fatalf("notes not newest-first: %#v", detail.Notes)
	}
	if detail.Notes[0].ID == 0 || detail.Notes[0].OccurredAt == "" {
		t.Fatalf("note identity/timestamp missing: %#v", detail.Notes[0])
	}

	if len(detail.Commits) != 1 {
		t.Fatalf("commits = %#v, want 1 entry", detail.Commits)
	}
	commit := detail.Commits[0]
	if commit.SHA != "1234567890abcdef" || commit.Subject != "Render richer cards" ||
		commit.ID == 0 || commit.OccurredAt == "" {
		t.Fatalf("commit = %#v", commit)
	}

	if len(detail.Issues) != 1 {
		t.Fatalf("issues = %#v, want 1 entry", detail.Issues)
	}
	issue := detail.Issues[0]
	if issue.Title != "Card focus is too subtle" ||
		issue.Severity != string(model.SeverityHigh) || issue.TaskID != 1 {
		t.Fatalf("issue = %#v", issue)
	}
}

func TestGetTaskDetailV2SerializesEmptyCollections(t *testing.T) {
	app := seedApp(t)

	detail, err := app.GetTaskDetailV2(1, 2)
	if err != nil {
		t.Fatalf("GetTaskDetailV2: %v", err)
	}
	if detail.Notes == nil || detail.Commits == nil || detail.Issues == nil {
		t.Fatalf("empty collections must serialize as []: %#v", detail)
	}
	if len(detail.Notes) != 0 || len(detail.Commits) != 0 || len(detail.Issues) != 0 {
		t.Fatalf("task #2 detail = %#v, want no associations", detail)
	}
}

func TestGetTaskDetailV2RejectsUnknownTask(t *testing.T) {
	app := seedApp(t)

	_, err := app.GetTaskDetailV2(1, 99)
	if err == nil || !strings.Contains(err.Error(), "task #99 not found") {
		t.Fatalf("GetTaskDetailV2 unknown task = %v", err)
	}
}

func TestGetTaskDetailV2RejectsStaleGeneration(t *testing.T) {
	app := seedApp(t)

	if _, err := app.GetTaskDetailV2(2, 1); !errors.Is(err, errStaleWorkspaceGeneration) {
		t.Fatalf("GetTaskDetailV2 stale generation = %v", err)
	}
}
