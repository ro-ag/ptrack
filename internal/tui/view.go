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
	focusStyle   = lipgloss.NewStyle().Bold(true).Underline(true).Foreground(lipgloss.Color("212"))
	columnStyle  = lipgloss.NewStyle().Padding(0, 3, 0, 0)
	headerBorder = lipgloss.NewStyle().Border(lipgloss.NormalBorder(), false, false, true, false).
			BorderForeground(lipgloss.Color("240"))
)

// View renders the dashboard: a goal/summary header, side-by-side plans and
// tasks panes, an optional input line, and a contextual help footer.
func (d dashboard) View() string {
	var h strings.Builder
	h.WriteString(titleStyle.Render("ptrack"))
	h.WriteString("\n")
	h.WriteString(labelStyle.Render("Goal: ") + orUnset(d.meta.Goal) + "\n")
	h.WriteString(labelStyle.Render("Summary: ") + orUnset(d.meta.Summary) + "\n")
	header := headerBorder.Render(h.String())

	body := lipgloss.JoinHorizontal(lipgloss.Top,
		columnStyle.Render(d.renderPlans()),
		d.renderTasks(),
	)

	var footer string
	if d.purpose != inputNone {
		footer = d.input.View() + "\n" + dimStyle.Render("enter confirm · esc cancel")
	} else {
		footer = d.help()
		if d.status != "" {
			footer = statusStyle.Render(d.status) + "\n" + footer
		}
	}

	return lipgloss.JoinVertical(lipgloss.Left, header, "", body, "", footer)
}

func (d dashboard) paneTitle(name string, f focus) string {
	if d.focus == f && d.purpose == inputNone {
		return focusStyle.Render(name)
	}
	return labelStyle.Render(name)
}

func (d dashboard) renderPlans() string {
	var b strings.Builder
	b.WriteString(d.paneTitle("Plans", focusPlans) + "\n")
	if len(d.plans) == 0 {
		b.WriteString(dimStyle.Render("  (press 'a' to add one)"))
		return b.String()
	}
	for i, p := range d.plans {
		marker := "  "
		if i == d.planCursor && d.focus == focusPlans {
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
	b.WriteString(d.paneTitle("Tasks", focusTasks) + "\n")
	tasks := d.currentTasks()
	if d.currentPlan() == nil {
		return b.String()
	}
	if len(tasks) == 0 {
		b.WriteString(dimStyle.Render("  (press 'a' to add a task)"))
		return b.String()
	}
	for i, t := range tasks {
		marker := "  "
		if i == d.taskCursor && d.focus == focusTasks {
			marker = cursorStyle.Render("▸ ")
		}
		b.WriteString(marker + fmt.Sprintf("%s #%d %s\n", statusIcon(t.Status), t.ID, t.Title))
	}
	return b.String()
}

func (d dashboard) help() string {
	common := "tab pane · ↑/↓ move · a add · n note · g goal · m summary · r reload · B backup · q quit"
	var ctx string
	if d.focus == focusPlans {
		ctx = "u set-active · x done"
	} else {
		ctx = "s start · d done · b block"
	}
	return dimStyle.Render(ctx + " · " + common)
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
