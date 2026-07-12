// Package tui provides the human-facing Bubble Tea dashboard for ptrack. It is a
// convenience layer over the same store the CLI uses; every action it offers is
// also available as a scriptable subcommand, so the TUI never sits on the
// critical path for AI-agent usage.
package tui

import (
	"os"
	"path/filepath"
	"time"

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

type dashboard struct {
	store  *store.Store
	dbPath string

	meta   model.Meta
	plans  []model.Plan
	tasks  map[uint64][]model.Task
	cursor int
	status string
	width  int
	height int
}

func newModel(s *store.Store, dbPath string) (dashboard, error) {
	d := dashboard{store: s, dbPath: dbPath, tasks: map[uint64][]model.Task{}}
	if err := d.reload(); err != nil {
		return d, err
	}
	// start the cursor on the active plan, if any.
	for i, p := range d.plans {
		if p.ID == d.meta.ActivePlan {
			d.cursor = i
			break
		}
	}
	return d, nil
}

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
	if d.cursor >= len(d.plans) {
		d.cursor = max(0, len(d.plans)-1)
	}
	return nil
}

func (d dashboard) Init() tea.Cmd { return nil }

func (d dashboard) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		d.width, d.height = msg.Width, msg.Height
	case tea.KeyMsg:
		switch msg.String() {
		case "q", "ctrl+c", "esc":
			return d, tea.Quit
		case "up", "k":
			if d.cursor > 0 {
				d.cursor--
			}
		case "down", "j":
			if d.cursor < len(d.plans)-1 {
				d.cursor++
			}
		case "r":
			if err := d.reload(); err != nil {
				d.status = "reload error: " + err.Error()
			} else {
				d.status = "reloaded"
			}
		case "b":
			d.status = d.backup()
		}
	}
	return d, nil
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
