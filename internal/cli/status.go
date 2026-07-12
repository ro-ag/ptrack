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

			out := cmd.OutOrStdout()
			goal := firstLine(m.Goal)
			if goal == "" {
				goal = "(no goal set)"
			}
			fmt.Fprintf(out, "goal: %s\n", goal)

			active := "(no active plan)"
			if m.ActivePlan != 0 {
				if p, err := s.GetPlan(m.ActivePlan); err == nil {
					active = p.Title
				}
			}
			fmt.Fprintf(out, "active plan: %s\n", active)

			counts := map[model.TaskStatus]int{}
			for _, t := range tasks {
				counts[t.Status]++
			}
			fmt.Fprintf(out, "tasks: %d todo, %d doing, %d done, %d blocked\n",
				counts[model.TaskTodo], counts[model.TaskDoing],
				counts[model.TaskDone], counts[model.TaskBlocked])
			fmt.Fprintf(out, "plans: %d\n", len(plans))
			return nil
		},
	}
	return cmd
}

// firstLine returns the first line of s, trimmed of surrounding whitespace.
func firstLine(s string) string {
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		s = s[:i]
	}
	return strings.TrimSpace(s)
}
