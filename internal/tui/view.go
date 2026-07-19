package tui

import (
	"fmt"
	"path/filepath"
	"strings"

	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
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
	if d.showWelcome {
		return d.viewWelcome(w, h)
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
	case d.showMenu:
		body = d.viewMenu(w, bodyH)
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
	case d.tab == tabMaintenance:
		body = d.viewMaintenance(w, bodyH)
	}

	return lipgloss.NewStyle().MaxWidth(w).MaxHeight(h).Render(
		lipgloss.JoinVertical(lipgloss.Left, header, tabs, body, footer),
	)
}

// header composition: two blocks anchored to opposite margins with open
// space between. Left rail (reading position) carries identity and the goal;
// right rail carries the menu hint and the 2×2 stat grid, right edges flush.
//
//	▉P-TRACK · project▉▒░fade                                        ? menu
//	✦ Goal line one …                        milestones ▰▰▱▱▱  1/2  plans …
//	  goal line two …                        tasks      ▰▱▱▱▱  1/5  issues …
//	───────────────────────────── gradient rule ────────────────────────────
func (d dashboard) header(w int) string {
	// Row 1 — brand band: project name rides the accent band, which fades
	// rightward toward the terminal background.
	name := filepath.Base(filepath.Dir(filepath.Dir(d.dbPath)))
	bandText := lipgloss.NewStyle().Bold(true).Foreground(cInk).Background(cAccent)
	pill := bandText.Render(" P-TRACK · " + name + " ")
	right := hint("?", "menu")
	fadeW := min(24, max(0, w-lipgloss.Width(pill)-lipgloss.Width(right)-2))
	row1 := pill + bgFade(fadeW, gradAccent, gradNight)
	if pad := w - lipgloss.Width(row1) - lipgloss.Width(right); pad >= 1 {
		row1 += strings.Repeat(" ", pad) + right
	}

	stats1, stats2, statsW := d.headerStats()
	var row2, row3 string
	if goalW := w - statsW - 4; goalW >= 34 {
		// Wide: goal left, stats right, both flush to their margins.
		g1, g2 := d.goalLines(goalW)
		join := func(left, right string) string {
			pad := max(1, w-lipgloss.Width(left)-lipgloss.Width(right))
			return fitLine(left+strings.Repeat(" ", pad)+right, w)
		}
		row2 = join(g1, stats1)
		row3 = join(g2, stats2)
	} else {
		// Narrow: goal gets the full row, stats compress to one line.
		g1, _ := d.goalLines(w)
		row2 = fitLine(g1, w)
		row3 = fitLine(d.compactStats(), w)
	}

	return lipgloss.JoinVertical(lipgloss.Left,
		fitLine(row1, w),
		row2,
		row3,
		gradientText(strings.Repeat("─", w), gradDarkCyan, gradBlueGreen),
	)
}

// headerStats renders the 2×2 stat grid. All four cells share label, meter,
// and count columns — counts right-aligned — so the block reads as one shape.
// Returns both rows and the block width.
func (d dashboard) headerStats() (string, string, int) {
	c := d.counts
	count := func(col lipgloss.Color, width int, s string, quiet bool) string {
		pad := strings.Repeat(" ", max(0, width-len([]rune(s))))
		if quiet {
			return pad + dimStyle.Render(s)
		}
		return pad + lipgloss.NewStyle().Foreground(col).Bold(true).Render(s)
	}
	// Meter leads each cell as a colored row marker: meter · label · count.
	cell := func(m string, labelW int, label string, cnt string) string {
		return m + " " + dimStyle.Render(fmt.Sprintf("%-*s", labelW, label)) + " " + cnt
	}
	msC := fmt.Sprintf("%d/%d", c.MilestonesDone, c.Milestones)
	tkC := fmt.Sprintf("%d/%d", c.TasksDone, c.Tasks)
	plC := fmt.Sprintf("%d/%d", c.PlansDone, c.Plans)
	isC := fmt.Sprintf("%d", c.IssuesOpen)
	cw1 := max(len(msC), len(tkC))
	cw2 := max(len(plC), len(isC))

	row1 := cell(meter(c.MilestonesDone, c.Milestones, 5, cLavender), 10, "milestones", count(cLavender, cw1, msC, false)) + "   " +
		cell(meter(c.PlansDone, c.Plans, 5, cBlue), 6, "plans", count(cBlue, cw2, plC, false))
	row2 := cell(meter(c.TasksDone, c.Tasks, 5, cGreen), 10, "tasks", count(cGreen, cw1, tkC, false)) + "   " +
		cell(meter(c.IssuesOpen, c.Issues, 5, cRed), 6, "issues", count(cRed, cw2, isC, c.IssuesOpen == 0))
	return row1, row2, lipgloss.Width(row1)
}

// compactStats is the narrow-terminal fallback: counts only, one line.
func (d dashboard) compactStats() string {
	c := d.counts
	n := func(col lipgloss.Color, s string) string {
		return lipgloss.NewStyle().Foreground(col).Bold(true).Render(s)
	}
	issues := dimStyle.Render("0 open")
	if c.IssuesOpen > 0 {
		issues = n(cRed, fmt.Sprintf("%d open", c.IssuesOpen))
	}
	return hintRow(
		dimStyle.Render("milestones ")+n(cLavender, fmt.Sprintf("%d/%d", c.MilestonesDone, c.Milestones)),
		dimStyle.Render("plans ")+n(cBlue, fmt.Sprintf("%d/%d", c.PlansDone, c.Plans)),
		dimStyle.Render("tasks ")+n(cGreen, fmt.Sprintf("%d/%d", c.TasksDone, c.Tasks)),
		dimStyle.Render("issues ")+issues,
	)
}

// goalLines wraps the goal into up to two left-rail lines; the second line
// continues in a quieter gray and ends with an ellipsis when cut.
func (d dashboard) goalLines(width int) (string, string) {
	if width < 8 {
		return "", ""
	}
	textW := width - 2
	if strings.TrimSpace(d.meta.Goal) == "" {
		return labelStyle.Render("✦ ") + dimStyle.Render("no goal — press g"), ""
	}
	lines := strings.Split(ansi.Wrap(d.meta.Goal, textW, ""), "\n")
	first := labelStyle.Render("✦ ") + textStyle.Render(lines[0])
	second := ""
	if len(lines) > 1 {
		rest := strings.Join(lines[1:], " ")
		second = "  " + lipgloss.NewStyle().Foreground(cGray).Render(truncate(rest, textW))
	}
	return first, second
}

func (d dashboard) viewWelcome(w, h int) string {
	menuW := min(58, w-4)
	if menuW < 20 {
		menuW = max(4, w)
	}
	brandW := min(76, max(1, w-2))
	identity := lipgloss.NewStyle().Width(brandW).Align(lipgloss.Center).Render(
		dimStyle.Render("PERSISTENT PROJECT MEMORY  ·  HUMANS + AI AGENTS"),
	)
	action := selectedLine(" ENTER  Open dashboard", menuW)
	shortcuts := lipgloss.NewStyle().Width(menuW).Align(lipgloss.Center).Render(
		hint("1–5", "screens") + "    " + hint("?", "menu") + "    " + hint("q", "quit"),
	)
	content := lipgloss.JoinVertical(lipgloss.Center,
		blockWordmark(w-2),
		"",
		identity,
		"",
		action,
		shortcuts,
	)
	return lipgloss.Place(w, h, lipgloss.Center, lipgloss.Center, content)
}

func blockWordmark(w int) string {
	if w < 76 {
		width := min(58, max(1, w))
		rule := gradientText(strings.Repeat("━", width), gradDarkCyan, gradBlueGreen)
		name := lipgloss.NewStyle().Width(width).Align(lipgloss.Center).Bold(true).Foreground(cAccent).Render("P-TRACK")
		return lipgloss.JoinVertical(lipgloss.Center, rule, name, rule)
	}
	lines := []string{
		` ███████████             ███████████                              █████     `,
		`░░███░░░░░███           ░█░░░███░░░█                             ░░███      `,
		` ░███    ░███           ░   ░███  ░  ████████   ██████    ██████  ░███ █████`,
		` ░██████████  ██████████    ░███    ░░███░░███ ░░░░░███  ███░░███ ░███░░███ `,
		` ░███░░░░░░  ░░░░░░░░░░     ░███     ░███ ░░░   ███████ ░███ ░░░  ░██████░  `,
		` ░███                       ░███     ░███      ███░░███ ░███  ███ ░███░░███ `,
		` █████                      █████    █████    ░░████████░░██████  ████ █████`,
		`░░░░░                      ░░░░░    ░░░░░      ░░░░░░░░  ░░░░░░  ░░░░ ░░░░░ `,
	}
	for i := range lines {
		lines[i] = gradientText(lines[i], gradDarkCyan, gradBlueGreen)
	}
	return lipgloss.JoinVertical(lipgloss.Left, lines...)
}

// tabBar is a single-row segmented control: the active tab is an accent pill,
// the rest stay quiet.
func (d dashboard) tabBar(w int) string {
	if w < 4 {
		return fitLine(tabNames[d.tab], w)
	}
	parts := make([]string, len(tabNames))
	for i, name := range tabNames {
		if tab(i) == d.tab {
			parts[i] = tabActiveStyle.Render(fmt.Sprintf("%d %s", i+1, name))
		} else {
			parts[i] = " " + dimStyle.Render(fmt.Sprintf("%d", i+1)) + " " + hintStyle.Render(name) + " "
		}
	}
	return fitLine(strings.Join(parts, " "), w)
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
		title := fmt.Sprintf("#%d %s", p.ID, p.Title)
		state := ""
		if p.Status != model.PlanActive {
			state = "  " + string(p.Status)
		}
		if sel {
			star := ""
			if p.ID == d.meta.ActivePlan {
				star = "★ "
			}
			pl.WriteString(selectedLine(truncate(star+title+state, leftContentW-2), leftContentW) + "\n")
			continue
		}
		mark := "  "
		if i == d.planCursor {
			mark = dimStyle.Render("▏ ") // cursor parked here while the other pane has focus
		}
		line := lipgloss.NewStyle().Foreground(planStatusColor(p.Status)).Render(truncate(title, leftContentW-6-lipgloss.Width(state)))
		if p.ID == d.meta.ActivePlan {
			line = activeStyle.Render("★ ") + line
		} else {
			line = "  " + line
		}
		pl.WriteString(mark + line + dimStyle.Render(state) + "\n")
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
		if sel {
			tk.WriteString(selectedLine(truncate(fmt.Sprintf("%s #%d %s", taskIcon(t.Status), t.ID, t.Title), rightContentW-2), rightContentW) + "\n")
			continue
		}
		mark := "  "
		if i == d.taskCursor {
			mark = dimStyle.Render("▏ ")
		}
		icon := lipgloss.NewStyle().Foreground(taskStatusColor(t.Status)).Render(taskIcon(t.Status))
		title := truncate(fmt.Sprintf("#%d %s", t.ID, t.Title), rightContentW-4)
		tk.WriteString(mark + icon + " " + textStyle.Render(title) + "\n")
	}
	if d.currentPlan() != nil && len(tasks) == 0 {
		tk.WriteString(dimStyle.Render("press 'a' to add a task"))
	}

	planHints := hintRow(hint("enter", "view"), hint("a", "add"), hint("e", "rename"), hint("u", "activate"), hint("x", "done"))
	taskHints := hintRow(hint("enter", "view"), hint("a", "add"), hint("s/d/b", "status"), hint("n", "note"))
	left := panel("Plans", len(d.plans), leftW, h, d.focus == focusPlans, planHints, pl.String())
	right := panel("Tasks", len(tasks), rightW, h, d.focus == focusTasks, taskHints, tk.String())
	return lipgloss.JoinHorizontal(lipgloss.Top, left, " ", right)
}

// --- board ---

func (d dashboard) viewBoard(w, h int) string {
	p := d.currentPlan()
	if p == nil {
		return panel("Board", 0, w, h, true, "", dimStyle.Render("No plan selected — add one in Overview"))
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
			card := fmt.Sprintf("#%d %s", t.ID, t.Title)
			if i == d.boardCol && row == d.boardRow {
				body.WriteString(selectedLine(truncate(card, contentW-2), contentW) + "\n")
				continue
			}
			st := lipgloss.NewStyle().Foreground(accent)
			body.WriteString("  " + st.Render(truncate(card, contentW-2)) + "\n")
		}
		rendered[i] = panel(taskIcon(boardStatuses[i])+" "+boardTitles[i], len(cols[i]), width, h-1, i == d.boardCol, "", body.String())
	}
	left := labelStyle.Render("Board") + dimStyle.Render(fmt.Sprintf("  /  Plan #%d  ", p.ID)) + textStyle.Render(p.Title)
	hints := hintRow(hint("H/L", "move card"), hint("a", "add"), hint("e", "rename"), hint("n", "note"))
	title := left
	if pad := w - lipgloss.Width(left) - lipgloss.Width(hints); pad >= 2 {
		title = left + strings.Repeat(" ", pad) + hints
	}
	return fitLine(title, w) + "\n" + lipgloss.JoinHorizontal(lipgloss.Top, rendered[0], " ", rendered[1], " ", rendered[2], " ", rendered[3])
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
		if i == d.msCursor {
			ml.WriteString(selectedLine(truncate(fmt.Sprintf("#%d %s  %s", m.ID, m.Title, m.Status), leftContentW-2), leftContentW) + "\n")
			continue
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
		ml.WriteString("  " + title + dimStyle.Render(" ["+string(m.Status)+"]") + due + "\n")
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

	msHints := hintRow(hint("enter", "view"), hint("a", "add"), hint("e", "rename"), hint("x", "done"), hint("o", "reopen"))
	left := panel("Milestones", len(d.milestones), leftW, h, true, msHints, ml.String())
	right := panel("Plans in milestone", -1, rightW, h, false, "", rp.String())
	return lipgloss.JoinHorizontal(lipgloss.Top, left, " ", right)
}

// --- issues ---

func (d dashboard) viewIssues(w, h int) string {
	contentW := panelContentWidth(w)
	var il strings.Builder
	start, end := windowRange(len(d.issues), d.issueCursor, h-2)
	for i := start; i < end; i++ {
		is := d.issues[i]
		if i == d.issueCursor {
			il.WriteString(selectedLine(truncate(fmt.Sprintf("%-8s %-6s #%d %s", is.Severity, is.Status, is.ID, is.Title), contentW-2), contentW) + "\n")
			continue
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
		il.WriteString("  " + sev + " " + st + " " + title + link + "\n")
	}
	if len(d.issues) == 0 {
		il.WriteString(dimStyle.Render("press 'a' to add an issue"))
	}
	issueHints := hintRow(hint("enter", "view"), hint("a", "add"), hint("e", "rename"), hint("c", "close"), hint("o", "reopen"))
	return panel("Issues", len(d.issues), w, h, true, issueHints, il.String())
}

// --- command menu / maintenance ---

func (d dashboard) viewMenu(w, h int) string {
	contentW := panelContentWidth(w)
	var body strings.Builder
	lastGroup := ""
	for i, item := range commandMenu {
		if item.group != lastGroup {
			if lastGroup != "" {
				body.WriteString("\n")
			}
			body.WriteString(groupStyle.Render(strings.ToUpper(item.group)) + "\n")
			lastGroup = item.group
		}
		key := keyStyle.Render(fmt.Sprintf(" %-3s", item.key))
		line := key + textStyle.Render(fmt.Sprintf("%-16s", item.title)) + dimStyle.Render(item.description)
		if i == d.menuCursor {
			plain := fmt.Sprintf("%-3s %-16s%s", item.key, item.title, item.description)
			line = selectedLine(truncate(plain, contentW-2), contentW)
		}
		body.WriteString(line + "\n")
	}
	menuHints := hintRow(hint("↑/↓", "select"), hint("enter", "open"), hint("esc", "close"))
	return panel("Command menu", -1, w, h, true, menuHints, body.String())
}

func (d dashboard) viewMaintenance(w, h int) string {
	leftW := w / 2
	rightW := w - leftW - 1
	root := filepath.Dir(filepath.Dir(d.dbPath))
	home, err := store.GlobalHome()
	if err != nil {
		home = "unavailable: " + err.Error()
	}

	project := strings.Join([]string{
		kv("Project", filepath.Base(root)),
		kv("Goal", orUnset(d.meta.Goal)),
		kv("Summary", orUnset(d.meta.Summary)),
		kv("Root", root),
		kv("Database", d.dbPath),
		kv("Schema", fmt.Sprintf("v%d", d.meta.FormatVersion)),
		kv("Writer", orUnset(d.meta.LastWriteVersion)),
		kv("Updated", d.meta.UpdatedAt.Format("2006-01-02 15:04")),
		"",
		dimStyle.Render("P-TRACK opens the database only for each action,"),
		dimStyle.Render("so agents and this dashboard can work side by side."),
	}, "\n")

	maintenance := strings.Join([]string{
		keyStyle.Render("r") + textStyle.Render("  Reload project state"),
		dimStyle.Render("   Pull in changes written by an agent or CLI."),
		"",
		keyStyle.Render("B") + textStyle.Render("  Create database backup"),
		dimStyle.Render("   Destination: ") + textStyle.Render(filepath.Join(home, "backups")),
		"",
		labelStyle.Render("Agent upkeep"),
		dimStyle.Render("ptrack guide") + textStyle.Render("         refresh agent instructions"),
		dimStyle.Render("ptrack hook install") + textStyle.Render("  record git commits"),
		"",
		keyStyle.Render("?") + textStyle.Render("  Open the command menu from any screen"),
	}, "\n")

	mHints := hintRow(hint("r", "reload"), hint("B", "backup"), hint("g", "goal"), hint("m", "summary"))
	left := panel("Project health", -1, leftW, h, true, mHints, project)
	right := panel("Maintenance actions", -1, rightW, h, false, "", maintenance)
	return lipgloss.JoinHorizontal(lipgloss.Top, left, " ", right)
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
	detailHints := hintRow(hint("↑/↓", "scroll"), hint("pgup/pgdn", "page"), hint("esc", "back"))
	return panel(title, -1, w, h, true, detailHints, b.String())
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
	return borderStyle.Render("╭─") + " " +
		lipgloss.NewStyle().Bold(true).Foreground(cAccentDim).Render(title) + " " +
		borderStyle.Render(tail)
}

func detailSectionBody(line string, width int) string {
	if width < 4 {
		return fitLine(line, width)
	}
	edge := borderStyle.Render("│")
	return edge + " " + fitLine(line, width-4) + " " + edge
}

func detailSectionBottom(width int) string {
	if width < 2 {
		return fitLine("", width)
	}
	return borderStyle.Render("╰" + strings.Repeat("─", width-2) + "╯")
}

// --- footer / helpers ---

// footer is a single global-keys line; per-context actions live in the focused
// panel's bottom border. The status toast docks to the right edge.
func (d dashboard) footer(w int) string {
	if d.purpose != inputNone {
		return fitLine(d.input.View(), w) + "\n" + fitLine(hint("enter", "confirm")+"  "+hint("esc", "cancel"), w)
	}
	keys := []string{hint("?", "menu"), hint("tab", "switch"), hint("1–5", "jump")}
	if !d.showMenu && !d.showDetail {
		keys = append(keys, hint("←/→ ↑/↓", "navigate"))
	}
	keys = append(keys, hint("g", "goal"), hint("m", "summary"), hint("r", "reload"), hint("B", "backup"), hint("q", "quit"))
	global := strings.Join(keys, "  ")
	if d.status == "" {
		return fitLine(global, w)
	}
	toast := statusStyle.Render("● " + d.status)
	if pad := w - lipgloss.Width(global) - lipgloss.Width(toast); pad >= 2 {
		return fitLine(global+strings.Repeat(" ", pad)+toast, w)
	}
	return fitLine(toast+dimStyle.Render("  ·  ")+global, w)
}

func hint(key, action string) string {
	return keyStyle.Render(key) + " " + hintStyle.Render(action)
}

// hintRow joins key/action hints with dim separators, for border embedding.
func hintRow(parts ...string) string {
	return strings.Join(parts, dimStyle.Render(" · "))
}

// panel draws a btop-style frame: the title sits inside ┤ ├ caps embedded in
// the top border, and a focused panel may carry its contextual key hints
// embedded in the bottom border. width and height are outer dimensions,
// preventing borders from overflowing or pushing rows outside the terminal.
func panel(name string, count, width, height int, focused bool, hints, content string) string {
	if width < 4 {
		return fitLine(content, width)
	}
	if height < 2 {
		return fitLine(content, width)
	}

	titleStyle := lipgloss.NewStyle().Bold(true).Foreground(cGray)
	if focused {
		titleStyle = lipgloss.NewStyle().Bold(true).Foreground(cAccent)
	}

	countStr := ""
	if count >= 0 {
		countStr = fmt.Sprintf(" · %d", count)
	}
	useCaps := width >= 12
	overhead := 5 // ╭─ ␣title␣ …╮
	if useCaps {
		overhead = 7 // ╭─┤ ␣title␣ ├…╮
	}
	maxTitleWidth := max(1, width-overhead-1)
	if nameMax := maxTitleWidth - lipgloss.Width(countStr); nameMax >= 1 {
		name = truncate(name, nameMax)
	} else {
		countStr = ""
		name = truncate(name, maxTitleWidth)
	}
	titleWidth := lipgloss.Width(name) + lipgloss.Width(countStr)
	topFill := max(0, width-titleWidth-overhead)
	title := titleStyle.Render(name) + dimStyle.Render(countStr)
	topLead, topTail := "╭─", strings.Repeat("─", topFill)+"╮"
	if useCaps {
		topLead, topTail = "╭─┤", "├"+strings.Repeat("─", topFill)+"╮"
	}

	// The focused frame carries the house gradient; the title stays solid so
	// it reads instantly. Side edges pick up the gradient's endpoint colors,
	// letting the sweep visually continue down the borders.
	top := borderStyle.Render(topLead) + " " + title + " " + borderStyle.Render(topTail)
	leftEdge := borderStyle.Render("│")
	rightEdge := leftEdge
	bottomPlain := "╰" + strings.Repeat("─", width-2) + "╯"
	bottom := borderStyle.Render(bottomPlain)
	if focused {
		top = gradientText(topLead, gradDarkCyan) + " " + title + " " + gradientText(topTail, gradDarkCyan, gradBlueGreen)
		leftEdge = gradientText("│", gradDarkCyan)
		rightEdge = gradientText("│", gradBlueGreen)
		bottom = gradientText(bottomPlain, gradDarkCyan, gradBlueGreen)
		// Contextual keys live in the bottom border, right-aligned btop-style.
		if hw := lipgloss.Width(hints); hints != "" && width-hw-7 >= 0 {
			fill := width - hw - 7
			bottom = gradientText("╰"+strings.Repeat("─", fill), gradDarkCyan, gradBlueGreen) +
				gradientText("┤", gradBlueGreen) + " " + hints + " " +
				gradientText("├─╯", gradBlueGreen)
		}
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
