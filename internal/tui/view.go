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

// View renders the dashboard: a goal/summary header, the list or kanban body,
// an optional input line, and a contextual help footer.
func (d dashboard) View() string {
	var h strings.Builder
	h.WriteString(titleStyle.Render("ptrack"))
	h.WriteString("\n")
	h.WriteString(labelStyle.Render("Goal: ") + orUnset(d.meta.Goal) + "\n")
	h.WriteString(labelStyle.Render("Summary: ") + orUnset(d.meta.Summary) + "\n")
	header := headerBorder.Render(h.String())

	var body string
	if d.mode == modeBoard {
		body = d.renderBoard()
	} else {
		body = lipgloss.JoinHorizontal(lipgloss.Top,
			columnStyle.Render(d.renderPlans()),
			d.renderTasks(),
		)
	}

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

// columnAccent maps each board column to an accent color.
var columnAccent = []lipgloss.Color{
	lipgloss.Color("245"), // todo — gray
	lipgloss.Color("214"), // doing — amber
	lipgloss.Color("196"), // blocked — red
	lipgloss.Color("42"),  // done — green
}

// renderBoard draws the kanban board for the current plan: four bordered
// columns with status-accented headers, cards, and a highlighted selection.
func (d dashboard) renderBoard() string {
	p := d.currentPlan()
	if p == nil {
		return dimStyle.Render("no plan selected")
	}
	cols := d.boardColumns()
	colW := 22
	if d.width > 0 {
		if w := (d.width - 6) / len(boardStatuses); w > colW {
			colW = w
		}
	}

	rendered := make([]string, len(boardStatuses))
	for i := range boardStatuses {
		accent := columnAccent[i]
		head := lipgloss.NewStyle().Bold(true).Foreground(accent).
			Render(fmt.Sprintf("%s (%d)", boardTitles[i], len(cols[i])))

		var cards strings.Builder
		cards.WriteString(head + "\n\n")
		if len(cols[i]) == 0 {
			cards.WriteString(dimStyle.Render("—"))
		}
		for row, t := range cols[i] {
			card := fmt.Sprintf("#%d %s", t.ID, t.Title)
			card = truncate(card, colW-4)
			cardStyle := lipgloss.NewStyle().Foreground(accent)
			if i == d.boardCol && row == d.boardRow {
				cardStyle = lipgloss.NewStyle().Bold(true).
					Foreground(lipgloss.Color("231")).Background(accent).Padding(0, 1)
			}
			cards.WriteString(cardStyle.Render(card) + "\n")
		}

		border := lipgloss.RoundedBorder()
		box := lipgloss.NewStyle().
			Border(border).
			BorderForeground(accent).
			Width(colW).
			Padding(0, 1).
			MarginRight(1)
		if i == d.boardCol {
			box = box.BorderForeground(lipgloss.Color("212"))
		}
		rendered[i] = box.Render(cards.String())
	}
	title := labelStyle.Render(fmt.Sprintf("Board — #%d %s", p.ID, p.Title))
	return title + "\n\n" + lipgloss.JoinHorizontal(lipgloss.Top, rendered...)
}

// truncate shortens s to n runes, appending an ellipsis when cut.
func truncate(s string, n int) string {
	if n < 1 {
		n = 1
	}
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	if n <= 1 {
		return string(r[:n])
	}
	return string(r[:n-1]) + "…"
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
	if d.mode == modeBoard {
		return dimStyle.Render("←/→ column · ↑/↓ card · H/L move card · a add · n note · v list · r reload · B backup · q quit")
	}
	common := "tab pane · ↑/↓ move · v board · a add · n note · g goal · m summary · r reload · B backup · q quit"
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
