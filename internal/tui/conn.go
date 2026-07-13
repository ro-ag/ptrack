package tui

import (
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

// conn is a transient accessor to a project's bbolt database. Every method opens
// the database, performs one operation, and closes it, so the TUI never holds a
// file lock while idle — an AI agent (or the CLI) can read and write the same
// project concurrently without blocking on the viewer.
type conn struct{ dbPath string }

func (c conn) do(fn func(*store.Store) error) error {
	s, err := store.Open(c.dbPath)
	if err != nil {
		return err
	}
	defer s.Close()
	return fn(s)
}

func (c conn) GetMeta() (model.Meta, error) {
	var r model.Meta
	return r, c.do(func(s *store.Store) (e error) { r, e = s.GetMeta(); return })
}
func (c conn) SetGoal(v string) error {
	return c.do(func(s *store.Store) error { return s.SetGoal(v) })
}
func (c conn) SetSummary(v string) error {
	return c.do(func(s *store.Store) error { return s.SetSummary(v) })
}
func (c conn) SetActivePlan(id uint64) error {
	return c.do(func(s *store.Store) error { return s.SetActivePlan(id) })
}

func (c conn) AddPlan(title string) (model.Plan, error) {
	var r model.Plan
	return r, c.do(func(s *store.Store) (e error) { r, e = s.AddPlan(title); return })
}
func (c conn) ListPlans() ([]model.Plan, error) {
	var r []model.Plan
	return r, c.do(func(s *store.Store) (e error) { r, e = s.ListPlans(); return })
}
func (c conn) GetPlan(id uint64) (model.Plan, error) {
	var r model.Plan
	return r, c.do(func(s *store.Store) (e error) { r, e = s.GetPlan(id); return })
}
func (c conn) SetPlanStatus(id uint64, st model.PlanStatus) error {
	return c.do(func(s *store.Store) error { return s.SetPlanStatus(id, st) })
}
func (c conn) SetPlanTitle(id uint64, t string) error {
	return c.do(func(s *store.Store) error { return s.SetPlanTitle(id, t) })
}

func (c conn) AddTask(planID uint64, title string) (model.Task, error) {
	var r model.Task
	return r, c.do(func(s *store.Store) (e error) { r, e = s.AddTask(planID, title); return })
}
func (c conn) ListTasksByPlan(planID uint64) ([]model.Task, error) {
	var r []model.Task
	return r, c.do(func(s *store.Store) (e error) { r, e = s.ListTasksByPlan(planID); return })
}
func (c conn) GetTask(id uint64) (model.Task, error) {
	var r model.Task
	return r, c.do(func(s *store.Store) (e error) { r, e = s.GetTask(id); return })
}
func (c conn) SetTaskStatus(id uint64, st model.TaskStatus) error {
	return c.do(func(s *store.Store) error { return s.SetTaskStatus(id, st) })
}
func (c conn) SetTaskTitle(id uint64, t string) error {
	return c.do(func(s *store.Store) error { return s.SetTaskTitle(id, t) })
}

func (c conn) AddNote(target model.NoteTarget, id uint64, body string) (model.Note, error) {
	var r model.Note
	return r, c.do(func(s *store.Store) (e error) { r, e = s.AddNote(target, id, body); return })
}
func (c conn) NotesByTask(id uint64) ([]model.Note, error) {
	var r []model.Note
	return r, c.do(func(s *store.Store) (e error) { r, e = s.NotesByTask(id); return })
}
func (c conn) NotesByPlan(id uint64) ([]model.Note, error) {
	var r []model.Note
	return r, c.do(func(s *store.Store) (e error) { r, e = s.NotesByPlan(id); return })
}

func (c conn) Counts() (model.Counts, error) {
	var r model.Counts
	return r, c.do(func(s *store.Store) (e error) { r, e = s.Counts(); return })
}

func (c conn) AddMilestone(title string) (model.Milestone, error) {
	var r model.Milestone
	return r, c.do(func(s *store.Store) (e error) { r, e = s.AddMilestone(title); return })
}
func (c conn) ListMilestones() ([]model.Milestone, error) {
	var r []model.Milestone
	return r, c.do(func(s *store.Store) (e error) { r, e = s.ListMilestones(); return })
}
func (c conn) GetMilestone(id uint64) (model.Milestone, error) {
	var r model.Milestone
	return r, c.do(func(s *store.Store) (e error) { r, e = s.GetMilestone(id); return })
}
func (c conn) SetMilestoneStatus(id uint64, st model.MilestoneStatus) error {
	return c.do(func(s *store.Store) error { return s.SetMilestoneStatus(id, st) })
}
func (c conn) SetMilestoneTitle(id uint64, t string) error {
	return c.do(func(s *store.Store) error { return s.SetMilestoneTitle(id, t) })
}

func (c conn) AddIssue(title, body string, sev model.Severity, taskID uint64) (model.Issue, error) {
	var r model.Issue
	return r, c.do(func(s *store.Store) (e error) { r, e = s.AddIssue(title, body, sev, taskID); return })
}
func (c conn) ListIssues() ([]model.Issue, error) {
	var r []model.Issue
	return r, c.do(func(s *store.Store) (e error) { r, e = s.ListIssues(); return })
}
func (c conn) GetIssue(id uint64) (model.Issue, error) {
	var r model.Issue
	return r, c.do(func(s *store.Store) (e error) { r, e = s.GetIssue(id); return })
}
func (c conn) SetIssueStatus(id uint64, st model.IssueStatus) error {
	return c.do(func(s *store.Store) error { return s.SetIssueStatus(id, st) })
}
func (c conn) SetIssueTitle(id uint64, t string) error {
	return c.do(func(s *store.Store) error { return s.SetIssueTitle(id, t) })
}

func (c conn) CommitsByTask(id uint64) ([]model.Commit, error) {
	var r []model.Commit
	return r, c.do(func(s *store.Store) (e error) { r, e = s.CommitsByTask(id); return })
}
func (c conn) CommitsByPlan(id uint64) ([]model.Commit, error) {
	var r []model.Commit
	return r, c.do(func(s *store.Store) (e error) { r, e = s.CommitsByPlan(id); return })
}
