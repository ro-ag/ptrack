// Package tui provides the human-facing Bubble Tea dashboard for ptrack. It is a
// convenience layer over the same store the CLI uses; every action it offers is
// also available as a scriptable subcommand, so the TUI never sits on the
// critical path for AI-agent usage.
package tui

import (
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

func trim(s string) string { return strings.TrimSpace(s) }

// Run opens the current project's store and launches the dashboard. It returns
// store.ErrNoProject (with guidance) when run outside a ptrack project.
func Run() error {
	cwd, err := os.Getwd()
	if err != nil {
		return err
	}
	dbPath, err := store.FindProjectDB(cwd)
	if err != nil {
		return err
	}
	s, err := store.Open(dbPath)
	if err != nil {
		return err
	}
	defer s.Close()

	m, err := newModel(s, dbPath)
	if err != nil {
		return err
	}
	_, err = tea.NewProgram(m, tea.WithAltScreen()).Run()
	return err
}

type tab int

const (
	tabOverview tab = iota
	tabBoard
	tabMilestones
	tabIssues
	tabCount
)

var tabNames = []string{"Overview", "Board", "Milestones", "Issues"}

type paneFocus int

const (
	focusPlans paneFocus = iota
	focusTasks
)

type inputPurpose int

const (
	inputNone inputPurpose = iota
	inputAddPlan
	inputAddTask
	inputAddMilestone
	inputAddIssue
	inputAddNote
	inputEditGoal
	inputEditSummary
	inputRename
)

var boardStatuses = []model.TaskStatus{
	model.TaskTodo, model.TaskDoing, model.TaskBlocked, model.TaskDone,
}
var boardTitles = []string{"Todo", "Doing", "Blocked", "Done"}

type dashboard struct {
	store  *store.Store
	dbPath string

	meta        model.Meta
	milestones  []model.Milestone
	plans       []model.Plan
	tasksByPlan map[uint64][]model.Task
	issues      []model.Issue
	counts      model.Counts

	tab   tab
	focus paneFocus

	planCursor  int
	taskCursor  int
	boardCol    int
	boardRow    int
	msCursor    int
	issueCursor int

	input   textinput.Model
	purpose inputPurpose

	status string
	width  int
	height int
}

func newModel(s *store.Store, dbPath string) (dashboard, error) {
	d := dashboard{store: s, dbPath: dbPath, tasksByPlan: map[uint64][]model.Task{}}
	if err := d.reload(); err != nil {
		return d, err
	}
	for i, p := range d.plans {
		if p.ID == d.meta.ActivePlan {
			d.planCursor = i
			break
		}
	}
	return d, nil
}

func (d *dashboard) reload() error {
	var err error
	if d.meta, err = d.store.GetMeta(); err != nil {
		return err
	}
	if d.milestones, err = d.store.ListMilestones(); err != nil {
		return err
	}
	if d.plans, err = d.store.ListPlans(); err != nil {
		return err
	}
	d.tasksByPlan = make(map[uint64][]model.Task, len(d.plans))
	for _, p := range d.plans {
		ts, err := d.store.ListTasksByPlan(p.ID)
		if err != nil {
			return err
		}
		d.tasksByPlan[p.ID] = ts
	}
	if d.issues, err = d.store.ListIssues(); err != nil {
		return err
	}
	if d.counts, err = d.store.Counts(); err != nil {
		return err
	}
	d.clampCursors()
	return nil
}

func (d *dashboard) clampCursors() {
	d.planCursor = clamp(d.planCursor, 0, len(d.plans)-1)
	d.taskCursor = clamp(d.taskCursor, 0, len(d.currentTasks())-1)
	d.msCursor = clamp(d.msCursor, 0, len(d.milestones)-1)
	d.issueCursor = clamp(d.issueCursor, 0, len(d.issues)-1)
	d.boardRow = clamp(d.boardRow, 0, len(d.boardColumns()[d.boardCol])-1)
}

// --- selection helpers ---

func (d *dashboard) currentPlan() *model.Plan {
	if len(d.plans) == 0 {
		return nil
	}
	return &d.plans[d.planCursor]
}

func (d *dashboard) currentTasks() []model.Task {
	p := d.currentPlan()
	if p == nil {
		return nil
	}
	return d.tasksByPlan[p.ID]
}

func (d *dashboard) currentTask() *model.Task {
	ts := d.currentTasks()
	if d.taskCursor < 0 || d.taskCursor >= len(ts) {
		return nil
	}
	return &ts[d.taskCursor]
}

func (d *dashboard) currentMilestone() *model.Milestone {
	if len(d.milestones) == 0 {
		return nil
	}
	return &d.milestones[d.msCursor]
}

func (d *dashboard) currentIssue() *model.Issue {
	if len(d.issues) == 0 {
		return nil
	}
	return &d.issues[d.issueCursor]
}

func (d *dashboard) boardColumns() [][]model.Task {
	cols := make([][]model.Task, len(boardStatuses))
	for _, t := range d.currentTasks() {
		for i, st := range boardStatuses {
			if t.Status == st {
				cols[i] = append(cols[i], t)
				break
			}
		}
	}
	return cols
}

func (d *dashboard) boardTask() *model.Task {
	cols := d.boardColumns()
	if d.boardCol < 0 || d.boardCol >= len(cols) {
		return nil
	}
	col := cols[d.boardCol]
	if d.boardRow < 0 || d.boardRow >= len(col) {
		return nil
	}
	return &col[d.boardRow]
}

// --- bubbletea ---

func (d dashboard) Init() tea.Cmd { return nil }

func (d dashboard) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		d.width, d.height = msg.Width, msg.Height
		return d, nil
	case tea.KeyMsg:
		if d.purpose != inputNone {
			return d.updateInput(msg)
		}
		return d.updateKey(msg)
	}
	return d, nil
}

func (d dashboard) updateInput(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "enter":
		d.commitInput()
		return d, nil
	case "esc":
		d.purpose = inputNone
		d.status = "cancelled"
		return d, nil
	}
	var cmd tea.Cmd
	d.input, cmd = d.input.Update(msg)
	return d, cmd
}

func (d dashboard) updateKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	// Global keys.
	switch msg.String() {
	case "q", "ctrl+c":
		return d, tea.Quit
	case "tab":
		d.tab = (d.tab + 1) % tabCount
		return d, nil
	case "shift+tab":
		d.tab = (d.tab + tabCount - 1) % tabCount
		return d, nil
	case "1":
		d.tab = tabOverview
		return d, nil
	case "2":
		d.tab = tabBoard
		return d, nil
	case "3":
		d.tab = tabMilestones
		return d, nil
	case "4":
		d.tab = tabIssues
		return d, nil
	case "g":
		return d, d.startInput(inputEditGoal, "Goal:", d.meta.Goal)
	case "m":
		return d, d.startInput(inputEditSummary, "Summary:", d.meta.Summary)
	case "e":
		if _, _, title, ok := d.renameTarget(); ok {
			return d, d.startInput(inputRename, "Rename:", title)
		}
		d.status = "nothing to rename"
		return d, nil
	case "r":
		d.applyErr(d.reload(), "reloaded")
		return d, nil
	case "B":
		d.status = d.backup()
		return d, nil
	}

	switch d.tab {
	case tabOverview:
		return d.updateOverview(msg)
	case tabBoard:
		return d.updateBoard(msg)
	case tabMilestones:
		return d.updateMilestones(msg)
	case tabIssues:
		return d.updateIssues(msg)
	}
	return d, nil
}

func (d dashboard) updateOverview(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "left", "h", "right", "l":
		if d.focus == focusPlans {
			d.focus = focusTasks
		} else {
			d.focus = focusPlans
		}
	case "up", "k":
		d.moveOverview(-1)
	case "down", "j":
		d.moveOverview(1)
	case "a":
		if d.focus == focusPlans {
			return d, d.startInput(inputAddPlan, "New plan:", "")
		}
		if d.currentPlan() == nil {
			d.status = "add a plan first"
			return d, nil
		}
		return d, d.startInput(inputAddTask, "New task:", "")
	case "n":
		return d, d.startInput(inputAddNote, "Note:", "")
	case "u":
		if p := d.currentPlan(); p != nil {
			d.applyErr(d.store.SetActivePlan(p.ID), "active plan set")
		}
	case "x":
		if p := d.currentPlan(); p != nil {
			d.applyErr(d.store.SetPlanStatus(p.ID, model.PlanDone), "plan done")
		}
	case "s":
		d.setTask(model.TaskDoing, "task started")
	case "d":
		d.setTask(model.TaskDone, "task done")
	case "b":
		d.setTask(model.TaskBlocked, "task blocked")
	}
	return d, nil
}

func (d *dashboard) moveOverview(delta int) {
	if d.focus == focusPlans {
		d.planCursor = clamp(d.planCursor+delta, 0, len(d.plans)-1)
		d.taskCursor = 0
		return
	}
	d.taskCursor = clamp(d.taskCursor+delta, 0, len(d.currentTasks())-1)
}

func (d *dashboard) setTask(st model.TaskStatus, ok string) {
	t := d.currentTask()
	if t == nil {
		d.status = "no task selected"
		return
	}
	d.applyErr(d.store.SetTaskStatus(t.ID, st), ok)
}

func (d dashboard) updateBoard(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "left", "h":
		if d.boardCol > 0 {
			d.boardCol--
			d.boardRow = clamp(d.boardRow, 0, len(d.boardColumns()[d.boardCol])-1)
		}
	case "right", "l":
		if d.boardCol < len(boardStatuses)-1 {
			d.boardCol++
			d.boardRow = clamp(d.boardRow, 0, len(d.boardColumns()[d.boardCol])-1)
		}
	case "up", "k":
		if d.boardRow > 0 {
			d.boardRow--
		}
	case "down", "j":
		d.boardRow = clamp(d.boardRow+1, 0, len(d.boardColumns()[d.boardCol])-1)
	case "H", "<":
		d.moveCard(-1)
	case "L", ">":
		d.moveCard(1)
	case "a":
		if d.currentPlan() == nil {
			d.status = "add a plan first"
			return d, nil
		}
		return d, d.startInput(inputAddTask, "New task:", "")
	case "n":
		if d.boardTask() == nil {
			d.status = "no card selected"
			return d, nil
		}
		return d, d.startInput(inputAddNote, "Note:", "")
	}
	return d, nil
}

func (d *dashboard) moveCard(dir int) {
	t := d.boardTask()
	if t == nil {
		d.status = "no card selected"
		return
	}
	nc := d.boardCol + dir
	if nc < 0 || nc >= len(boardStatuses) {
		return
	}
	if err := d.store.SetTaskStatus(t.ID, boardStatuses[nc]); err != nil {
		d.status = err.Error()
		return
	}
	_ = d.reload()
	d.boardCol = nc
	d.boardRow = clamp(d.boardRow, 0, len(d.boardColumns()[d.boardCol])-1)
	d.status = "moved #" + strconv.FormatUint(t.ID, 10) + " → " + string(boardStatuses[nc])
}

func (d dashboard) updateMilestones(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "up", "k":
		d.msCursor = clamp(d.msCursor-1, 0, len(d.milestones)-1)
	case "down", "j":
		d.msCursor = clamp(d.msCursor+1, 0, len(d.milestones)-1)
	case "a":
		return d, d.startInput(inputAddMilestone, "New milestone:", "")
	case "x":
		if m := d.currentMilestone(); m != nil {
			d.applyErr(d.store.SetMilestoneStatus(m.ID, model.MilestoneDone), "milestone done")
		}
	case "o":
		if m := d.currentMilestone(); m != nil {
			d.applyErr(d.store.SetMilestoneStatus(m.ID, model.MilestoneOpen), "milestone reopened")
		}
	}
	return d, nil
}

func (d dashboard) updateIssues(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "up", "k":
		d.issueCursor = clamp(d.issueCursor-1, 0, len(d.issues)-1)
	case "down", "j":
		d.issueCursor = clamp(d.issueCursor+1, 0, len(d.issues)-1)
	case "a":
		return d, d.startInput(inputAddIssue, "New issue:", "")
	case "c":
		if is := d.currentIssue(); is != nil {
			d.applyErr(d.store.SetIssueStatus(is.ID, model.IssueClosed), "issue closed")
		}
	case "o":
		if is := d.currentIssue(); is != nil {
			d.applyErr(d.store.SetIssueStatus(is.ID, model.IssueOpen), "issue reopened")
		}
	}
	return d, nil
}

// --- input handling ---

func (d *dashboard) startInput(p inputPurpose, prompt, initial string) tea.Cmd {
	ti := textinput.New()
	ti.Prompt = prompt + " "
	ti.SetValue(initial)
	ti.CursorEnd()
	ti.Focus()
	d.input = ti
	d.purpose = p
	d.status = ""
	return textinput.Blink
}

func (d *dashboard) commitInput() {
	val := trim(d.input.Value())
	p := d.purpose
	d.purpose = inputNone

	switch p {
	case inputEditGoal:
		d.applyErr(d.store.SetGoal(val), "goal updated")
	case inputEditSummary:
		d.applyErr(d.store.SetSummary(val), "summary updated")
	case inputAddPlan:
		d.addNamed(val, func() (uint64, error) { p, e := d.store.AddPlan(val); return p.ID, e }, "plan")
	case inputAddMilestone:
		d.addNamed(val, func() (uint64, error) { m, e := d.store.AddMilestone(val); return m.ID, e }, "milestone")
	case inputAddIssue:
		d.addNamed(val, func() (uint64, error) { is, e := d.store.AddIssue(val, "", "", 0); return is.ID, e }, "issue")
	case inputAddTask:
		pl := d.currentPlan()
		if pl == nil {
			d.status = "no plan selected"
			return
		}
		d.addNamed(val, func() (uint64, error) { t, e := d.store.AddTask(pl.ID, val); return t.ID, e }, "task")
	case inputAddNote:
		if val == "" {
			d.status = "cancelled"
			return
		}
		d.addNote(val)
	case inputRename:
		d.rename(val)
	}
}

func (d *dashboard) addNamed(val string, create func() (uint64, error), kind string) {
	if val == "" {
		d.status = "cancelled"
		return
	}
	id, err := create()
	if err != nil {
		d.status = err.Error()
		return
	}
	_ = d.reload()
	d.status = "added " + kind + " #" + strconv.FormatUint(id, 10)
}

// renameTarget resolves the entity the rename action applies to, based on the
// current tab and selection.
func (d *dashboard) renameTarget() (kind string, id uint64, title string, ok bool) {
	switch d.tab {
	case tabIssues:
		if is := d.currentIssue(); is != nil {
			return "issue", is.ID, is.Title, true
		}
	case tabMilestones:
		if m := d.currentMilestone(); m != nil {
			return "milestone", m.ID, m.Title, true
		}
	case tabBoard:
		if t := d.boardTask(); t != nil {
			return "task", t.ID, t.Title, true
		}
	case tabOverview:
		if d.focus == focusTasks {
			if t := d.currentTask(); t != nil {
				return "task", t.ID, t.Title, true
			}
		}
		if p := d.currentPlan(); p != nil {
			return "plan", p.ID, p.Title, true
		}
	}
	return "", 0, "", false
}

func (d *dashboard) rename(val string) {
	kind, id, _, ok := d.renameTarget()
	if !ok || val == "" {
		d.status = "nothing to rename"
		return
	}
	var err error
	switch kind {
	case "plan":
		err = d.store.SetPlanTitle(id, val)
	case "task":
		err = d.store.SetTaskTitle(id, val)
	case "milestone":
		err = d.store.SetMilestoneTitle(id, val)
	case "issue":
		err = d.store.SetIssueTitle(id, val)
	}
	d.applyErr(err, "renamed")
}

func (d *dashboard) addNote(body string) {
	var err error
	switch {
	case d.tab == tabIssues && d.currentIssue() != nil:
		// notes don't attach to issues; record against the project.
		_, err = d.store.AddNote(model.TargetProject, 0, body)
	case (d.tab == tabBoard) && d.boardTask() != nil:
		_, err = d.store.AddNote(model.TargetTask, d.boardTask().ID, body)
	case d.tab == tabOverview && d.focus == focusTasks && d.currentTask() != nil:
		_, err = d.store.AddNote(model.TargetTask, d.currentTask().ID, body)
	case d.currentPlan() != nil:
		_, err = d.store.AddNote(model.TargetPlan, d.currentPlan().ID, body)
	default:
		_, err = d.store.AddNote(model.TargetProject, 0, body)
	}
	d.applyErr(err, "note added")
}

func (d *dashboard) applyErr(err error, ok string) {
	if err != nil {
		d.status = err.Error()
		return
	}
	_ = d.reload()
	d.status = ok
}

func (d dashboard) backup() string {
	home, err := store.GlobalHome()
	if err != nil {
		return "backup error: " + err.Error()
	}
	dst, err := store.BackupProject(d.dbPath, filepath.Join(home, "backups"), time.Now().Unix())
	if err != nil {
		return "backup error: " + err.Error()
	}
	if g, err := store.OpenGlobal(); err == nil {
		_ = g.RecordBackup(d.dbPath, dst)
		_ = g.Close()
	}
	return "backed up → " + dst
}
