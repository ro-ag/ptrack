package report

import (
	"fmt"
	"strings"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

// PlanRef is a compact plan reference.
type PlanRef struct {
	ID     uint64 `json:"id"`
	Title  string `json:"title"`
	Status string `json:"status"`
}

func planRef(p model.Plan) PlanRef {
	return PlanRef{ID: p.ID, Title: p.Title, Status: string(p.Status)}
}

// --- next ---

// NextView names the single most-actionable task, or explains its absence.
type NextView struct {
	Task      *TaskLine `json:"task"`
	PlanTitle string    `json:"plan_title,omitempty"`
	Message   string    `json:"message,omitempty"`
}

// Next returns the first doing task in the active plan, else the first todo.
func Next(s *store.Store) (NextView, error) {
	m, err := s.GetMeta()
	if err != nil {
		return NextView{}, err
	}
	if m.ActivePlan == 0 {
		return NextView{Message: "no active plan (set one with 'ptrack plan use <id>')"}, nil
	}
	p, err := s.GetPlan(m.ActivePlan)
	if err != nil {
		return NextView{}, err
	}
	tasks, err := s.ListTasksByPlan(p.ID)
	if err != nil {
		return NextView{}, err
	}
	pick := firstWithStatus(tasks, model.TaskDoing)
	if pick == nil {
		pick = firstWithStatus(tasks, model.TaskTodo)
	}
	if pick == nil {
		return NextView{PlanTitle: p.Title, Message: "no actionable task in the active plan"}, nil
	}
	tl := taskLine(*pick)
	return NextView{Task: &tl, PlanTitle: p.Title}, nil
}

// Markdown renders the next view.
func (n NextView) Markdown() string {
	if n.Task == nil {
		return n.Message + "\n"
	}
	return fmt.Sprintf("next: [%s] #%d %s (plan: %s)\n", n.Task.Status, n.Task.ID, n.Task.Title, n.PlanTitle)
}

func firstWithStatus(tasks []model.Task, st model.TaskStatus) *model.Task {
	for i := range tasks {
		if tasks[i].Status == st {
			return &tasks[i]
		}
	}
	return nil
}

// --- plan show ---

// PlanShow is a single plan with its tasks and notes.
type PlanShow struct {
	Plan  PlanRef    `json:"plan"`
	Tasks []TaskLine `json:"tasks"`
	Notes []NoteLine `json:"notes"`
}

// ShowPlan assembles a full view of one plan.
func ShowPlan(s *store.Store, id uint64) (PlanShow, error) {
	p, err := s.GetPlan(id)
	if err != nil {
		return PlanShow{}, err
	}
	tasks, err := s.ListTasksByPlan(id)
	if err != nil {
		return PlanShow{}, err
	}
	notes, err := s.NotesByPlan(id)
	if err != nil {
		return PlanShow{}, err
	}
	v := PlanShow{Plan: planRef(p)}
	for _, t := range tasks {
		v.Tasks = append(v.Tasks, taskLine(t))
	}
	for _, n := range notes {
		v.Notes = append(v.Notes, noteLine(n))
	}
	return v, nil
}

// Markdown renders a plan view.
func (v PlanShow) Markdown() string {
	var b strings.Builder
	fmt.Fprintf(&b, "# Plan #%d %s [%s]\n\n", v.Plan.ID, v.Plan.Title, v.Plan.Status)
	b.WriteString("## Tasks\n")
	if len(v.Tasks) == 0 {
		b.WriteString("_none_\n")
	} else {
		for _, t := range v.Tasks {
			fmt.Fprintf(&b, "- [%s] #%d %s\n", t.Status, t.ID, t.Title)
		}
	}
	b.WriteString("\n## Notes\n")
	b.WriteString(notesMarkdown(v.Notes))
	return b.String()
}

// --- task show ---

// TaskShow is a single task with its parent plan and notes.
type TaskShow struct {
	Task  TaskLine   `json:"task"`
	Plan  *PlanRef   `json:"plan"`
	Notes []NoteLine `json:"notes"`
}

// ShowTask assembles a full view of one task.
func ShowTask(s *store.Store, id uint64) (TaskShow, error) {
	t, err := s.GetTask(id)
	if err != nil {
		return TaskShow{}, err
	}
	v := TaskShow{Task: taskLine(t)}
	if p, err := s.GetPlan(t.PlanID); err == nil {
		pr := planRef(p)
		v.Plan = &pr
	}
	notes, err := s.NotesByTask(id)
	if err != nil {
		return TaskShow{}, err
	}
	for _, n := range notes {
		v.Notes = append(v.Notes, noteLine(n))
	}
	return v, nil
}

// Markdown renders a task view.
func (v TaskShow) Markdown() string {
	var b strings.Builder
	fmt.Fprintf(&b, "# Task #%d %s [%s]\n\n", v.Task.ID, v.Task.Title, v.Task.Status)
	if v.Plan != nil {
		fmt.Fprintf(&b, "Plan: #%d %s\n\n", v.Plan.ID, v.Plan.Title)
	}
	b.WriteString("## Notes\n")
	b.WriteString(notesMarkdown(v.Notes))
	return b.String()
}

// --- search ---

// MilestoneRef is a compact milestone reference.
type MilestoneRef struct {
	ID     uint64 `json:"id"`
	Title  string `json:"title"`
	Status string `json:"status"`
}

// SearchView holds substring matches across milestones, plans, tasks, issues,
// and notes.
type SearchView struct {
	Term       string         `json:"term"`
	Milestones []MilestoneRef `json:"milestones"`
	Plans      []PlanRef      `json:"plans"`
	Tasks      []TaskLine     `json:"tasks"`
	Issues     []IssueLine    `json:"issues"`
	Notes      []NoteLine     `json:"notes"`
}

// Search matches term (case-insensitive substring) against milestone, plan, and
// task titles, issue titles and bodies, and note bodies.
func Search(s *store.Store, term string) (SearchView, error) {
	needle := strings.ToLower(term)
	v := SearchView{Term: term}
	has := func(s string) bool { return strings.Contains(strings.ToLower(s), needle) }

	milestones, err := s.ListMilestones()
	if err != nil {
		return SearchView{}, err
	}
	for _, m := range milestones {
		if has(m.Title) {
			v.Milestones = append(v.Milestones, MilestoneRef{ID: m.ID, Title: m.Title, Status: string(m.Status)})
		}
	}
	plans, err := s.ListPlans()
	if err != nil {
		return SearchView{}, err
	}
	for _, p := range plans {
		if has(p.Title) {
			v.Plans = append(v.Plans, planRef(p))
		}
	}
	tasks, err := s.ListTasks()
	if err != nil {
		return SearchView{}, err
	}
	for _, t := range tasks {
		if has(t.Title) {
			v.Tasks = append(v.Tasks, taskLine(t))
		}
	}
	issues, err := s.ListIssues()
	if err != nil {
		return SearchView{}, err
	}
	for _, is := range issues {
		if has(is.Title) || has(is.Body) {
			v.Issues = append(v.Issues, issueLine(is))
		}
	}
	notes, err := s.ListNotes()
	if err != nil {
		return SearchView{}, err
	}
	for _, n := range notes {
		if has(n.Body) {
			v.Notes = append(v.Notes, noteLine(n))
		}
	}
	return v, nil
}

// Markdown renders search results grouped by kind.
func (v SearchView) Markdown() string {
	var b strings.Builder
	fmt.Fprintf(&b, "# Search: %q\n\n", v.Term)
	if len(v.Milestones) == 0 && len(v.Plans) == 0 && len(v.Tasks) == 0 && len(v.Issues) == 0 && len(v.Notes) == 0 {
		b.WriteString("_no matches_\n")
		return b.String()
	}
	if len(v.Milestones) > 0 {
		b.WriteString("## Milestones\n")
		for _, m := range v.Milestones {
			fmt.Fprintf(&b, "- #%d %s [%s]\n", m.ID, m.Title, m.Status)
		}
		b.WriteString("\n")
	}
	if len(v.Plans) > 0 {
		b.WriteString("## Plans\n")
		for _, p := range v.Plans {
			fmt.Fprintf(&b, "- #%d %s [%s]\n", p.ID, p.Title, p.Status)
		}
		b.WriteString("\n")
	}
	if len(v.Tasks) > 0 {
		b.WriteString("## Tasks\n")
		for _, t := range v.Tasks {
			fmt.Fprintf(&b, "- [%s] #%d %s (plan %d)\n", t.Status, t.ID, t.Title, t.PlanID)
		}
		b.WriteString("\n")
	}
	if len(v.Issues) > 0 {
		b.WriteString("## Issues\n")
		for _, is := range v.Issues {
			fmt.Fprintf(&b, "- #%d [%s] %s (%s)\n", is.ID, is.Severity, is.Title, is.Status)
		}
		b.WriteString("\n")
	}
	if len(v.Notes) > 0 {
		b.WriteString("## Notes\n")
		b.WriteString(notesMarkdown(v.Notes))
	}
	return b.String()
}

// --- milestone show ---

// MilestoneShow is a milestone with its plans and a task rollup.
type MilestoneShow struct {
	ID        uint64    `json:"id"`
	Title     string    `json:"title"`
	Status    string    `json:"status"`
	Due       string    `json:"due,omitempty"`
	Plans     []PlanRef `json:"plans"`
	TasksDone int       `json:"tasks_done"`
	TasksOpen int       `json:"tasks_open"`
}

// ShowMilestone assembles a full view of one milestone.
func ShowMilestone(s *store.Store, id uint64) (MilestoneShow, error) {
	m, err := s.GetMilestone(id)
	if err != nil {
		return MilestoneShow{}, err
	}
	v := MilestoneShow{ID: m.ID, Title: m.Title, Status: string(m.Status)}
	if !m.Due.IsZero() {
		v.Due = m.Due.Format("2006-01-02")
	}
	plans, err := s.ListPlansByMilestone(id)
	if err != nil {
		return MilestoneShow{}, err
	}
	for _, p := range plans {
		v.Plans = append(v.Plans, planRef(p))
		tasks, err := s.ListTasksByPlan(p.ID)
		if err != nil {
			return MilestoneShow{}, err
		}
		for _, t := range tasks {
			if t.Status == model.TaskDone {
				v.TasksDone++
			} else {
				v.TasksOpen++
			}
		}
	}
	return v, nil
}

// Markdown renders a milestone view.
func (v MilestoneShow) Markdown() string {
	var b strings.Builder
	due := ""
	if v.Due != "" {
		due = " (due " + v.Due + ")"
	}
	fmt.Fprintf(&b, "# Milestone #%d %s [%s]%s\n\n", v.ID, v.Title, v.Status, due)
	fmt.Fprintf(&b, "Tasks: %d done · %d open\n\n", v.TasksDone, v.TasksOpen)
	b.WriteString("## Plans\n")
	if len(v.Plans) == 0 {
		b.WriteString("_none_\n")
		return b.String()
	}
	for _, p := range v.Plans {
		fmt.Fprintf(&b, "- #%d %s [%s]\n", p.ID, p.Title, p.Status)
	}
	return b.String()
}

// --- issue show ---

// IssueShow is a single issue with its linked task (if any).
type IssueShow struct {
	ID       uint64    `json:"id"`
	Title    string    `json:"title"`
	Body     string    `json:"body,omitempty"`
	Status   string    `json:"status"`
	Severity string    `json:"severity"`
	Task     *TaskLine `json:"task"`
}

// ShowIssue assembles a full view of one issue.
func ShowIssue(s *store.Store, id uint64) (IssueShow, error) {
	is, err := s.GetIssue(id)
	if err != nil {
		return IssueShow{}, err
	}
	v := IssueShow{ID: is.ID, Title: is.Title, Body: is.Body, Status: string(is.Status), Severity: string(is.Severity)}
	if is.TaskID != 0 {
		if t, err := s.GetTask(is.TaskID); err == nil {
			tl := taskLine(t)
			v.Task = &tl
		}
	}
	return v, nil
}

// Markdown renders an issue view.
func (v IssueShow) Markdown() string {
	var b strings.Builder
	fmt.Fprintf(&b, "# Issue #%d %s\n\n", v.ID, v.Title)
	fmt.Fprintf(&b, "Status: %s · Severity: %s\n", v.Status, v.Severity)
	if v.Task != nil {
		fmt.Fprintf(&b, "Task: #%d %s\n", v.Task.ID, v.Task.Title)
	}
	if strings.TrimSpace(v.Body) != "" {
		b.WriteString("\n" + v.Body + "\n")
	}
	return b.String()
}

// --- board (kanban) ---

// Board groups a plan's tasks into kanban columns.
type Board struct {
	PlanID    uint64     `json:"plan_id"`
	PlanTitle string     `json:"plan_title"`
	Todo      []TaskLine `json:"todo"`
	Doing     []TaskLine `json:"doing"`
	Blocked   []TaskLine `json:"blocked"`
	Done      []TaskLine `json:"done"`
}

// BoardFor assembles the kanban board for one plan.
func BoardFor(s *store.Store, planID uint64) (Board, error) {
	p, err := s.GetPlan(planID)
	if err != nil {
		return Board{}, err
	}
	tasks, err := s.ListTasksByPlan(planID)
	if err != nil {
		return Board{}, err
	}
	b := Board{PlanID: p.ID, PlanTitle: p.Title}
	for _, t := range tasks {
		line := taskLine(t)
		switch t.Status {
		case model.TaskTodo:
			b.Todo = append(b.Todo, line)
		case model.TaskDoing:
			b.Doing = append(b.Doing, line)
		case model.TaskBlocked:
			b.Blocked = append(b.Blocked, line)
		case model.TaskDone:
			b.Done = append(b.Done, line)
		}
	}
	return b, nil
}

// Markdown renders the board as one section per column.
func (b Board) Markdown() string {
	var sb strings.Builder
	fmt.Fprintf(&sb, "# Board — #%d %s\n\n", b.PlanID, b.PlanTitle)
	cols := []struct {
		name  string
		tasks []TaskLine
	}{
		{"Todo", b.Todo}, {"Doing", b.Doing}, {"Blocked", b.Blocked}, {"Done", b.Done},
	}
	for _, c := range cols {
		fmt.Fprintf(&sb, "## %s (%d)\n", c.name, len(c.tasks))
		if len(c.tasks) == 0 {
			sb.WriteString("_none_\n\n")
			continue
		}
		for _, t := range c.tasks {
			fmt.Fprintf(&sb, "- #%d %s\n", t.ID, t.Title)
		}
		sb.WriteString("\n")
	}
	return sb.String()
}

// notesMarkdown renders a notes list or "_none_".
func notesMarkdown(notes []NoteLine) string {
	if len(notes) == 0 {
		return "_none_\n"
	}
	var b strings.Builder
	for _, n := range notes {
		b.WriteString("- " + noteMarkdown(n) + "\n")
	}
	return b.String()
}
