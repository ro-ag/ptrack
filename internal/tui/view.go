package tui

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
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
	tabs := d.tabBar(w)
	footer := d.footer(w)

	// All regions use outer dimensions, so their sum fits the terminal exactly.
	used := lipgloss.Height(header) + lipgloss.Height(tabs) + lipgloss.Height(footer)
	bodyH := h - used
	if bodyH < 3 {
		bodyH = 3
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

	return lipgloss.NewStyle().MaxWidth(w).MaxHeight(h).Render(
		lipgloss.JoinVertical(lipgloss.Left, header, tabs, body, footer),
	)
}

func (d dashboard) header(w int) string {
	c := d.counts
	badges := strings.Join([]string{
		badge("milestones", c.Milestones, fmt.Sprintf("%d done", c.MilestonesDone), cMagenta),
		badge("plans", c.Plans, fmt.Sprintf("%d done", c.PlansDone), cBlue),
		badge("tasks", c.Tasks, fmt.Sprintf("%d done", c.TasksDone), cGreen),
		badge("issues", c.Issues, fmt.Sprintf("%d open", c.IssuesOpen), cRed),
	}, "  ")

	title := lipgloss.NewStyle().Bold(true).Render(gradientText("ptrack", gradDarkCyan, gradBlueGreen))
	goal := labelStyle.Render("Goal ") + textStyle.Render(truncate(orUnset(d.meta.Goal), w-8))
	summary := dimStyle.Render(truncate("— "+orUnset(d.meta.Summary), w-2))

	top := fitLine(lipgloss.JoinHorizontal(lipgloss.Left, title, "   ", badges), w)
	return lipgloss.JoinVertical(lipgloss.Left,
		top,
		fitLine(goal, w),
		fitLine(summary, w),
		gradientText(strings.Repeat("─", w), gradDarkCyan, gradBlueGreen),
	)
}

func badge(name string, total int, detail string, col lipgloss.Color) string {
	return lipgloss.NewStyle().Foreground(col).Bold(true).Render(fmt.Sprintf("%d", total)) +
		dimStyle.Render(fmt.Sprintf(" %s (%s)", name, detail))
}

func (d dashboard) tabBar(w int) string {
	if w < 4 {
		return fitLine(tabNames[d.tab], w)
	}
	parts := make([]string, len(tabNames))
	for i, name := range tabNames {
		label := fmt.Sprintf("%d %s", i+1, name)
		if tab(i) == d.tab {
			parts[i] = tabActiveStyle.Render(label)
		} else {
			parts[i] = tabInactiveStyle.Render(label)
		}
	}
	divider := lipgloss.NewStyle().Foreground(cBorder).Render("│")
	content := fitLine(lipgloss.JoinHorizontal(lipgloss.Left,
		parts[0], divider, parts[1], divider, parts[2], divider, parts[3],
	), w-2)
	top := gradientText("╭"+strings.Repeat("─", w-2)+"╮", gradDarkCyan, gradBlueGreen)
	middle := lipgloss.NewStyle().Foreground(cCyan).Render("│") + content +
		lipgloss.NewStyle().Foreground(cTeal).Render("│")
	bottom := gradientText("╰"+strings.Repeat("─", w-2)+"╯", gradDarkCyan, gradBlueGreen)
	return lipgloss.JoinVertical(lipgloss.Left, top, middle, bottom)
}

// --- overview ---

func (d dashboard) viewOverview(w, h int) string {
	leftW := w / 2
	rightW := w - leftW - 1
	leftContentW := panelContentWidth(leftW)
	rightContentW := panelContentWidth(rightW)

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
		state := ""
		if p.Status != model.PlanActive {
			state = "  " + string(p.Status)
		}
		line := lipgloss.NewStyle().Foreground(planStatusColor(p.Status)).Render(truncate(title, leftContentW-6-lipgloss.Width(state)))
		if p.ID == d.meta.ActivePlan {
			line = activeStyle.Render("★ ") + line
		} else {
			line = "  " + line
		}
		row := mark + line + dimStyle.Render(state)
		if sel {
			star := ""
			if p.ID == d.meta.ActivePlan {
				star = "★ "
			}
			row = selectedLine("› "+truncate(star+title+state, leftContentW-2), leftContentW)
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
		title := truncate(fmt.Sprintf("#%d %s", t.ID, t.Title), rightContentW-4)
		row := mark + icon + " " + textStyle.Render(title)
		if sel {
			row = selectedLine(truncate(fmt.Sprintf("› %s  #%d %s", taskIcon(t.Status), t.ID, t.Title), rightContentW), rightContentW)
		}
		tk.WriteString(row + "\n")
	}
	if d.currentPlan() != nil && len(tasks) == 0 {
		tk.WriteString(dimStyle.Render("press 'a' to add a task"))
	}

	left := panel("Plans", len(d.plans), leftW, h, d.focus == focusPlans, pl.String())
	right := panel("Tasks", len(tasks), rightW, h, d.focus == focusTasks, tk.String())
	return lipgloss.JoinHorizontal(lipgloss.Top, left, " ", right)
}

// --- board ---

func (d dashboard) viewBoard(w, h int) string {
	p := d.currentPlan()
	if p == nil {
		return panel("Board", 0, w, h, true, dimStyle.Render("No plan selected — add one in Overview"))
	}
	cols := d.boardColumns()
	gapW := len(boardStatuses) - 1
	available := max(0, w-gapW)
	colW := available / len(boardStatuses)
	remainder := available % len(boardStatuses)
	rendered := make([]string, len(boardStatuses))
	for i := range boardStatuses {
		width := colW
		if i < remainder {
			width++
		}
		contentW := panelContentWidth(width)
		accent := taskStatusColor(boardStatuses[i])
		var body strings.Builder
		if len(cols[i]) == 0 {
			body.WriteString(dimStyle.Render("—"))
		}
		for row, t := range cols[i] {
			card := truncate(fmt.Sprintf("#%d %s", t.ID, t.Title), contentW)
			st := lipgloss.NewStyle().Foreground(accent)
			if i == d.boardCol && row == d.boardRow {
				body.WriteString(selectedLine("› "+truncate(card, contentW-2), contentW) + "\n")
				continue
			}
			body.WriteString(st.Render(card) + "\n")
		}
		rendered[i] = panel(boardTitles[i], len(cols[i]), width, h-1, i == d.boardCol, body.String())
	}
	title := fitLine(labelStyle.Render("Board")+dimStyle.Render(fmt.Sprintf("  /  Plan #%d  ", p.ID))+textStyle.Render(p.Title), w)
	return title + "\n" + lipgloss.JoinHorizontal(lipgloss.Top, rendered[0], " ", rendered[1], " ", rendered[2], " ", rendered[3])
}

// --- milestones ---

func (d dashboard) viewMilestones(w, h int) string {
	leftW := w / 2
	rightW := w - leftW - 1
	leftContentW := panelContentWidth(leftW)
	rightContentW := panelContentWidth(rightW)

	var ml strings.Builder
	start, end := windowRange(len(d.milestones), d.msCursor, h-2)
	for i := start; i < end; i++ {
		m := d.milestones[i]
		sel := i == d.msCursor
		mark := "  "
		if sel {
			mark = cursorStyle.Render("▸ ")
		}
		col := cLavender
		if m.Status == model.MilestoneDone {
			col = cGreen
		}
		due := ""
		if !m.Due.IsZero() {
			due = dimStyle.Render(" ⏰ " + m.Due.Format("2006-01-02"))
		}
		title := lipgloss.NewStyle().Foreground(col).Render(truncate(fmt.Sprintf("#%d %s", m.ID, m.Title), leftContentW-14))
		row := mark + title + dimStyle.Render(" ["+string(m.Status)+"]") + due
		if sel {
			row = selectedLine(truncate(fmt.Sprintf("› #%d %s  %s", m.ID, m.Title, m.Status), leftContentW), leftContentW)
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
			rp.WriteString(textStyle.Render(truncate(fmt.Sprintf("#%d %s", p.ID, p.Title), rightContentW-10)) +
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

	left := panel("Milestones", len(d.milestones), leftW, h, true, ml.String())
	right := panel("Plans in milestone", -1, rightW, h, false, rp.String())
	return lipgloss.JoinHorizontal(lipgloss.Top, left, " ", right)
}

// --- issues ---

func (d dashboard) viewIssues(w, h int) string {
	contentW := panelContentWidth(w)
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
		title := textStyle.Render(truncate(fmt.Sprintf("#%d %s", is.ID, is.Title), contentW-24))
		row := mark + sev + " " + st + " " + title + link
		if sel {
			row = selectedLine(truncate(fmt.Sprintf("› %-8s %-6s #%d %s", is.Severity, is.Status, is.ID, is.Title), contentW), contentW)
		}
		il.WriteString(row + "\n")
	}
	if len(d.issues) == 0 {
		il.WriteString(dimStyle.Render("press 'a' to add an issue"))
	}
	return panel("Issues", len(d.issues), w, h, true, il.String())
}

// viewDetail renders the scrollable detail panel for the selected entity.
func (d dashboard) viewDetail(w, h int) string {
	inner := h - 2
	if inner < 1 {
		inner = 1
	}
	lines := d.wrappedDetailLines(w)
	start, end := windowRange(len(lines), d.detailOffset, inner)
	var b strings.Builder
	for i := start; i < end; i++ {
		b.WriteString(lines[i] + "\n")
	}
	title := d.detailTitle
	if len(lines) > inner {
		title += fmt.Sprintf("  %d–%d/%d", start+1, end, len(lines))
	}
	return panel(title, -1, w, h, true, b.String())
}

// wrappedDetailLines expands logical detail rows into display rows for the
// current panel width. ANSI-aware wrapping keeps styled notes and explanations
// readable instead of silently clipping their tails at the right border.
func (d dashboard) wrappedDetailLines(w int) []string {
	if w <= 0 {
		w = 100
	}
	width := panelContentWidth(w)
	lines := make([]string, 0, len(d.detailLines)+4)
	inSection := false
	for i, line := range d.detailLines {
		if name, ok := detailSectionName(line); ok {
			if inSection {
				lines = append(lines, detailSectionBottom(width))
			}
			lines = append(lines, detailSectionTop(name, width))
			inSection = true
			continue
		}

		// A logical spacer immediately before the next section belongs between
		// panels, not inside the preceding panel.
		if inSection && line == "" && i+1 < len(d.detailLines) {
			if _, nextIsSection := detailSectionName(d.detailLines[i+1]); nextIsSection {
				continue
			}
		}

		lineWidth := width
		if inSection {
			lineWidth = max(1, width-4)
		}
		wrapped := strings.Split(ansi.Wrap(line, lineWidth, ""), "\n")
		for _, displayLine := range wrapped {
			if inSection {
				lines = append(lines, detailSectionBody(displayLine, width))
			} else {
				lines = append(lines, displayLine)
			}
		}
	}
	if inSection {
		lines = append(lines, detailSectionBottom(width))
	}
	return lines
}

func detailSectionName(line string) (string, bool) {
	if !strings.HasPrefix(line, detailSectionPrefix) {
		return "", false
	}
	return strings.TrimPrefix(line, detailSectionPrefix), true
}

func detailSectionTop(name string, width int) string {
	if width < 6 {
		return fitLine(name, width)
	}
	title := truncate(name, width-5)
	tail := strings.Repeat("─", max(0, width-lipgloss.Width(title)-5)) + "╮"
	return gradientText("╭─", gradMagenta, gradIndigo) + " " +
		lipgloss.NewStyle().Bold(true).Render(gradientText(title, gradMagenta, gradIndigo)) + " " +
		gradientText(tail, gradMagenta, gradIndigo)
}

func detailSectionBody(line string, width int) string {
	if width < 4 {
		return fitLine(line, width)
	}
	return lipgloss.NewStyle().Foreground(cMagenta).Render("│") + " " +
		fitLine(line, width-4) + " " +
		lipgloss.NewStyle().Foreground(cIndigo).Render("│")
}

func detailSectionBottom(width int) string {
	if width < 2 {
		return fitLine("", width)
	}
	return gradientText("╰"+strings.Repeat("─", width-2)+"╯", gradMagenta, gradIndigo)
}

// --- footer / helpers ---

func (d dashboard) footer(w int) string {
	if d.purpose != inputNone {
		return fitLine(d.input.View(), w) + "\n" + fitLine(hint("enter", "confirm")+"  "+hint("esc", "cancel"), w)
	}
	if d.showDetail {
		return fitLine(strings.Join([]string{
			hint("enter/esc", "back"), hint("↑/↓", "scroll"), hint("pgup/pgdn", "page"),
		}, "  "), w) + "\n" + fitLine(strings.Join([]string{hint("r", "refresh"), hint("q", "quit")}, "  "), w)
	}
	var actions []string
	switch d.tab {
	case tabOverview:
		actions = []string{hint("enter", "view"), hint("←/→", "pane"), hint("↑/↓", "select"), hint("a", "add"), hint("e", "rename"), hint("u", "activate"), hint("x", "complete"), hint("s/d/b", "task status"), hint("n", "note")}
	case tabBoard:
		actions = []string{hint("enter", "view"), hint("←/→", "column"), hint("↑/↓", "select"), hint("H/L", "move card"), hint("a", "add"), hint("e", "rename"), hint("n", "note")}
	case tabMilestones:
		actions = []string{hint("enter", "view"), hint("↑/↓", "select"), hint("a", "add"), hint("e", "rename"), hint("x", "complete"), hint("o", "reopen")}
	case tabIssues:
		actions = []string{hint("enter", "view"), hint("↑/↓", "select"), hint("a", "add"), hint("e", "rename"), hint("c", "close"), hint("o", "reopen")}
	}
	global := strings.Join([]string{hint("tab", "switch"), hint("1–4", "jump"), hint("g", "goal"), hint("m", "summary"), hint("r", "reload"), hint("B", "backup"), hint("q", "quit")}, "  ")
	secondary := global
	if d.status != "" {
		secondary = statusStyle.Render("● "+d.status) + dimStyle.Render("  ·  ") + global
	}
	return fitLine(strings.Join(actions, "  "), w) + "\n" + fitLine(secondary, w)
}

func hint(key, action string) string {
	return keyStyle.Render(key) + " " + hintStyle.Render(action)
}

// panel draws a btop-style frame whose title is embedded into the top border.
// width and height are outer dimensions, preventing borders from overflowing
// or pushing the first/last rows outside the terminal.
func panel(name string, count, width, height int, focused bool, content string) string {
	if width < 4 {
		return fitLine(content, width)
	}
	if height < 2 {
		return fitLine(content, width)
	}

	borderColor := cBorder
	titleColor := cCyan
	if focused {
		borderColor = cTeal
		titleColor = cTeal
	}
	borderStyle := lipgloss.NewStyle().Foreground(borderColor)
	titleStyle := lipgloss.NewStyle().Bold(true).Foreground(titleColor)

	title := name
	if count >= 0 {
		title += fmt.Sprintf(" %d", count)
	}
	maxTitleWidth := max(1, width-6)
	if lipgloss.Width(title) > maxTitleWidth {
		title = truncate(title, maxTitleWidth)
	}
	titleWidth := lipgloss.Width(title)
	topFill := max(0, width-titleWidth-5)
	topTail := strings.Repeat("─", topFill) + "╮"
	top := borderStyle.Render("╭─") + " " + titleStyle.Render(title) + " " + borderStyle.Render(topTail)
	leftEdge := borderStyle.Render("│")
	rightEdge := leftEdge
	bottom := borderStyle.Render("╰" + strings.Repeat("─", width-2) + "╯")
	if focused {
		top = gradientText("╭─", gradDarkCyan, gradBlueGreen) + " " +
			lipgloss.NewStyle().Bold(true).Render(gradientText(title, gradDarkCyan, gradBlueGreen)) + " " +
			gradientText(topTail, gradDarkCyan, gradBlueGreen)
		leftEdge = lipgloss.NewStyle().Foreground(cCyan).Render("│")
		rightEdge = lipgloss.NewStyle().Foreground(cTeal).Render("│")
		bottom = gradientText("╰"+strings.Repeat("─", width-2)+"╯", gradDarkCyan, gradBlueGreen)
	}

	innerWidth := panelContentWidth(width)
	bodyRows := height - 2
	trimmed := strings.TrimSuffix(content, "\n")
	lines := []string{}
	if trimmed != "" {
		lines = strings.Split(trimmed, "\n")
	}
	if len(lines) > bodyRows {
		lines = lines[:bodyRows]
	}

	var out strings.Builder
	out.WriteString(top)
	for i := 0; i < bodyRows; i++ {
		line := ""
		if i < len(lines) {
			line = lines[i]
		}
		out.WriteByte('\n')
		out.WriteString(leftEdge)
		out.WriteByte(' ')
		out.WriteString(fitLine(line, innerWidth))
		out.WriteByte(' ')
		out.WriteString(rightEdge)
	}
	out.WriteByte('\n')
	out.WriteString(bottom)
	return out.String()
}

func orUnset(s string) string {
	if strings.TrimSpace(s) == "" {
		return "(unset)"
	}
	return s
}
