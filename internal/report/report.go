// Package report builds the ptrack restore digest — the compact project summary
// a fresh AI agent reads at the start of a new session.
package report

import (
	"encoding/json"
	"fmt"
	"strings"

	"github.com/ro-ag/ptrack/internal/store"
)

// recentNoteCount bounds how many notes the digest includes.
const recentNoteCount = 10

// digest is the assembled restore view, shared by the Markdown and JSON renderers.
type digest struct {
	Goal       string     `json:"goal"`
	Summary    string     `json:"summary"`
	ActivePlan *planView  `json:"active_plan"`
	Notes      []noteView `json:"recent_notes"`
}

type planView struct {
	ID    uint64     `json:"id"`
	Title string     `json:"title"`
	Tasks []taskView `json:"open_tasks"`
}

type taskView struct {
	ID     uint64 `json:"id"`
	Title  string `json:"title"`
	Status string `json:"status"`
}

type noteView struct {
	ID       uint64 `json:"id"`
	Target   string `json:"target"`
	TargetID uint64 `json:"target_id"`
	Body     string `json:"body"`
}

func build(s *store.Store) (digest, error) {
	m, err := s.GetMeta()
	if err != nil {
		return digest{}, err
	}
	d := digest{Goal: m.Goal, Summary: m.Summary}

	if m.ActivePlan != 0 {
		p, err := s.GetPlan(m.ActivePlan)
		if err == nil {
			pv := &planView{ID: p.ID, Title: p.Title}
			tasks, err := s.ListTasksByPlan(p.ID)
			if err != nil {
				return digest{}, err
			}
			for _, t := range tasks {
				if t.Status.Open() {
					pv.Tasks = append(pv.Tasks, taskView{ID: t.ID, Title: t.Title, Status: string(t.Status)})
				}
			}
			d.ActivePlan = pv
		} else if err != store.ErrNotFound {
			return digest{}, err
		}
	}

	notes, err := s.RecentNotes(recentNoteCount)
	if err != nil {
		return digest{}, err
	}
	for _, n := range notes {
		d.Notes = append(d.Notes, noteView{ID: n.ID, Target: string(n.Target), TargetID: n.TargetID, Body: n.Body})
	}
	return d, nil
}

// Markdown renders the restore digest as compact Markdown for an LLM to read.
func Markdown(s *store.Store) (string, error) {
	d, err := build(s)
	if err != nil {
		return "", err
	}
	var b strings.Builder
	b.WriteString("# ptrack context\n\n")

	b.WriteString("## Goal\n")
	b.WriteString(orDash(d.Goal))
	b.WriteString("\n\n")

	b.WriteString("## Summary\n")
	b.WriteString(orDash(d.Summary))
	b.WriteString("\n\n")

	b.WriteString("## Active plan\n")
	if d.ActivePlan == nil {
		b.WriteString("_none_\n\n")
	} else {
		fmt.Fprintf(&b, "**#%d %s**\n\n", d.ActivePlan.ID, d.ActivePlan.Title)
		b.WriteString("### Open tasks\n")
		if len(d.ActivePlan.Tasks) == 0 {
			b.WriteString("_none_\n")
		} else {
			for _, t := range d.ActivePlan.Tasks {
				fmt.Fprintf(&b, "- [%s] #%d %s\n", t.Status, t.ID, t.Title)
			}
		}
		b.WriteString("\n")
	}

	b.WriteString("## Recent notes\n")
	if len(d.Notes) == 0 {
		b.WriteString("_none_\n")
	} else {
		for _, n := range d.Notes {
			if n.TargetID == 0 {
				fmt.Fprintf(&b, "- (%s) %s\n", n.Target, n.Body)
			} else {
				fmt.Fprintf(&b, "- (%s #%d) %s\n", n.Target, n.TargetID, n.Body)
			}
		}
	}
	return b.String(), nil
}

// JSON renders the restore digest as indented JSON.
func JSON(s *store.Store) ([]byte, error) {
	d, err := build(s)
	if err != nil {
		return nil, err
	}
	return json.MarshalIndent(d, "", "  ")
}

func orDash(s string) string {
	if strings.TrimSpace(s) == "" {
		return "_(unset)_"
	}
	return s
}
