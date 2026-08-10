package gui

import (
	"strings"

	"github.com/ro-ag/ptrack/internal/model"
)

const (
	searchResultLimit = 50
	searchSnippetSpan = 60
)

// SearchResultV2 is one palette hit across plans, tasks, and notes.
type SearchResultV2 struct {
	Kind    string `json:"kind"` // "plan" | "task" | "note"
	ID      uint64 `json:"id"`
	PlanID  uint64 `json:"planId"`
	Title   string `json:"title"`
	Snippet string `json:"snippet"` // short context excerpt for notes; empty for plans/tasks
}

// SearchV2 runs a case-insensitive substring search across plan titles,
// task titles, and note bodies in the CURRENT workspace. Empty/whitespace
// query returns an empty slice (not an error). Results are capped, plans
// first, then tasks, then notes, to keep the palette snappy.
func (a *App) SearchV2(query string) ([]SearchResultV2, error) {
	needle := strings.ToLower(strings.TrimSpace(query))
	results := []SearchResultV2{}
	if needle == "" {
		return results, nil
	}
	s, _, release, err := a.openWorkspace(0)
	if err != nil {
		return nil, err
	}
	defer release()
	defer s.Close()

	has := func(haystack string) bool {
		return strings.Contains(strings.ToLower(haystack), needle)
	}
	plans, err := s.ListPlans()
	if err != nil {
		return nil, err
	}
	for _, plan := range plans {
		if len(results) >= searchResultLimit {
			return results, nil
		}
		if has(plan.Title) {
			results = append(results, SearchResultV2{
				Kind:   "plan",
				ID:     plan.ID,
				PlanID: plan.ID,
				Title:  plan.Title,
			})
		}
	}
	tasks, err := s.ListTasks()
	if err != nil {
		return nil, err
	}
	for _, task := range tasks {
		if len(results) >= searchResultLimit {
			return results, nil
		}
		if has(task.Title) {
			results = append(results, SearchResultV2{
				Kind:   "task",
				ID:     task.ID,
				PlanID: task.PlanID,
				Title:  task.Title,
			})
		}
	}
	notes, err := s.ListNotes()
	if err != nil {
		return nil, err
	}
	for _, note := range notes {
		if len(results) >= searchResultLimit {
			return results, nil
		}
		if has(note.Body) {
			results = append(results, SearchResultV2{
				Kind:    "note",
				ID:      note.ID,
				PlanID:  notePlanID(note),
				Title:   searchNoteTitle(note),
				Snippet: searchSnippet(note.Body, needle),
			})
		}
	}
	return results, nil
}

// notePlanID resolves the plan a note belongs to when the note targets a plan
// directly; task and project notes carry no plan reference.
func notePlanID(note model.Note) uint64 {
	if note.Target == model.TargetPlan {
		return note.TargetID
	}
	return 0
}

func searchNoteTitle(note model.Note) string {
	prefix := ""
	if note.Kind != "" {
		prefix = strings.ToUpper(string(note.Kind[:1])) + string(note.Kind[1:]) + " · "
	}
	switch note.Target {
	case model.TargetPlan:
		return prefix + "Plan note"
	case model.TargetTask:
		return prefix + "Task note"
	default:
		return prefix + "Project note"
	}
}

// searchSnippet returns a short excerpt of body centered on the first needle
// match, ellipsized on both sides when truncated.
func searchSnippet(body, needle string) string {
	index := strings.Index(strings.ToLower(body), needle)
	if index < 0 {
		return ""
	}
	start := index - searchSnippetSpan/2
	if start < 0 {
		start = 0
	}
	end := index + len(needle) + searchSnippetSpan/2
	if end > len(body) {
		end = len(body)
	}
	snippet := body[start:end]
	if start > 0 {
		snippet = "…" + snippet
	}
	if end < len(body) {
		snippet += "…"
	}
	return snippet
}
