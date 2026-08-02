package gui

import (
	"errors"
	"testing"
)

func TestSearchV2MatchesPlansTasksAndNotes(t *testing.T) {
	app := seedApp(t)

	planResults, err := app.SearchV2("board")
	if err != nil {
		t.Fatalf("SearchV2: %v", err)
	}
	if len(planResults) != 1 || planResults[0].Kind != "plan" ||
		planResults[0].ID != 1 || planResults[0].PlanID != 1 ||
		planResults[0].Title != "Desktop board" || planResults[0].Snippet != "" {
		t.Fatalf("plan results = %#v", planResults)
	}

	// "card" hits one task title and one note body; tasks come before notes.
	results, err := app.SearchV2("card")
	if err != nil {
		t.Fatalf("SearchV2: %v", err)
	}
	if len(results) != 2 {
		t.Fatalf("results = %#v, want the task and note hits", results)
	}
	if results[0].Kind != "task" || results[0].PlanID != 1 ||
		results[0].Title != "Build cards" || results[0].Snippet != "" {
		t.Fatalf("task result = %#v", results[0])
	}
	if results[1].Kind != "note" || results[1].ID != 1 || results[1].Snippet == "" {
		t.Fatalf("note result = %#v, want a snippet", results[1])
	}
}

func TestSearchV2IsCaseInsensitive(t *testing.T) {
	app := seedApp(t)
	results, err := app.SearchV2("KEEP THE CARD")
	if err != nil {
		t.Fatalf("SearchV2: %v", err)
	}
	if len(results) != 1 || results[0].Kind != "note" {
		t.Fatalf("results = %#v, want the note body match", results)
	}
}

func TestSearchV2EmptyQueryReturnsEmptySlice(t *testing.T) {
	app := seedApp(t)
	for _, query := range []string{"", "   "} {
		results, err := app.SearchV2(query)
		if err != nil {
			t.Fatalf("SearchV2(%q): %v", query, err)
		}
		if results == nil || len(results) != 0 {
			t.Fatalf("SearchV2(%q) = %#v, want an empty slice", query, results)
		}
	}
}

func TestSearchV2RequiresOpenWorkspace(t *testing.T) {
	app := newWorkspaceCoordinator(nil, nil)
	if _, err := app.SearchV2("anything"); !errors.Is(err, errNoWorkspace) {
		t.Fatalf("SearchV2 without project = %v, want errNoWorkspace", err)
	}
}

func TestSearchSnippetCentersOnMatch(t *testing.T) {
	body := "prefix context before the needle and a long tail of trailing context that keeps going past the excerpt window"
	snippet := searchSnippet(body, "needle")
	if snippet == body || snippet == "" {
		t.Fatalf("snippet = %q, want a truncated excerpt", snippet)
	}
	if got := searchSnippet(body, "missing"); got != "" {
		t.Fatalf("snippet for missing needle = %q, want empty", got)
	}
	short := searchSnippet("short body", "short")
	if short != "short body" {
		t.Fatalf("snippet for short body = %q, want the whole body", short)
	}
}
