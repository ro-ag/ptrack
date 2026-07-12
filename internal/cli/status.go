package cli

import (
	"fmt"
	"strings"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/spf13/cobra"
)

// newStatusCmd builds `ptrack status`: a short plain-text project overview —
// the first line of the goal, the active plan title, task counts by status,
// and the plan count.
func newStatusCmd() *cobra.Command {
	var asJSON bool
	cmd := &cobra.Command{
		Use:   "status",
		Short: "Print a short project overview",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			m, err := s.GetMeta()
			if err != nil {
				return err
			}
			plans, err := s.ListPlans()
			if err != nil {
				return err
			}
			tasks, err := s.ListTasks()
			if err != nil {
				return err
			}

			activeTitle := ""
			if m.ActivePlan != 0 {
				if p, err := s.GetPlan(m.ActivePlan); err == nil {
					activeTitle = p.Title
				}
			}
			counts := map[model.TaskStatus]int{}
			for _, t := range tasks {
				counts[t.Status]++
			}

			if asJSON {
				return emitJSON(cmd, struct {
					Goal        string `json:"goal"`
					ActivePlan  uint64 `json:"active_plan"`
					ActiveTitle string `json:"active_plan_title"`
					Plans       int    `json:"plans"`
					Todo        int    `json:"todo"`
					Doing       int    `json:"doing"`
					Done        int    `json:"done"`
					Blocked     int    `json:"blocked"`
				}{
					Goal: m.Goal, ActivePlan: m.ActivePlan, ActiveTitle: activeTitle, Plans: len(plans),
					Todo: counts[model.TaskTodo], Doing: counts[model.TaskDoing],
					Done: counts[model.TaskDone], Blocked: counts[model.TaskBlocked],
				})
			}

			out := cmd.OutOrStdout()
			goal := firstLine(m.Goal)
			if goal == "" {
				goal = "(no goal set)"
			}
			fmt.Fprintf(out, "goal: %s\n", goal)

			active := activeTitle
			if active == "" {
				active = "(no active plan)"
			}
			fmt.Fprintf(out, "active plan: %s\n", active)

			fmt.Fprintf(out, "tasks: %d todo, %d doing, %d done, %d blocked\n",
				counts[model.TaskTodo], counts[model.TaskDoing],
				counts[model.TaskDone], counts[model.TaskBlocked])
			fmt.Fprintf(out, "plans: %d\n", len(plans))
			return nil
		},
	}
	jsonFlag(cmd, &asJSON)
	return cmd
}

// firstLine returns the first line of s, trimmed of surrounding whitespace.
func firstLine(s string) string {
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		s = s[:i]
	}
	return strings.TrimSpace(s)
}
