package tui

import (
	"fmt"

	"github.com/charmbracelet/lipgloss"
	"github.com/ro-ag/ptrack/internal/model"
)

// openDetail builds the detail view for the current selection and enters detail
// mode. It is a no-op when nothing is selected.
func (d *dashboard) openDetail() {
	title, lines, ok := d.buildDetail()
	if !ok {
		d.status = "nothing to open"
		return
	}
	d.detailTitle = title
	d.detailLines = lines
	d.detailOffset = 0
	d.showDetail = true
}

// buildDetail assembles the title and rendered lines for the selected entity.
func (d *dashboard) buildDetail() (string, []string, bool) {
	switch d.tab {
	case tabIssues:
		if is := d.currentIssue(); is != nil {
			return fmt.Sprintf("Issue #%d", is.ID), d.issueDetail(*is), true
		}
	case tabMilestones:
		if m := d.currentMilestone(); m != nil {
			return fmt.Sprintf("Milestone #%d", m.ID), d.milestoneDetail(*m), true
		}
	case tabBoard:
		if t := d.boardTask(); t != nil {
			return fmt.Sprintf("Task #%d", t.ID), d.taskDetail(*t), true
		}
	case tabOverview:
		if d.focus == focusTasks {
			if t := d.currentTask(); t != nil {
				return fmt.Sprintf("Task #%d", t.ID), d.taskDetail(*t), true
			}
		}
		if p := d.currentPlan(); p != nil {
			return fmt.Sprintf("Plan #%d", p.ID), d.planDetail(*p), true
		}
	}
	return "", nil, false
}

func kv(k, v string) string {
	return dimStyle.Render(fmt.Sprintf("%-10s", k)) + textStyle.Render(v)
}

func section(name string) string { return labelStyle.Render(name) }

func (d *dashboard) noteLines(notes []model.Note) []string {
	if len(notes) == 0 {
		return []string{dimStyle.Render("  (none)")}
	}
	out := make([]string, 0, len(notes))
	for _, n := range notes {
		stamp := dimStyle.Render(n.CreatedAt.Format("2006-01-02 15:04") + "  ")
		out = append(out, "• "+stamp+textStyle.Render(n.Body))
	}
	return out
}

func (d *dashboard) taskDetail(t model.Task) []string {
	lines := []string{
		lipgloss.NewStyle().Bold(true).Foreground(taskStatusColor(t.Status)).Render(t.Title),
		"",
		kv("Status", taskIcon(t.Status)+" "+string(t.Status)),
	}
	if p, err := d.store.GetPlan(t.PlanID); err == nil {
		lines = append(lines, kv("Plan", fmt.Sprintf("#%d %s", p.ID, p.Title)))
	}
	lines = append(lines,
		kv("Created", t.CreatedAt.Format("2006-01-02 15:04")),
		kv("Updated", t.UpdatedAt.Format("2006-01-02 15:04")),
		"",
		section("Notes"),
	)
	notes, _ := d.store.NotesByTask(t.ID)
	lines = append(lines, d.noteLines(notes)...)
	lines = append(lines, "", section("Commits"))
	commits, _ := d.store.CommitsByTask(t.ID)
	lines = append(lines, d.commitLines(commits)...)
	return lines
}

func (d *dashboard) commitLines(commits []model.Commit) []string {
	if len(commits) == 0 {
		return []string{dimStyle.Render("  (none)")}
	}
	out := make([]string, 0, len(commits))
	for _, c := range commits {
		sha := c.SHA
		if len(sha) > 8 {
			sha = sha[:8]
		}
		out = append(out, "• "+lipgloss.NewStyle().Foreground(cAmber).Render(sha)+"  "+textStyle.Render(c.Subject))
	}
	return out
}

func (d *dashboard) planDetail(p model.Plan) []string {
	lines := []string{
		lipgloss.NewStyle().Bold(true).Foreground(planStatusColor(p.Status)).Render(p.Title),
		"",
		kv("Status", string(p.Status)),
	}
	if p.MilestoneID != 0 {
		if m, err := d.store.GetMilestone(p.MilestoneID); err == nil {
			lines = append(lines, kv("Milestone", fmt.Sprintf("#%d %s", m.ID, m.Title)))
		}
	}
	lines = append(lines, kv("Created", p.CreatedAt.Format("2006-01-02 15:04")), "", section("Tasks"))
	tasks := d.tasksByPlan[p.ID]
	if len(tasks) == 0 {
		lines = append(lines, dimStyle.Render("  (none)"))
	}
	for _, t := range tasks {
		icon := lipgloss.NewStyle().Foreground(taskStatusColor(t.Status)).Render(taskIcon(t.Status))
		lines = append(lines, fmt.Sprintf("  %s #%d %s", icon, t.ID, textStyle.Render(t.Title)))
	}
	lines = append(lines, "", section("Notes"))
	notes, _ := d.store.NotesByPlan(p.ID)
	lines = append(lines, d.noteLines(notes)...)
	lines = append(lines, "", section("Commits"))
	commits, _ := d.store.CommitsByPlan(p.ID)
	lines = append(lines, d.commitLines(commits)...)
	return lines
}

func (d *dashboard) milestoneDetail(m model.Milestone) []string {
	col := cText
	if m.Status == model.MilestoneDone {
		col = cGreen
	}
	lines := []string{
		lipgloss.NewStyle().Bold(true).Foreground(col).Render(m.Title),
		"",
		kv("Status", string(m.Status)),
	}
	if !m.Due.IsZero() {
		lines = append(lines, kv("Due", m.Due.Format("2006-01-02")))
	}
	lines = append(lines, "", section("Plans"))
	var done, open int
	var any bool
	for _, p := range d.plans {
		if p.MilestoneID != m.ID {
			continue
		}
		any = true
		lines = append(lines, fmt.Sprintf("  #%d %s %s", p.ID, textStyle.Render(p.Title), dimStyle.Render("["+string(p.Status)+"]")))
		for _, t := range d.tasksByPlan[p.ID] {
			if t.Status == model.TaskDone {
				done++
			} else {
				open++
			}
		}
	}
	if !any {
		lines = append(lines, dimStyle.Render("  (none)"))
	}
	lines = append(lines, "", dimStyle.Render(fmt.Sprintf("tasks: %d done · %d open", done, open)))
	return lines
}

func (d *dashboard) issueDetail(is model.Issue) []string {
	lines := []string{
		lipgloss.NewStyle().Bold(true).Foreground(severityColor(is.Severity)).Render(is.Title),
		"",
		kv("Status", string(is.Status)),
		kv("Severity", string(is.Severity)),
	}
	if is.TaskID != 0 {
		if t, err := d.store.GetTask(is.TaskID); err == nil {
			lines = append(lines, kv("Task", fmt.Sprintf("#%d %s", t.ID, t.Title)))
		}
	}
	lines = append(lines,
		kv("Created", is.CreatedAt.Format("2006-01-02 15:04")),
		"",
		section("Explanation"),
	)
	if is.Body == "" {
		lines = append(lines, dimStyle.Render("  (none — add with 'ptrack issue add ... --body \"...\"')"))
	} else {
		lines = append(lines, textStyle.Render(is.Body))
	}
	return lines
}
