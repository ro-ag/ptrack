package gui

import (
	"context"
	"errors"
	"fmt"
	"os"
	"time"

	"github.com/ro-ag/ptrack/internal/gitinfo"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

const (
	snapshotPlanLimit        = 100
	snapshotTaskLimit        = 300
	snapshotBlockerLimit     = 50
	snapshotNoteLimit        = 50
	snapshotCommitLimit      = 50
	snapshotIssueLimit       = 50
	workspaceSnapshotTimeout = 8 * time.Second
	snapshotActivityLimit    = 24
)

type gitSnapshotter interface {
	Capture(context.Context, string) (gitinfo.Snapshot, error)
}

type SnapshotState string

const (
	SnapshotLoading SnapshotState = "loading"
	SnapshotReady   SnapshotState = "ready"
	SnapshotStale   SnapshotState = "stale"
	SnapshotError   SnapshotState = "error"
)

type BoundedSnapshot struct {
	Shown int `json:"shown"`
	Total int `json:"total"`
	More  int `json:"more"`
}

type ProjectStorage struct {
	Status           SnapshotState `json:"status"`
	Exists           bool          `json:"exists"`
	DBPath           string        `json:"dbPath"`
	SizeBytes        int64         `json:"sizeBytes"`
	FormatVersion    uint          `json:"formatVersion"`
	LastWriteVersion string        `json:"lastWriteVersion"`
	Error            string        `json:"error,omitempty"`
}

type ProjectOverview struct {
	Name    string         `json:"name"`
	Root    string         `json:"root"`
	Storage ProjectStorage `json:"storage"`
}

type SnapshotNote struct {
	ID         uint64 `json:"id"`
	Target     string `json:"target"`
	TargetID   uint64 `json:"targetId"`
	Kind       string `json:"kind,omitempty"`
	Body       string `json:"body"`
	OccurredAt string `json:"occurredAt"`
}

type TrackingBounds struct {
	Plans    BoundedSnapshot `json:"plans"`
	Tasks    BoundedSnapshot `json:"tasks"`
	Blockers BoundedSnapshot `json:"blockers"`
	Notes    BoundedSnapshot `json:"notes"`
	Activity BoundedSnapshot `json:"activity"`
	Issues   BoundedSnapshot `json:"issues"`
}

type TrackingSnapshot struct {
	State    SnapshotState  `json:"state"`
	Board    Board          `json:"board"`
	Blockers []Task         `json:"blockers"`
	Notes    []SnapshotNote `json:"notes"`
	Issues   []Issue        `json:"issues"`
	Bounds   TrackingBounds `json:"bounds"`
}

type GitSnapshot struct {
	State    SnapshotState    `json:"state"`
	Error    string           `json:"error,omitempty"`
	Snapshot gitinfo.Snapshot `json:"snapshot"`
}

type TerminalSnapshot struct {
	State    SnapshotState            `json:"state"`
	Error    string                   `json:"error,omitempty"`
	Sessions []TerminalRuntimeSummary `json:"sessions"`
	Bounds   BoundedSnapshot          `json:"bounds"`
}

type AgentRunSnapshot struct {
	State  SnapshotState         `json:"state"`
	Error  string                `json:"error,omitempty"`
	Runs   []AgentRuntimeSummary `json:"runs"`
	Bounds BoundedSnapshot       `json:"bounds"`
}

type WorkspaceSnapshot struct {
	Generation uint64           `json:"generation"`
	CapturedAt string           `json:"capturedAt"`
	Project    ProjectOverview  `json:"project"`
	Tracking   TrackingSnapshot `json:"tracking"`
	Git        GitSnapshot      `json:"git"`
	Terminals  TerminalSnapshot `json:"terminals"`
	AgentRuns  AgentRunSnapshot `json:"agentRuns"`
}

// GetWorkspaceSnapshot returns one generation-scoped, bounded project view.
// Git failures are section-local so tracking data remains usable.
func (a *App) GetWorkspaceSnapshot(
	generation uint64,
	planID uint64,
) (WorkspaceSnapshot, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return WorkspaceSnapshot{}, err
	}
	defer release()
	snapshotCtx, cancelSnapshot := context.WithTimeout(
		workspace.Context(),
		workspaceSnapshotTimeout,
	)
	defer cancelSnapshot()
	defer func() {
		if s != nil {
			_ = s.Close()
		}
	}()

	meta, err := s.GetMeta()
	if err != nil {
		return WorkspaceSnapshot{}, err
	}
	counts, err := s.Counts()
	if err != nil {
		return WorkspaceSnapshot{}, err
	}
	tracking, err := boundedTrackingSnapshot(
		snapshotCtx,
		s,
		workspace,
		meta,
		counts,
		planID,
	)
	if err != nil {
		return WorkspaceSnapshot{}, err
	}
	runtimeProjection, err := workspaceRuntimeProjection(s, workspace)
	if err != nil {
		return WorkspaceSnapshot{}, err
	}
	applyLinkedRuntimeToBoard(&tracking.Board, runtimeProjection)
	snapshot := WorkspaceSnapshot{
		Generation: workspace.Generation(),
		CapturedAt: time.Now().UTC().Format(time.RFC3339),
		Project: ProjectOverview{
			Name:    workspace.name,
			Root:    workspace.root,
			Storage: inspectProjectStorage(workspace.dbPath, meta),
		},
		Tracking: tracking,
		Terminals: TerminalSnapshot{
			State:    SnapshotReady,
			Sessions: runtimeProjection.terminals,
			Bounds:   runtimeProjection.terminalBounds,
		},
		AgentRuns: AgentRunSnapshot{
			State:  SnapshotReady,
			Runs:   runtimeProjection.agents,
			Bounds: runtimeProjection.agentBounds,
		},
	}

	// Do not retain a bbolt read handle while the separately bounded Git
	// commands run.
	if err := s.Close(); err != nil {
		return WorkspaceSnapshot{}, err
	}
	s = nil

	gitService := a.gitSnapshots
	if gitService == nil {
		gitService = gitinfo.Service{}
	}
	gitSnapshot, gitErr := gitService.Capture(snapshotCtx, workspace.root)
	if gitErr != nil {
		snapshot.Git = GitSnapshot{
			State: SnapshotError,
			Error: fmt.Sprintf("Git snapshot unavailable: %v", gitErr),
		}
	} else {
		snapshot.Git = GitSnapshot{
			State:    SnapshotReady,
			Snapshot: gitSnapshot,
		}
	}
	return snapshot, nil
}

func inspectProjectStorage(dbPath string, meta model.Meta) ProjectStorage {
	storage := ProjectStorage{
		Status:           SnapshotReady,
		DBPath:           dbPath,
		FormatVersion:    meta.FormatVersion,
		LastWriteVersion: meta.LastWriteVersion,
	}
	info, err := os.Stat(dbPath)
	switch {
	case err == nil:
		storage.Exists = true
		storage.SizeBytes = info.Size()
	case errors.Is(err, os.ErrNotExist):
		storage.Status = SnapshotError
		storage.Error = "p-track database is missing"
	default:
		storage.Status = SnapshotError
		storage.Error = "p-track database status is unavailable"
	}
	return storage
}

func boundedTrackingSnapshot(
	ctx context.Context,
	s *store.Store,
	workspace *WorkspaceContext,
	meta model.Meta,
	counts model.Counts,
	requestedPlan uint64,
) (TrackingSnapshot, error) {
	plans, err := s.ListPlansBounded(snapshotPlanLimit)
	if err != nil {
		return TrackingSnapshot{}, err
	}
	if err := ctx.Err(); err != nil {
		return TrackingSnapshot{}, err
	}
	planID := requestedPlan
	if planID == 0 {
		planID = workspace.initialPlan
	}
	if planID == 0 {
		planID = meta.ActivePlan
	}

	var selected model.Plan
	tasks := store.Bounded[model.Task]{Items: []model.Task{}}
	progress := store.TaskProgress{}
	if planID != 0 {
		selected, err = s.GetPlan(planID)
		if err != nil {
			if errors.Is(err, store.ErrNotFound) {
				return TrackingSnapshot{}, fmt.Errorf("plan #%d not found", planID)
			}
			return TrackingSnapshot{}, err
		}
		tasks, err = s.ListTasksByPlanBoundedContext(ctx, planID, snapshotTaskLimit)
		if err != nil {
			return TrackingSnapshot{}, err
		}
		progress, err = s.PlanTaskProgressContext(ctx, planID)
		if err != nil {
			return TrackingSnapshot{}, err
		}
	}
	blockers, err := s.ListBlockedTasksBoundedContext(ctx, snapshotBlockerLimit)
	if err != nil {
		return TrackingSnapshot{}, err
	}
	allTasks, err := s.ListTasks()
	if err != nil {
		return TrackingSnapshot{}, err
	}
	planProgress := planProgressByPlan(allTasks)
	notes, err := s.RecentNotesBounded(snapshotNoteLimit)
	if err != nil {
		return TrackingSnapshot{}, err
	}
	commits, err := s.RecentCommitsBounded(snapshotCommitLimit)
	if err != nil {
		return TrackingSnapshot{}, err
	}
	issues, err := s.ListOpenIssuesBoundedContext(ctx, snapshotIssueLimit)
	if err != nil {
		return TrackingSnapshot{}, err
	}

	taskIDs := make(map[uint64]bool, len(tasks.Items))
	for _, task := range tasks.Items {
		taskIDs[task.ID] = true
	}
	associations, err := s.TaskAssociationsContext(ctx, taskIDs)
	if err != nil {
		return TrackingSnapshot{}, err
	}
	board := buildBoundedBoard(
		workspace.name,
		meta,
		counts,
		progress,
		selected,
		plans.Items,
		tasks.Items,
		notes.Items,
		commits.Items,
		issues.Items,
		associations,
		planProgress,
	)
	blockerCards := make([]Task, 0, len(blockers.Items))
	for _, blocker := range blockers.Items {
		blockerCards = append(blockerCards, snapshotTaskCard(blocker, nil, nil, nil, nil))
	}
	snapshotNotes := make([]SnapshotNote, 0, len(notes.Items))
	for _, note := range notes.Items {
		snapshotNotes = append(snapshotNotes, SnapshotNote{
			ID:         note.ID,
			Target:     string(note.Target),
			TargetID:   note.TargetID,
			Kind:       string(note.Kind),
			Body:       note.Body,
			OccurredAt: note.CreatedAt.UTC().Format(time.RFC3339),
		})
	}
	snapshotIssues := make([]Issue, 0, len(issues.Items))
	for _, issue := range issues.Items {
		snapshotIssues = append(snapshotIssues, Issue{
			ID:       issue.ID,
			Title:    issue.Title,
			Severity: string(issue.Severity),
			TaskID:   issue.TaskID,
		})
	}
	activityTotal := notes.Total + commits.Total
	return TrackingSnapshot{
		State:    SnapshotReady,
		Board:    board,
		Blockers: blockerCards,
		Notes:    snapshotNotes,
		Issues:   snapshotIssues,
		Bounds: TrackingBounds{
			Plans:    snapshotBound(len(plans.Items), plans.Total),
			Tasks:    snapshotBound(len(tasks.Items), tasks.Total),
			Blockers: snapshotBound(len(blockers.Items), blockers.Total),
			Notes:    snapshotBound(len(notes.Items), notes.Total),
			Activity: snapshotBound(len(board.Activity), activityTotal),
			Issues:   snapshotBound(len(issues.Items), issues.Total),
		},
	}, nil
}

func buildBoundedBoard(
	projectName string,
	meta model.Meta,
	counts model.Counts,
	progress store.TaskProgress,
	selected model.Plan,
	plans []model.Plan,
	tasks []model.Task,
	notes []model.Note,
	commits []model.Commit,
	issues []model.Issue,
	associations store.TaskAssociations,
	planProgress map[uint64]store.TaskProgress,
) Board {
	board := Board{
		ProjectName: projectName,
		Goal:        meta.Goal,
		Summary:     meta.Summary,
		PlanID:      selected.ID,
		PlanTitle:   selected.Title,
		Plans:       make([]PlanSummary, 0, len(plans)),
		Columns:     make([]Column, len(statuses)),
		Stats: ProjectStats{
			PlanTasks:     progress.Total,
			PlanTasksDone: progress.Done,
			TasksOpen:     counts.TasksOpen,
			TasksBlocked:  counts.TasksBlocked,
			Notes:         counts.Notes,
			Commits:       counts.Commits,
			OpenIssues:    counts.IssuesOpen,
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
	for index, status := range statuses {
		columnByStatus[status] = index
		board.Columns[index] = Column{
			Status: string(status),
			Title:  titles[status],
			Tasks:  []Task{},
		}
	}

	taskIDs := make(map[uint64]bool, len(tasks))
	for _, task := range tasks {
		taskIDs[task.ID] = true
	}
	for _, issue := range issues {
		board.OpenIssues = append(board.OpenIssues, Issue{
			ID:       issue.ID,
			Title:    issue.Title,
			Severity: string(issue.Severity),
			TaskID:   issue.TaskID,
		})
	}
	for _, task := range tasks {
		index, exists := columnByStatus[task.Status]
		if !exists {
			continue
		}
		board.Columns[index].Tasks = append(board.Columns[index].Tasks, snapshotTaskCard(
			task,
			associations.NoteCounts,
			associations.CommitCounts,
			associations.IssueCounts,
			associations.LatestNotes,
		))
	}
	board.Activity = recentActivity(selected.ID, taskIDs, notes, commits)
	if len(board.Activity) > snapshotActivityLimit {
		board.Activity = board.Activity[:snapshotActivityLimit]
	}
	return board
}

func snapshotTaskCard(
	task model.Task,
	noteCount map[uint64]int,
	commitCount map[uint64]int,
	issueCount map[uint64]int,
	latestNote map[uint64]string,
) Task {
	return Task{
		ID:          task.ID,
		Title:       task.Title,
		Status:      string(task.Status),
		UpdatedAt:   task.UpdatedAt.UTC().Format(time.RFC3339),
		NoteCount:   noteCount[task.ID],
		CommitCount: commitCount[task.ID],
		IssueCount:  issueCount[task.ID],
		LatestNote:  latestNote[task.ID],
	}
}

func snapshotBound(shown, total int) BoundedSnapshot {
	return BoundedSnapshot{
		Shown: shown,
		Total: total,
		More:  max(0, total-shown),
	}
}

// planProgressByPlan groups per-plan task totals in one pass over all tasks.
// Done means status done, matching store.PlanTaskProgress.
func planProgressByPlan(tasks []model.Task) map[uint64]store.TaskProgress {
	progress := make(map[uint64]store.TaskProgress)
	for _, task := range tasks {
		entry := progress[task.PlanID]
		entry.Total++
		if task.Status == model.TaskDone {
			entry.Done++
		}
		progress[task.PlanID] = entry
	}
	return progress
}

func planSummary(
	plan model.Plan,
	activePlan uint64,
	progress map[uint64]store.TaskProgress,
) PlanSummary {
	entry := progress[plan.ID]
	return PlanSummary{
		ID:         plan.ID,
		Title:      plan.Title,
		IsActive:   plan.ID == activePlan,
		TasksTotal: entry.Total,
		TasksDone:  entry.Done,
	}
}
