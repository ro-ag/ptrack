// Package gui provides the Wails desktop kanban board.
package gui

import (
	"context"
	"errors"
	"fmt"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

var statuses = []model.TaskStatus{
	model.TaskTodo,
	model.TaskDoing,
	model.TaskBlocked,
	model.TaskDone,
}

// App is the durable Go backend exposed to the Wails frontend. Published
// project resources live in generation-scoped WorkspaceContexts.
type App struct {
	dbPath       string
	initialPlan  uint64
	projectName  string
	projectRoot  string
	terminals    terminalManager
	emitTerminal terminalEventEmitter

	lifecycleMu         sync.Mutex
	wailsContext        context.Context
	monitorCtx          context.Context
	monitorCancel       context.CancelFunc
	monitorWG           lifecycleGroup
	terminalOps         lifecycleGroup
	watcherCancel       context.CancelFunc
	watcherGeneration   uint64
	startupReady        chan struct{}
	startupOnce         sync.Once
	shuttingDown        bool
	shutdownStarted     chan struct{}
	shutdownSignalOnce  sync.Once
	shutdownOnce        sync.Once
	shutdownWaitTimeout time.Duration

	workspaceMu       sync.RWMutex
	workspace         *WorkspaceContext
	workspaceStatus   WorkspaceStatus
	workspaceError    string
	lastGeneration    uint64
	transitionMu      sync.Mutex
	buildWorkspace    workspaceBuilder
	confirmation      *workspaceConfirmation
	confirmationTTL   time.Duration
	confirmationTimer *time.Timer
	gitSnapshots      gitSnapshotter
}

// Board is the complete snapshot rendered by the frontend.
type Board struct {
	ProjectName string        `json:"projectName"`
	Goal        string        `json:"goal"`
	Summary     string        `json:"summary"`
	Plans       []PlanSummary `json:"plans"`
	PlanID      uint64        `json:"planId"`
	PlanTitle   string        `json:"planTitle"`
	Columns     []Column      `json:"columns"`
	Stats       ProjectStats  `json:"stats"`
	Activity    []Activity    `json:"activity"`
	OpenIssues  []Issue       `json:"openIssues"`
}

type BoardV2 struct {
	Generation uint64 `json:"generation"`
	Board      Board  `json:"board"`
}

type TaskV2 struct {
	Generation uint64 `json:"generation"`
	Task       Task   `json:"task"`
}

type WorkspaceMutationResult struct {
	Generation uint64 `json:"generation"`
}

// PlanSummary is a selectable plan in the board header.
type PlanSummary struct {
	ID         uint64 `json:"id"`
	Title      string `json:"title"`
	IsActive   bool   `json:"isActive"`
	TasksTotal int    `json:"tasksTotal"`
	TasksDone  int    `json:"tasksDone"`
}

// Column is one task status lane.
type Column struct {
	Status string `json:"status"`
	Title  string `json:"title"`
	Tasks  []Task `json:"tasks"`
}

// Task is the frontend representation of a kanban card.
type Task struct {
	ID          uint64 `json:"id"`
	Title       string `json:"title"`
	Status      string `json:"status"`
	UpdatedAt   string `json:"updatedAt"`
	NoteCount   int    `json:"noteCount"`
	CommitCount int    `json:"commitCount"`
	IssueCount  int    `json:"issueCount"`
	LatestNote  string `json:"latestNote"`
}

// ProjectStats is the compact project status shown in the memory rail.
type ProjectStats struct {
	PlanTasks     int `json:"planTasks"`
	PlanTasksDone int `json:"planTasksDone"`
	TasksOpen     int `json:"tasksOpen"`
	TasksBlocked  int `json:"tasksBlocked"`
	Notes         int `json:"notes"`
	Commits       int `json:"commits"`
	OpenIssues    int `json:"openIssues"`
}

// Activity is a recent note or commit relevant to the selected plan.
type Activity struct {
	Kind       string `json:"kind"`
	Title      string `json:"title"`
	Detail     string `json:"detail"`
	Target     string `json:"target"`
	OccurredAt string `json:"occurredAt"`
}

// Issue is a compact open issue reference for the memory rail.
type Issue struct {
	ID       uint64 `json:"id"`
	Title    string `json:"title"`
	Severity string `json:"severity"`
	TaskID   uint64 `json:"taskId"`
}

func newApp(dbPath string, initialPlan uint64) *App {
	app, _ := newAppWithTerminal(dbPath, initialPlan, nil, nil)
	return app
}

func (a *App) openWorkspace(
	expectedGeneration uint64,
) (*store.Store, *WorkspaceContext, func(), error) {
	workspace, err := a.currentWorkspace(expectedGeneration)
	if err != nil {
		return nil, nil, nil, err
	}
	release, err := workspace.beginOperation(expectedGeneration, false)
	if err != nil {
		return nil, nil, nil, err
	}
	s, err := store.Open(workspace.dbPath)
	if err != nil {
		release()
		return nil, nil, nil, err
	}
	return s, workspace, release, nil
}

// GetBoard returns a fresh board snapshot. A zero plan ID selects the command's
// --plan value, then falls back to the project's active plan.
func (a *App) GetBoard(planID uint64) (Board, error) {
	return a.getBoard(0, planID)
}

func (a *App) GetBoardV2(generation, planID uint64) (BoardV2, error) {
	workspace, err := a.currentWorkspace(generation)
	if err != nil {
		return BoardV2{}, err
	}
	actualGeneration := workspace.Generation()
	board, err := a.getBoard(actualGeneration, planID)
	if err != nil {
		return BoardV2{}, err
	}
	return BoardV2{Generation: actualGeneration, Board: board}, nil
}

func (a *App) getBoard(expectedGeneration, planID uint64) (Board, error) {
	s, workspace, release, err := a.openWorkspace(expectedGeneration)
	if err != nil {
		return Board{}, err
	}
	defer release()
	defer s.Close()

	meta, err := s.GetMeta()
	if err != nil {
		return Board{}, err
	}
	if planID == 0 {
		planID = a.initialPlan
	}
	if planID == 0 {
		planID = meta.ActivePlan
	}
	if planID == 0 {
		return Board{}, errors.New("no active plan; set one with 'ptrack plan use <id>' or pass --plan")
	}

	selected, err := s.GetPlan(planID)
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return Board{}, fmt.Errorf("plan #%d not found", planID)
		}
		return Board{}, err
	}
	plans, err := s.ListPlans()
	if err != nil {
		return Board{}, err
	}
	allTasks, err := s.ListTasks()
	if err != nil {
		return Board{}, err
	}
	tasks := make([]model.Task, 0, len(allTasks))
	for _, task := range allTasks {
		if task.PlanID == planID {
			tasks = append(tasks, task)
		}
	}
	planProgress := planProgressByPlan(allTasks)
	notes, err := s.ListNotes()
	if err != nil {
		return Board{}, err
	}
	commits, err := s.ListCommits()
	if err != nil {
		return Board{}, err
	}
	issues, err := s.ListIssues()
	if err != nil {
		return Board{}, err
	}
	counts, err := s.Counts()
	if err != nil {
		return Board{}, err
	}

	board := Board{
		ProjectName: workspace.name,
		Goal:        meta.Goal,
		Summary:     meta.Summary,
		PlanID:      selected.ID,
		PlanTitle:   selected.Title,
		Plans:       make([]PlanSummary, 0, len(plans)),
		Columns:     make([]Column, len(statuses)),
		Stats: ProjectStats{
			PlanTasks:    len(tasks),
			TasksOpen:    counts.TasksOpen,
			TasksBlocked: counts.TasksBlocked,
			Notes:        counts.Notes,
			Commits:      counts.Commits,
			OpenIssues:   counts.IssuesOpen,
		},
		Activity:   []Activity{},
		OpenIssues: []Issue{},
	}
	for _, plan := range plans {
		board.Plans = append(board.Plans, planSummary(plan, meta.ActivePlan, planProgress))
	}
	titles := map[model.TaskStatus]string{
		model.TaskTodo:    "Todo",
		model.TaskDoing:   "Doing",
		model.TaskBlocked: "Blocked",
		model.TaskDone:    "Done",
	}
	columnByStatus := make(map[model.TaskStatus]int, len(statuses))
	for i, status := range statuses {
		columnByStatus[status] = i
		board.Columns[i] = Column{
			Status: string(status),
			Title:  titles[status],
			Tasks:  []Task{},
		}
	}

	taskIDs := make(map[uint64]bool, len(tasks))
	noteCount := make(map[uint64]int, len(tasks))
	commitCount := make(map[uint64]int, len(tasks))
	issueCount := make(map[uint64]int, len(tasks))
	latestNote := make(map[uint64]string, len(tasks))
	for _, task := range tasks {
		taskIDs[task.ID] = true
		if task.Status == model.TaskDone {
			board.Stats.PlanTasksDone++
		}
	}
	for _, note := range notes {
		if note.Target == model.TargetTask && taskIDs[note.TargetID] {
			noteCount[note.TargetID]++
			latestNote[note.TargetID] = note.Body
		}
	}
	for _, commit := range commits {
		if taskIDs[commit.TaskID] {
			commitCount[commit.TaskID]++
		}
	}
	for _, issue := range issues {
		if issue.Status == model.IssueOpen {
			if taskIDs[issue.TaskID] {
				issueCount[issue.TaskID]++
			}
			board.OpenIssues = append(board.OpenIssues, Issue{
				ID:       issue.ID,
				Title:    issue.Title,
				Severity: string(issue.Severity),
				TaskID:   issue.TaskID,
			})
		}
	}
	for _, task := range tasks {
		i, ok := columnByStatus[task.Status]
		if !ok {
			continue
		}
		board.Columns[i].Tasks = append(board.Columns[i].Tasks, Task{
			ID:          task.ID,
			Title:       task.Title,
			Status:      string(task.Status),
			UpdatedAt:   task.UpdatedAt.Format(time.RFC3339),
			NoteCount:   noteCount[task.ID],
			CommitCount: commitCount[task.ID],
			IssueCount:  issueCount[task.ID],
			LatestNote:  latestNote[task.ID],
		})
	}
	board.Activity = recentActivity(planID, taskIDs, notes, commits)
	if len(board.OpenIssues) > 5 {
		board.OpenIssues = board.OpenIssues[:5]
	}
	return board, nil
}

type activityEvent struct {
	at       time.Time
	activity Activity
}

func recentActivity(planID uint64, taskIDs map[uint64]bool, notes []model.Note, commits []model.Commit) []Activity {
	events := make([]activityEvent, 0, len(notes)+len(commits))
	for _, note := range notes {
		relevant := note.Target == model.TargetProject ||
			(note.Target == model.TargetPlan && note.TargetID == planID) ||
			(note.Target == model.TargetTask && taskIDs[note.TargetID])
		if !relevant {
			continue
		}
		target := "Project"
		if note.Target == model.TargetPlan {
			target = fmt.Sprintf("Plan #%d", note.TargetID)
		}
		if note.Target == model.TargetTask {
			target = fmt.Sprintf("Task #%d", note.TargetID)
		}
		events = append(events, activityEvent{
			at: note.CreatedAt,
			activity: Activity{
				Kind:       "note",
				Title:      "Decision recorded",
				Detail:     note.Body,
				Target:     target,
				OccurredAt: note.CreatedAt.Format(time.RFC3339),
			},
		})
	}
	for _, commit := range commits {
		if commit.PlanID != planID && !taskIDs[commit.TaskID] {
			continue
		}
		target := fmt.Sprintf("Plan #%d", planID)
		if commit.TaskID != 0 {
			target = fmt.Sprintf("Task #%d", commit.TaskID)
		}
		sha := commit.SHA
		if len(sha) > 8 {
			sha = sha[:8]
		}
		events = append(events, activityEvent{
			at: commit.CreatedAt,
			activity: Activity{
				Kind:       "commit",
				Title:      commit.Subject,
				Detail:     sha,
				Target:     target,
				OccurredAt: commit.CreatedAt.Format(time.RFC3339),
			},
		})
	}
	sort.SliceStable(events, func(i, j int) bool { return events[i].at.After(events[j].at) })
	limit := min(24, len(events))
	activity := make([]Activity, 0, limit)
	for _, event := range events[:limit] {
		activity = append(activity, event.activity)
	}
	return activity
}

// AddTask creates a todo card in planID.
func (a *App) AddTask(planID uint64, title string) (Task, error) {
	result, err := a.addTask(0, planID, title)
	return result.Task, err
}

func (a *App) AddTaskV2(generation, planID uint64, title string) (TaskV2, error) {
	return a.addTask(generation, planID, title)
}

func (a *App) addTask(expectedGeneration, planID uint64, title string) (TaskV2, error) {
	title = strings.TrimSpace(title)
	if title == "" {
		return TaskV2{}, errors.New("task title cannot be empty")
	}
	s, workspace, release, err := a.openWorkspace(expectedGeneration)
	if err != nil {
		return TaskV2{}, err
	}
	defer release()
	defer s.Close()
	task, err := s.AddTask(planID, title)
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return TaskV2{}, fmt.Errorf("plan #%d not found", planID)
		}
		return TaskV2{}, err
	}
	return TaskV2{
		Generation: workspace.Generation(),
		Task: Task{
			ID:        task.ID,
			Title:     task.Title,
			Status:    string(task.Status),
			UpdatedAt: task.UpdatedAt.Format(time.RFC3339),
		},
	}, nil
}

// RenameTask updates a card title.
func (a *App) RenameTask(taskID uint64, title string) error {
	_, err := a.renameTask(0, taskID, title)
	return err
}

func (a *App) RenameTaskV2(
	generation, taskID uint64,
	title string,
) (WorkspaceMutationResult, error) {
	return a.renameTask(generation, taskID, title)
}

func (a *App) renameTask(
	expectedGeneration, taskID uint64,
	title string,
) (WorkspaceMutationResult, error) {
	title = strings.TrimSpace(title)
	if title == "" {
		return WorkspaceMutationResult{}, errors.New("task title cannot be empty")
	}
	s, workspace, release, err := a.openWorkspace(expectedGeneration)
	if err != nil {
		return WorkspaceMutationResult{}, err
	}
	defer release()
	defer s.Close()
	if err := s.SetTaskTitle(taskID, title); err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return WorkspaceMutationResult{}, fmt.Errorf("task #%d not found", taskID)
		}
		return WorkspaceMutationResult{}, err
	}
	return WorkspaceMutationResult{Generation: workspace.Generation()}, nil
}

// MoveTask changes a card's status after validating the status value supplied
// by JavaScript.
func (a *App) MoveTask(taskID uint64, status string) error {
	_, err := a.moveTask(0, taskID, status)
	return err
}

func (a *App) MoveTaskV2(
	generation, taskID uint64,
	status string,
) (WorkspaceMutationResult, error) {
	return a.moveTask(generation, taskID, status)
}

func (a *App) moveTask(
	expectedGeneration, taskID uint64,
	status string,
) (WorkspaceMutationResult, error) {
	wanted := model.TaskStatus(status)
	valid := false
	for _, candidate := range statuses {
		if wanted == candidate {
			valid = true
			break
		}
	}
	if !valid {
		return WorkspaceMutationResult{}, fmt.Errorf("invalid task status %q", status)
	}
	s, workspace, release, err := a.openWorkspace(expectedGeneration)
	if err != nil {
		return WorkspaceMutationResult{}, err
	}
	defer release()
	defer s.Close()
	if err := s.SetTaskStatus(taskID, wanted); err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return WorkspaceMutationResult{}, fmt.Errorf("task #%d not found", taskID)
		}
		return WorkspaceMutationResult{}, err
	}
	return WorkspaceMutationResult{Generation: workspace.Generation()}, nil
}

// AddTaskNote records a decision or observation on a card.
func (a *App) AddTaskNote(taskID uint64, body string) error {
	_, err := a.addTaskNote(0, taskID, body)
	return err
}

func (a *App) AddTaskNoteV2(
	generation, taskID uint64,
	body string,
) (WorkspaceMutationResult, error) {
	return a.addTaskNote(generation, taskID, body)
}

func (a *App) addTaskNote(
	expectedGeneration, taskID uint64,
	body string,
) (WorkspaceMutationResult, error) {
	body = strings.TrimSpace(body)
	if body == "" {
		return WorkspaceMutationResult{}, errors.New("memory note cannot be empty")
	}
	s, workspace, release, err := a.openWorkspace(expectedGeneration)
	if err != nil {
		return WorkspaceMutationResult{}, err
	}
	defer release()
	defer s.Close()
	if _, err := s.GetTask(taskID); err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return WorkspaceMutationResult{}, fmt.Errorf("task #%d not found", taskID)
		}
		return WorkspaceMutationResult{}, err
	}
	_, err = s.AddNote(model.TargetTask, taskID, body)
	if err != nil {
		return WorkspaceMutationResult{}, err
	}
	return WorkspaceMutationResult{Generation: workspace.Generation()}, nil
}
