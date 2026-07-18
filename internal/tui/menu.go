package tui

import tea "github.com/charmbracelet/bubbletea"

type menuAction int

const (
	menuOverview menuAction = iota
	menuBoard
	menuMilestones
	menuIssues
	menuMaintenance
	menuGoal
	menuSummary
	menuReload
	menuBackup
)

type menuItem struct {
	group       string
	key         string
	title       string
	description string
	action      menuAction
}

var commandMenu = []menuItem{
	{group: "Navigate", key: "1", title: "Overview", description: "Plans and tasks", action: menuOverview},
	{group: "Navigate", key: "2", title: "Board", description: "Kanban workflow", action: menuBoard},
	{group: "Navigate", key: "3", title: "Milestones", description: "Project checkpoints", action: menuMilestones},
	{group: "Navigate", key: "4", title: "Issues", description: "Problems and bugs", action: menuIssues},
	{group: "Navigate", key: "5", title: "Maintenance", description: "Storage health and upkeep", action: menuMaintenance},
	{group: "Project", key: "g", title: "Edit goal", description: "Update the north star", action: menuGoal},
	{group: "Project", key: "m", title: "Edit summary", description: "Refresh handoff context", action: menuSummary},
	{group: "Maintain", key: "r", title: "Reload", description: "Read the latest project state", action: menuReload},
	{group: "Maintain", key: "B", title: "Create backup", description: "Copy the project database", action: menuBackup},
}

func (d dashboard) updateMenu(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "q", "ctrl+c":
		return d, tea.Quit
	case "?", "f1", "esc":
		d.showMenu = false
		return d, nil
	case "up", "k":
		d.menuCursor = clamp(d.menuCursor-1, 0, len(commandMenu)-1)
		return d, nil
	case "down", "j":
		d.menuCursor = clamp(d.menuCursor+1, 0, len(commandMenu)-1)
		return d, nil
	case "enter":
		return d.runMenuAction(commandMenu[d.menuCursor].action)
	}

	for _, item := range commandMenu {
		if msg.String() == item.key {
			return d.runMenuAction(item.action)
		}
	}
	return d, nil
}

func (d dashboard) runMenuAction(action menuAction) (tea.Model, tea.Cmd) {
	d.showMenu = false
	switch action {
	case menuOverview:
		d.showDetail = false
		d.tab = tabOverview
	case menuBoard:
		d.showDetail = false
		d.tab = tabBoard
	case menuMilestones:
		d.showDetail = false
		d.tab = tabMilestones
	case menuIssues:
		d.showDetail = false
		d.tab = tabIssues
	case menuMaintenance:
		d.showDetail = false
		d.tab = tabMaintenance
	case menuGoal:
		return d, d.startInput(inputEditGoal, "Goal:", d.meta.Goal)
	case menuSummary:
		return d, d.startInput(inputEditSummary, "Summary:", d.meta.Summary)
	case menuReload:
		d.applyErr(d.reload(), "project reloaded")
		if d.showDetail {
			d.openDetail()
		}
	case menuBackup:
		d.status = d.backup()
	}
	return d, nil
}
