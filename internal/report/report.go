// Package report builds ptrack's read views — the restore digest a fresh AI
// agent reads at session start, plus the drill-down views (next, plan/task show,
// search, board). Every view renders to token-efficient Markdown by default and
// exposes the same data as JSON for programmatic consumers. Payloads stay
// bounded: the context digest shows the live edge plus counts and pointers, not
// full dumps.
package report

import (
	"fmt"
	"strings"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

// Bounds keep the context digest roughly constant in size regardless of project
// scale.
const (
	contextRecentNotes  = 5
	contextBlockedShown = 8
	contextIssuesShown  = 8
)

// Digest is the bounded cold-start restore view.
type Digest struct {
	Goal        string       `json:"goal"`
	Summary     string       `json:"summary"`
	ActivePlan  *PlanBrief   `json:"active_plan"`
	Blocked     []TaskLine   `json:"blocked"`
	BlockedMore int          `json:"blocked_more"`
	OpenIssues  []IssueLine  `json:"open_issues"`
	IssuesMore  int          `json:"open_issues_more"`
	Notes       []NoteLine   `json:"recent_notes"`
	Inventory   model.Counts `json:"inventory"`
}

// IssueLine is a compact issue reference.
type IssueLine struct {
	ID       uint64 `json:"id"`
	Title    string `json:"title"`
	Severity string `json:"severity"`
	Status   string `json:"status"`
	TaskID   uint64 `json:"task_id"`
}

// PlanBrief is a plan plus its open tasks, for the digest.
type PlanBrief struct {
	ID    uint64     `json:"id"`
	Title string     `json:"title"`
	Tasks []TaskLine `json:"open_tasks"`
}

// TaskLine is a compact task reference.
type TaskLine struct {
	ID     uint64 `json:"id"`
	PlanID uint64 `json:"plan_id"`
	Title  string `json:"title"`
	Status string `json:"status"`
}

// NoteLine is a compact note reference.
type NoteLine struct {
	ID       uint64 `json:"id"`
	Target   string `json:"target"`
	TargetID uint64 `json:"target_id"`
	Body     string `json:"body"`
}

// Context assembles the bounded restore digest.
func Context(s *store.Store) (Digest, error) {
	m, err := s.GetMeta()
	if err != nil {
		return Digest{}, err
	}
	d := Digest{Goal: m.Goal, Summary: m.Summary}

	if m.ActivePlan != 0 {
		if p, err := s.GetPlan(m.ActivePlan); err == nil {
			pb := &PlanBrief{ID: p.ID, Title: p.Title}
			tasks, err := s.ListTasksByPlan(p.ID)
			if err != nil {
				return Digest{}, err
			}
			for _, t := range tasks {
				if t.Status.Open() {
					pb.Tasks = append(pb.Tasks, taskLine(t))
				}
			}
			d.ActivePlan = pb
		} else if err != store.ErrNotFound {
			return Digest{}, err
		}
	}

	allTasks, err := s.ListTasks()
	if err != nil {
		return Digest{}, err
	}
	for _, t := range allTasks {
		if t.Status == model.TaskBlocked {
			if len(d.Blocked) < contextBlockedShown {
				d.Blocked = append(d.Blocked, taskLine(t))
			} else {
				d.BlockedMore++
			}
		}
	}

	issues, err := s.ListIssues()
	if err != nil {
		return Digest{}, err
	}
	for _, is := range issues {
		if is.Status != model.IssueOpen {
			continue
		}
		if len(d.OpenIssues) < contextIssuesShown {
			d.OpenIssues = append(d.OpenIssues, issueLine(is))
		} else {
			d.IssuesMore++
		}
	}

	notes, err := s.RecentNotes(contextRecentNotes)
	if err != nil {
		return Digest{}, err
	}
	for _, n := range notes {
		d.Notes = append(d.Notes, noteLine(n))
	}

	if d.Inventory, err = s.Counts(); err != nil {
		return Digest{}, err
	}
	return d, nil
}

// Markdown renders the digest as compact Markdown.
func (d Digest) Markdown() string {
	var b strings.Builder
	b.WriteString("# ptrack context\n\n")

	b.WriteString("## Goal\n" + orDash(d.Goal) + "\n\n")
	b.WriteString("## Summary\n" + orDash(d.Summary) + "\n\n")

	b.WriteString("## Active plan\n")
	if d.ActivePlan == nil {
		b.WriteString("_none_\n\n")
	} else {
		fmt.Fprintf(&b, "**#%d %s**\n\n### Open tasks\n", d.ActivePlan.ID, d.ActivePlan.Title)
		if len(d.ActivePlan.Tasks) == 0 {
			b.WriteString("_none_\n")
		} else {
			for _, t := range d.ActivePlan.Tasks {
				fmt.Fprintf(&b, "- [%s] #%d %s\n", t.Status, t.ID, t.Title)
			}
		}
		b.WriteString("\n")
	}

	if len(d.Blocked) > 0 {
		b.WriteString("## Blocked (project-wide)\n")
		for _, t := range d.Blocked {
			fmt.Fprintf(&b, "- #%d %s (plan %d)\n", t.ID, t.Title, t.PlanID)
		}
		if d.BlockedMore > 0 {
			fmt.Fprintf(&b, "- … +%d more (use `ptrack task list --status blocked`)\n", d.BlockedMore)
		}
		b.WriteString("\n")
	}

	if len(d.OpenIssues) > 0 {
		b.WriteString("## Open issues\n")
		for _, is := range d.OpenIssues {
			if is.TaskID == 0 {
				fmt.Fprintf(&b, "- #%d [%s] %s\n", is.ID, is.Severity, is.Title)
			} else {
				fmt.Fprintf(&b, "- #%d [%s] %s (task %d)\n", is.ID, is.Severity, is.Title, is.TaskID)
			}
		}
		if d.IssuesMore > 0 {
			fmt.Fprintf(&b, "- … +%d more (use `ptrack issue list`)\n", d.IssuesMore)
		}
		b.WriteString("\n")
	}

	b.WriteString("## Recent decisions\n")
	if len(d.Notes) == 0 {
		b.WriteString("_none_\n")
	} else {
		for _, n := range d.Notes {
			b.WriteString("- " + noteMarkdown(n) + "\n")
		}
	}
	b.WriteString("\n")

	b.WriteString("## Inventory\n")
	c := d.Inventory
	fmt.Fprintf(&b, "%d milestones (%d done) · %d plans (%d done) · %d tasks (%d done · %d blocked · %d open) · %d issues (%d open) · %d notes\n\n",
		c.Milestones, c.MilestonesDone, c.Plans, c.PlansDone,
		c.Tasks, c.TasksDone, c.TasksBlocked, c.TasksOpen, c.Issues, c.IssuesOpen, c.Notes)
	b.WriteString("Drill deeper: `ptrack next` · `ptrack milestone list` · `ptrack plan show <id>` · " +
		"`ptrack task show <id>` · `ptrack task list --status doing,blocked` · `ptrack issue list` · " +
		"`ptrack note list` · `ptrack search <term>` · `ptrack board`\n")
	return b.String()
}

func issueLine(is model.Issue) IssueLine {
	return IssueLine{ID: is.ID, Title: is.Title, Severity: string(is.Severity), Status: string(is.Status), TaskID: is.TaskID}
}

func taskLine(t model.Task) TaskLine {
	return TaskLine{ID: t.ID, PlanID: t.PlanID, Title: t.Title, Status: string(t.Status)}
}

func noteLine(n model.Note) NoteLine {
	return NoteLine{ID: n.ID, Target: string(n.Target), TargetID: n.TargetID, Body: n.Body}
}

func noteMarkdown(n NoteLine) string {
	if n.TargetID == 0 {
		return fmt.Sprintf("(%s) %s", n.Target, n.Body)
	}
	return fmt.Sprintf("(%s #%d) %s", n.Target, n.TargetID, n.Body)
}

func orDash(s string) string {
	if strings.TrimSpace(s) == "" {
		return "_(unset)_"
	}
	return s
}
