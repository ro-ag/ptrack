package gui

import (
	"errors"
	"fmt"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/store"
)

// TaskDetailNote is one recorded memory on a task card.
type TaskDetailNote struct {
	ID         uint64 `json:"id"`
	Kind       string `json:"kind,omitempty"`
	Body       string `json:"body"`
	OccurredAt string `json:"occurredAt"`
}

// TaskDetailCommit is one commit linked to a task card.
type TaskDetailCommit struct {
	ID         uint64 `json:"id"`
	SHA        string `json:"sha"`
	Subject    string `json:"subject"`
	OccurredAt string `json:"occurredAt"`
}

// TaskDetail is the full context shown in the task detail drawer.
type TaskDetail struct {
	Generation        uint64                  `json:"generation"`
	Task              Task                    `json:"task"`
	LinkedRuntime     TaskLinkedRuntimeDetail `json:"linkedRuntime"`
	AgentIntelligence []AgentIntelligenceV2   `json:"agentIntelligence"`
	Notes             []TaskDetailNote        `json:"notes"`
	Commits           []TaskDetailCommit      `json:"commits"`
	Issues            []Issue                 `json:"issues"`
}

// GetTaskDetailV2 returns the full context for one board card: the task
// itself plus its notes (newest first), linked commits (newest first), and
// linked issues of any status.
func (a *App) GetTaskDetailV2(generation, taskID uint64) (TaskDetail, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return TaskDetail{}, err
	}
	defer release()
	defer s.Close()

	task, err := s.GetTask(taskID)
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return TaskDetail{}, fmt.Errorf("task #%d not found", taskID)
		}
		return TaskDetail{}, err
	}
	notes, err := s.NotesByTask(taskID)
	if err != nil {
		return TaskDetail{}, err
	}
	commits, err := s.CommitsByTask(taskID)
	if err != nil {
		return TaskDetail{}, err
	}
	issues, err := s.ListIssues()
	if err != nil {
		return TaskDetail{}, err
	}

	detail := TaskDetail{
		Generation: workspace.Generation(),
		Task: Task{
			ID:          task.ID,
			Title:       task.Title,
			Status:      string(task.Status),
			UpdatedAt:   task.UpdatedAt.UTC().Format(time.RFC3339),
			NoteCount:   len(notes),
			CommitCount: len(commits),
		},
		Notes:             make([]TaskDetailNote, 0, len(notes)),
		Commits:           make([]TaskDetailCommit, 0, len(commits)),
		Issues:            make([]Issue, 0),
		AgentIntelligence: []AgentIntelligenceV2{},
	}
	// NotesByTask is insertion ordered; the drawer shows newest first.
	for i := len(notes) - 1; i >= 0; i-- {
		note := notes[i]
		detail.Notes = append(detail.Notes, TaskDetailNote{
			ID:         note.ID,
			Kind:       string(note.Kind),
			Body:       note.Body,
			OccurredAt: note.CreatedAt.UTC().Format(time.RFC3339),
		})
	}
	for _, commit := range commits {
		detail.Commits = append(detail.Commits, TaskDetailCommit{
			ID:         commit.ID,
			SHA:        commit.SHA,
			Subject:    commit.Subject,
			OccurredAt: commit.CreatedAt.UTC().Format(time.RFC3339),
		})
	}
	for _, issue := range issues {
		if issue.TaskID != taskID {
			continue
		}
		detail.Task.IssueCount++
		detail.Issues = append(detail.Issues, Issue{
			ID:       issue.ID,
			Title:    issue.Title,
			Severity: string(issue.Severity),
			TaskID:   issue.TaskID,
		})
	}
	if len(notes) > 0 {
		detail.Task.LatestNote = notes[len(notes)-1].Body
	}
	projection, err := workspaceRuntimeProjection(s, workspace)
	if err != nil {
		return TaskDetail{}, err
	}
	detail.LinkedRuntime = taskLinkedRuntime(projection, taskID)
	if registry, ok := workspace.agents.(agentIntelligenceRegistry); ok {
		for _, run := range detail.LinkedRuntime.Agents {
			intelligence, intelligenceErr := buildAgentIntelligenceV2(
				s,
				workspace,
				registry,
				run.RunID,
			)
			if intelligenceErr != nil {
				if errors.Is(intelligenceErr, agentrun.ErrRunNotFound) {
					continue
				}
				return TaskDetail{}, intelligenceErr
			}
			if !agentIntelligenceMatchesTaskSnapshot(run, intelligence, taskID) {
				continue
			}
			detail.AgentIntelligence = append(detail.AgentIntelligence, intelligence)
		}
	}
	if detail.LinkedRuntime.Summary.Terminals != 0 ||
		detail.LinkedRuntime.Summary.Agents != 0 ||
		detail.LinkedRuntime.Summary.Truncated {
		summary := detail.LinkedRuntime.Summary
		detail.Task.LinkedRuntime = &summary
	}
	return detail, nil
}

func agentIntelligenceMatchesTaskSnapshot(
	run AgentRuntimeSummary,
	intelligence AgentIntelligenceV2,
	taskID uint64,
) bool {
	return run.Association != nil && intelligence.Association != nil &&
		run.Association.PlanID == intelligence.Association.PlanID &&
		run.Association.TaskID == taskID &&
		intelligence.Association.TaskID == taskID &&
		run.Association.Revision == intelligence.Association.Revision
}
