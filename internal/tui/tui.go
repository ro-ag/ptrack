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

// focus identifies which pane currently receives navigation keys.
type focus int

const (
	focusPlans focus = iota
	focusTasks
)

// mode selects the dashboard layout: the two-pane list or the kanban board.
type mode int

const (
	modeList mode = iota
	modeBoard
)

// boardStatuses is the left-to-right column order of the kanban board.
var boardStatuses = []model.TaskStatus{
	model.TaskTodo, model.TaskDoing, model.TaskBlocked, model.TaskDone,
}

var boardTitles = []string{"Todo", "Doing", "Blocked", "Done"}

// inputPurpose records what a pending text-input submission should do.
type inputPurpose int

const (
	inputNone inputPurpose = iota
	inputAddPlan
	inputAddTask
	inputEditGoal
	inputEditSummary
	inputAddNote
)

// dashboard is the Bubble Tea model backing the ptrack TUI.
type dashboard struct {
	store  *store.Store
	dbPath string

	meta  model.Meta
	plans []model.Plan
	tasks map[uint64][]model.Task

	mode       mode
	focus      focus
	planCursor int
	taskCursor int

	boardCol int
	boardRow int

	input   textinput.Model
	purpose inputPurpose

	status string
	width  int
	height int
}

func newModel(s *store.Store, dbPath string) (dashboard, error) {
	d := dashboard{store: s, dbPath: dbPath, tasks: map[uint64][]model.Task{}}
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

// reload refreshes the cached snapshot of meta, plans, and tasks from the store.
func (d *dashboard) reload() error {
	meta, err := d.store.GetMeta()
	if err != nil {
		return err
	}
	plans, err := d.store.ListPlans()
	if err != nil {
		return err
	}
	tasks := make(map[uint64][]model.Task, len(plans))
	for _, p := range plans {
		ts, err := d.store.ListTasksByPlan(p.ID)
		if err != nil {
			return err
		}
		tasks[p.ID] = ts
	}
	d.meta, d.plans, d.tasks = meta, plans, tasks
	d.clampCursors()
	return nil
}

func (d *dashboard) clampCursors() {
	if d.planCursor >= len(d.plans) {
		d.planCursor = max(0, len(d.plans)-1)
	}
	n := len(d.currentTasks())
	if d.taskCursor >= n {
		d.taskCursor = max(0, n-1)
	}
}

// currentPlan returns the plan under the plan cursor, or nil when there are none.
func (d *dashboard) currentPlan() *model.Plan {
	if len(d.plans) == 0 {
		return nil
	}
	return &d.plans[d.planCursor]
}

// currentTasks returns the tasks of the plan under the cursor.
func (d *dashboard) currentTasks() []model.Task {
	p := d.currentPlan()
	if p == nil {
		return nil
	}
	return d.tasks[p.ID]
}

// currentTask returns the task under the task cursor, or nil.
func (d *dashboard) currentTask() *model.Task {
	ts := d.currentTasks()
	if d.taskCursor < 0 || d.taskCursor >= len(ts) {
		return nil
	}
	return &ts[d.taskCursor]
}

// boardColumns groups the current plan's tasks into kanban columns in
// boardStatuses order.
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

// boardTask returns the card under the board cursor, or nil.
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

func (d *dashboard) clampBoardRow() {
	cols := d.boardColumns()
	n := 0
	if d.boardCol >= 0 && d.boardCol < len(cols) {
		n = len(cols[d.boardCol])
	}
	d.boardRow = clamp(d.boardRow, 0, n-1)
}

// moveCard shifts the selected card one column in dir (-1 left, +1 right),
// applying the target column's status.
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
	d.clampBoardRow()
	d.status = "moved #" + itoa(t.ID) + " → " + string(boardStatuses[nc])
}

func (d dashboard) Init() tea.Cmd { return nil }

// Update handles messages, dispatching to input-mode or normal-mode key handling.
func (d dashboard) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		d.width, d.height = msg.Width, msg.Height
		return d, nil
	case tea.KeyMsg:
		if d.purpose != inputNone {
			return d.updateInput(msg)
		}
		if d.mode == modeBoard {
			return d.updateBoard(msg)
		}
		return d.updateNormal(msg)
	}
	return d, nil
}

// updateBoard handles keys while the kanban board is shown.
func (d dashboard) updateBoard(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "q", "ctrl+c":
		return d, tea.Quit
	case "v", "esc":
		d.mode = modeList
	case "left", "h":
		if d.boardCol > 0 {
			d.boardCol--
			d.clampBoardRow()
		}
	case "right", "l":
		if d.boardCol < len(boardStatuses)-1 {
			d.boardCol++
			d.clampBoardRow()
		}
	case "up", "k":
		if d.boardRow > 0 {
			d.boardRow--
		}
	case "down", "j":
		d.boardRow++
		d.clampBoardRow()
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
	case "r":
		d.applyErr(d.reload(), "reloaded")
	case "B":
		d.status = d.backup()
	}
	return d, nil
}

// updateInput routes keys to the text input while a value is being entered.
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

// updateNormal handles navigation and action keys in the default mode.
func (d dashboard) updateNormal(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "q", "ctrl+c", "esc":
		return d, tea.Quit
	case "tab":
		if d.focus == focusPlans {
			d.focus = focusTasks
		} else {
			d.focus = focusPlans
		}
	case "v":
		if d.currentPlan() == nil {
			d.status = "add a plan first"
			return d, nil
		}
		d.mode = modeBoard
		d.boardCol, d.boardRow = 0, 0
	case "up", "k":
		d.moveCursor(-1)
	case "down", "j":
		d.moveCursor(1)
	case "g":
		return d, d.startInput(inputEditGoal, "Goal:", d.meta.Goal)
	case "m":
		return d, d.startInput(inputEditSummary, "Summary:", d.meta.Summary)
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
		if d.focus == focusTasks && d.currentTask() == nil {
			d.status = "no task selected"
			return d, nil
		}
		if d.focus == focusPlans && d.currentPlan() == nil {
			d.status = "nothing to annotate"
			return d, nil
		}
		return d, d.startInput(inputAddNote, "Note:", "")
	case "u": // set active plan
		if p := d.currentPlan(); p != nil {
			if err := d.store.SetActivePlan(p.ID); err != nil {
				d.status = err.Error()
			} else {
				d.status = "active plan set"
				_ = d.reload()
			}
		}
	case "x": // mark plan done
		if p := d.currentPlan(); p != nil {
			d.applyErr(d.store.SetPlanStatus(p.ID, model.PlanDone), "plan done")
		}
	case "s": // task -> doing
		d.setTask(model.TaskDoing, "task started")
	case "d": // task -> done
		d.setTask(model.TaskDone, "task done")
	case "b": // task -> blocked
		d.setTask(model.TaskBlocked, "task blocked")
	case "r":
		d.applyErr(d.reload(), "reloaded")
	case "B": // backup
		d.status = d.backup()
	}
	return d, nil
}

func (d *dashboard) moveCursor(delta int) {
	if d.focus == focusPlans {
		d.planCursor = clamp(d.planCursor+delta, 0, len(d.plans)-1)
		d.taskCursor = 0
		return
	}
	d.taskCursor = clamp(d.taskCursor+delta, 0, len(d.currentTasks())-1)
}

// setTask applies a status to the task under the cursor and reports a status line.
func (d *dashboard) setTask(st model.TaskStatus, ok string) {
	t := d.currentTask()
	if t == nil {
		d.status = "no task selected"
		return
	}
	d.applyErr(d.store.SetTaskStatus(t.ID, st), ok)
}

// applyErr reloads and sets a status line, reporting err when non-nil.
func (d *dashboard) applyErr(err error, ok string) {
	if err != nil {
		d.status = err.Error()
		return
	}
	_ = d.reload()
	d.status = ok
}

// startInput enters text-input mode for the given purpose, seeding the field.
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

// commitInput applies the entered value according to the pending purpose.
func (d *dashboard) commitInput() {
	val := strings.TrimSpace(d.input.Value())
	p := d.purpose
	d.purpose = inputNone

	switch p {
	case inputEditGoal:
		d.applyErr(d.store.SetGoal(val), "goal updated")
	case inputEditSummary:
		d.applyErr(d.store.SetSummary(val), "summary updated")
	case inputAddPlan:
		if val == "" {
			d.status = "cancelled"
			return
		}
		if pl, err := d.store.AddPlan(val); err != nil {
			d.status = err.Error()
		} else {
			d.status = "added plan #" + itoa(pl.ID)
			_ = d.reload()
		}
	case inputAddTask:
		if val == "" {
			d.status = "cancelled"
			return
		}
		pl := d.currentPlan()
		if pl == nil {
			d.status = "no plan selected"
			return
		}
		if t, err := d.store.AddTask(pl.ID, val); err != nil {
			d.status = err.Error()
		} else {
			d.status = "added task #" + itoa(t.ID)
			_ = d.reload()
		}
	case inputAddNote:
		if val == "" {
			d.status = "cancelled"
			return
		}
		d.addNote(val)
	}
}

// addNote attaches a note to the selected target: the board card in board mode,
// the selected task in the tasks pane, else the current plan, else the project.
func (d *dashboard) addNote(body string) {
	if d.mode == modeBoard {
		if t := d.boardTask(); t != nil {
			d.applyErr(mapErr(d.store.AddNote(model.TargetTask, t.ID, body)), "note added")
			return
		}
	}
	if d.focus == focusTasks {
		if t := d.currentTask(); t != nil {
			d.applyErr(mapErr(d.store.AddNote(model.TargetTask, t.ID, body)), "note added")
			return
		}
	}
	if p := d.currentPlan(); p != nil {
		d.applyErr(mapErr(d.store.AddNote(model.TargetPlan, p.ID, body)), "note added")
		return
	}
	d.applyErr(mapErr(d.store.AddNote(model.TargetProject, 0, body)), "note added")
}

// backup copies the project DB into the global backups dir and records it,
// returning a status line for the footer.
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

func clamp(v, lo, hi int) int {
	if hi < lo {
		return lo
	}
	if v < lo {
		return lo
	}
	if v > hi {
		return hi
	}
	return v
}

// mapErr discards the value from an (T, error) result, keeping only the error,
// so applyErr can consume store methods that also return the created record.
func mapErr[T any](_ T, err error) error { return err }

func itoa(v uint64) string { return strconv.FormatUint(v, 10) }
