package tui

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"
	"github.com/ro-ag/ptrack/internal/model"
)

// View composes the header, tab bar, active-tab body, and footer.
func (d dashboard) View() string {
	w := d.width
	if w <= 0 {
		w = 100
	}
	h := d.height
	if h <= 0 {
		h = 30
	}

	header := d.header(w)
	tabs := d.tabBar()
	footer := d.footer(w)

	// Body height = window - header - tabs - footer - separators.
	used := lipgloss.Height(header) + lipgloss.Height(tabs) + lipgloss.Height(footer) + 3
	bodyH := h - used
	if bodyH < 6 {
		bodyH = 6
	}

	var body string
	switch {
	case d.showDetail:
		body = d.viewDetail(w, bodyH)
	case d.tab == tabOverview:
		body = d.viewOverview(w, bodyH)
	case d.tab == tabBoard:
		body = d.viewBoard(w, bodyH)
	case d.tab == tabMilestones:
		body = d.viewMilestones(w, bodyH)
	case d.tab == tabIssues:
		body = d.viewIssues(w, bodyH)
	}

	return lipgloss.JoinVertical(lipgloss.Left, header, tabs, "", body, "", footer)
}

func (d dashboard) header(w int) string {
	c := d.counts
	badges := strings.Join([]string{
		badge("milestones", c.Milestones, fmt.Sprintf("%d done", c.MilestonesDone), cPink),
		badge("plans", c.Plans, fmt.Sprintf("%d done", c.PlansDone), cBlue),
		badge("tasks", c.Tasks, fmt.Sprintf("%d done", c.TasksDone), cGreen),
		badge("issues", c.Issues, fmt.Sprintf("%d open", c.IssuesOpen), cRed),
	}, "  ")

	title := appTitleStyle.Render("ptrack")
	goal := labelStyle.Render("Goal ") + textStyle.Render(truncate(orUnset(d.meta.Goal), w-8))
	summary := dimStyle.Render(truncate("— "+orUnset(d.meta.Summary), w-2))

	top := lipgloss.JoinHorizontal(lipgloss.Left, title, "   ", badges)
	inner := lipgloss.JoinVertical(lipgloss.Left, top, goal, summary)
	return lipgloss.NewStyle().
		Border(lipgloss.NormalBorder(), false, false, true, false).
		BorderForeground(cBorder).
		Width(w - 1).
		Render(inner)
}

func badge(name string, total int, detail string, col lipgloss.Color) string {
	return lipgloss.NewStyle().Foreground(col).Bold(true).Render(fmt.Sprintf("%d", total)) +
		dimStyle.Render(fmt.Sprintf(" %s (%s)", name, detail))
}

func (d dashboard) tabBar() string {
	parts := make([]string, len(tabNames))
	for i, name := range tabNames {
		label := fmt.Sprintf("%d %s", i+1, name)
		if tab(i) == d.tab {
			parts[i] = tabActiveStyle.Render(label)
		} else {
			parts[i] = tabInactiveStyle.Render(label)
		}
	}
	return lipgloss.JoinHorizontal(lipgloss.Left, parts...)
}

// --- overview ---

func (d dashboard) viewOverview(w, h int) string {
	leftW := w/2 - 2
	rightW := w - leftW - 4

	// Plans panel.
	var pl strings.Builder
	rows := len(d.plans)
	start, end := windowRange(rows, d.planCursor, h-2)
	for i := start; i < end; i++ {
		p := d.plans[i]
		sel := i == d.planCursor && d.focus == focusPlans
		mark := "  "
		if sel {
			mark = cursorStyle.Render("▸ ")
		}
		title := fmt.Sprintf("#%d %s", p.ID, p.Title)
		line := lipgloss.NewStyle().Foreground(planStatusColor(p.Status)).Render(truncate(title, leftW-14))
		if p.ID == d.meta.ActivePlan {
			line = activeStyle.Render("★ ") + line
		} else {
			line = "  " + line
		}
		badge := dimStyle.Render(" [" + string(p.Status) + "]")
		row := mark + line + badge
		if sel {
			star := ""
			if p.ID == d.meta.ActivePlan {
				star = "★ "
			}
			row = cursorStyle.Render("▸ ") + selRowStyle.Render(truncate(star+title+" ["+string(p.Status)+"]", leftW-4))
		}
		pl.WriteString(row + "\n")
	}
	if rows == 0 {
		pl.WriteString(dimStyle.Render("press 'a' to add a plan"))
	}

	// Tasks panel (for selected plan).
	var tk strings.Builder
	tasks := d.currentTasks()
	tstart, tend := windowRange(len(tasks), d.taskCursor, h-2)
	for i := tstart; i < tend; i++ {
		t := tasks[i]
		sel := i == d.taskCursor && d.focus == focusTasks
		mark := "  "
		if sel {
			mark = cursorStyle.Render("▸ ")
		}
		icon := lipgloss.NewStyle().Foreground(taskStatusColor(t.Status)).Render(taskIcon(t.Status))
		title := truncate(fmt.Sprintf("#%d %s", t.ID, t.Title), rightW-8)
		row := mark + icon + " " + textStyle.Render(title)
		if sel {
			row = selRowStyle.Render(truncate(fmt.Sprintf("%s #%d %s", taskIcon(t.Status), t.ID, t.Title), rightW-2))
		}
		tk.WriteString(row + "\n")
	}
	if d.currentPlan() != nil && len(tasks) == 0 {
		tk.WriteString(dimStyle.Render("press 'a' to add a task"))
	}

	left := panelStyle(leftW, h, d.focus == focusPlans).Render(titleLine("Plans", len(d.plans)) + "\n" + pl.String())
	right := panelStyle(rightW, h, d.focus == focusTasks).Render(titleLine("Tasks", len(tasks)) + "\n" + tk.String())
	return lipgloss.JoinHorizontal(lipgloss.Top, left, " ", right)
}

// --- board ---

func (d dashboard) viewBoard(w, h int) string {
	p := d.currentPlan()
	if p == nil {
		return panelStyle(w-2, h, true).Render(dimStyle.Render("no plan selected — add one in Overview"))
	}
	cols := d.boardColumns()
	colW := (w-8)/len(boardStatuses) - 2
	if colW < 12 {
		colW = 12
	}
	rendered := make([]string, len(boardStatuses))
	for i := range boardStatuses {
		accent := taskStatusColor(boardStatuses[i])
		head := lipgloss.NewStyle().Bold(true).Foreground(accent).
			Render(fmt.Sprintf("%s (%d)", boardTitles[i], len(cols[i])))
		var body strings.Builder
		body.WriteString(head + "\n\n")
		if len(cols[i]) == 0 {
			body.WriteString(dimStyle.Render("—"))
		}
		for row, t := range cols[i] {
			card := truncate(fmt.Sprintf("#%d %s", t.ID, t.Title), colW-4)
			st := lipgloss.NewStyle().Foreground(accent)
			if i == d.boardCol && row == d.boardRow {
				st = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("231")).Background(accent).Padding(0, 1)
			}
			body.WriteString(st.Render(card) + "\n")
		}
		box := lipgloss.NewStyle().MarginRight(1).Render(panelStyle(colW, h, i == d.boardCol).Render(body.String()))
		rendered[i] = box
	}
	title := labelStyle.Render(fmt.Sprintf("Board — #%d %s", p.ID, p.Title))
	return title + "\n" + lipgloss.JoinHorizontal(lipgloss.Top, rendered...)
}

// --- milestones ---

func (d dashboard) viewMilestones(w, h int) string {
	leftW := w/2 - 2
	rightW := w - leftW - 4

	var ml strings.Builder
	start, end := windowRange(len(d.milestones), d.msCursor, h-2)
	for i := start; i < end; i++ {
		m := d.milestones[i]
		sel := i == d.msCursor
		mark := "  "
		if sel {
			mark = cursorStyle.Render("▸ ")
		}
		col := cText
		if m.Status == model.MilestoneDone {
			col = cGreen
		}
		due := ""
		if !m.Due.IsZero() {
			due = dimStyle.Render(" ⏰ " + m.Due.Format("2006-01-02"))
		}
		title := lipgloss.NewStyle().Foreground(col).Render(truncate(fmt.Sprintf("#%d %s", m.ID, m.Title), leftW-16))
		row := mark + title + dimStyle.Render(" ["+string(m.Status)+"]") + due
		if sel {
			row = selRowStyle.Render(truncate(fmt.Sprintf("#%d %s [%s]", m.ID, m.Title, m.Status), leftW-2))
		}
		ml.WriteString(row + "\n")
	}
	if len(d.milestones) == 0 {
		ml.WriteString(dimStyle.Render("press 'a' to add a milestone"))
	}

	// Right: plans of selected milestone.
	var rp strings.Builder
	if m := d.currentMilestone(); m != nil {
		var done, open int
		for _, p := range d.plans {
			if p.MilestoneID != m.ID {
				continue
			}
			rp.WriteString(textStyle.Render(truncate(fmt.Sprintf("#%d %s", p.ID, p.Title), rightW-6)) +
				dimStyle.Render(" ["+string(p.Status)+"]") + "\n")
			for _, t := range d.tasksByPlan[p.ID] {
				if t.Status == model.TaskDone {
					done++
				} else {
					open++
				}
			}
		}
		if rp.Len() == 0 {
			rp.WriteString(dimStyle.Render("no plans — assign with 'ptrack plan add --milestone " + fmt.Sprintf("%d", m.ID) + "'"))
		}
		rp.WriteString("\n" + dimStyle.Render(fmt.Sprintf("tasks: %d done · %d open", done, open)))
	}

	left := panelStyle(leftW, h, true).Render(titleLine("Milestones", len(d.milestones)) + "\n" + ml.String())
	right := panelStyle(rightW, h, false).Render(labelStyle.Render("Plans in milestone") + "\n" + rp.String())
	return lipgloss.JoinHorizontal(lipgloss.Top, left, " ", right)
}

// --- issues ---

func (d dashboard) viewIssues(w, h int) string {
	var il strings.Builder
	start, end := windowRange(len(d.issues), d.issueCursor, h-2)
	for i := start; i < end; i++ {
		is := d.issues[i]
		sel := i == d.issueCursor
		mark := "  "
		if sel {
			mark = cursorStyle.Render("▸ ")
		}
		sev := lipgloss.NewStyle().Foreground(severityColor(is.Severity)).Bold(true).Render(fmt.Sprintf("%-8s", is.Severity))
		st := dimStyle.Render(fmt.Sprintf("%-6s", is.Status))
		if is.Status == model.IssueOpen {
			st = statusStyle.Render(fmt.Sprintf("%-6s", is.Status))
		}
		link := ""
		if is.TaskID != 0 {
			link = dimStyle.Render(fmt.Sprintf(" (task %d)", is.TaskID))
		}
		title := textStyle.Render(truncate(fmt.Sprintf("#%d %s", is.ID, is.Title), w-30))
		row := mark + sev + " " + st + " " + title + link
		if sel {
			row = selRowStyle.Render(truncate(fmt.Sprintf("%-8s %-6s #%d %s", is.Severity, is.Status, is.ID, is.Title), w-6))
		}
		il.WriteString(row + "\n")
	}
	if len(d.issues) == 0 {
		il.WriteString(dimStyle.Render("press 'a' to add an issue"))
	}
	return panelStyle(w-2, h, true).Render(titleLine("Issues", len(d.issues)) + "\n" + il.String())
}

// viewDetail renders the scrollable detail panel for the selected entity.
func (d dashboard) viewDetail(w, h int) string {
	inner := h - 2
	if inner < 1 {
		inner = 1
	}
	start, end := windowRange(len(d.detailLines), d.detailOffset, inner)
	var b strings.Builder
	for i := start; i < end; i++ {
		b.WriteString(d.detailLines[i] + "\n")
	}
	scroll := ""
	if len(d.detailLines) > inner {
		scroll = dimStyle.Render(fmt.Sprintf("  (%d–%d of %d · ↑/↓ scroll)", start+1, end, len(d.detailLines)))
	}
	title := labelStyle.Render(d.detailTitle) + scroll
	return panelStyle(w-2, h, true).Render(title + "\n\n" + b.String())
}

// --- footer / helpers ---

func (d dashboard) footer(w int) string {
	if d.purpose != inputNone {
		return d.input.View() + "\n" + dimStyle.Render("enter confirm · esc cancel")
	}
	if d.showDetail {
		return dimStyle.Render("↑/↓ scroll · pgup/pgdn page · esc/enter close · r refresh · q quit")
	}
	var keys string
	switch d.tab {
	case tabOverview:
		keys = "enter details · ←/→ pane · ↑/↓ move · a add · e rename · u active · x done · s/d/b task · n note"
	case tabBoard:
		keys = "enter details · ←/→ col · ↑/↓ card · H/L move card · a add · e rename · n note"
	case tabMilestones:
		keys = "enter details · ↑/↓ move · a add · e rename · x done · o reopen"
	case tabIssues:
		keys = "enter details · ↑/↓ move · a add · e rename · c close · o reopen"
	}
	global := dimStyle.Render("tab switch · 1-4 jump · g goal · m summary · r reload · B backup · q quit")
	help := dimStyle.Render(keys)
	if d.status != "" {
		return statusStyle.Render(d.status) + "\n" + help + "\n" + global
	}
	return help + "\n" + global
}

func titleLine(name string, n int) string {
	return labelStyle.Render(name) + dimStyle.Render(fmt.Sprintf("  (%d)", n))
}

func orUnset(s string) string {
	if strings.TrimSpace(s) == "" {
		return "(unset)"
	}
	return s
}
