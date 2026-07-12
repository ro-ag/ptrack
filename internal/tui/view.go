package tui

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"
	"github.com/ro-ag/ptrack/internal/model"
)

var (
	titleStyle   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("205"))
	labelStyle   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("39"))
	dimStyle     = lipgloss.NewStyle().Foreground(lipgloss.Color("241"))
	activeStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("42")).Bold(true)
	cursorStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("212")).Bold(true)
	statusStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("214"))
	columnStyle  = lipgloss.NewStyle().Padding(0, 2, 0, 0)
	headerBorder = lipgloss.NewStyle().Border(lipgloss.NormalBorder(), false, false, true, false).
			BorderForeground(lipgloss.Color("240"))
)

func (d dashboard) View() string {
	var b strings.Builder

	// Header: goal + summary.
	b.WriteString(titleStyle.Render("ptrack"))
	b.WriteString("\n")
	b.WriteString(labelStyle.Render("Goal: ") + orUnset(d.meta.Goal) + "\n")
	b.WriteString(labelStyle.Render("Summary: ") + orUnset(d.meta.Summary) + "\n")
	header := headerBorder.Render(b.String())

	// Body: plans column + tasks column.
	plans := d.renderPlans()
	tasks := d.renderTasks()
	body := lipgloss.JoinHorizontal(lipgloss.Top, columnStyle.Render(plans), tasks)

	footer := dimStyle.Render("↑/↓ move · b backup · r reload · q quit")
	if d.status != "" {
		footer = statusStyle.Render(d.status) + "\n" + footer
	}

	return lipgloss.JoinVertical(lipgloss.Left, header, "", body, "", footer)
}

func (d dashboard) renderPlans() string {
	var b strings.Builder
	b.WriteString(labelStyle.Render("Plans") + "\n")
	if len(d.plans) == 0 {
		b.WriteString(dimStyle.Render("  (none — add one with 'ptrack plan add')"))
		return b.String()
	}
	for i, p := range d.plans {
		marker := "  "
		if i == d.cursor {
			marker = cursorStyle.Render("▸ ")
		}
		line := fmt.Sprintf("#%d %s", p.ID, p.Title)
		if p.ID == d.meta.ActivePlan {
			line = activeStyle.Render(line + " *")
		}
		b.WriteString(marker + line + dimStyle.Render("  ["+string(p.Status)+"]") + "\n")
	}
	return b.String()
}

func (d dashboard) renderTasks() string {
	var b strings.Builder
	b.WriteString(labelStyle.Render("Tasks") + "\n")
	if len(d.plans) == 0 {
		return b.String()
	}
	p := d.plans[d.cursor]
	ts := d.tasks[p.ID]
	if len(ts) == 0 {
		b.WriteString(dimStyle.Render("  (no tasks)"))
		return b.String()
	}
	for _, t := range ts {
		b.WriteString(fmt.Sprintf("  %s #%d %s\n", statusIcon(t.Status), t.ID, t.Title))
	}
	return b.String()
}

func statusIcon(s model.TaskStatus) string {
	switch s {
	case model.TaskDone:
		return activeStyle.Render("✓")
	case model.TaskDoing:
		return statusStyle.Render("◐")
	case model.TaskBlocked:
		return lipgloss.NewStyle().Foreground(lipgloss.Color("196")).Render("✗")
	default:
		return dimStyle.Render("○")
	}
}

func orUnset(s string) string {
	if strings.TrimSpace(s) == "" {
		return dimStyle.Render("(unset)")
	}
	return s
}
