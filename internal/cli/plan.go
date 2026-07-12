package cli

import (
	"fmt"
	"strconv"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/report"
	"github.com/spf13/cobra"
)

// newPlanCmd builds `ptrack plan` with add, list, done, and use subcommands.
func newPlanCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "plan",
		Short: "Manage plans",
	}

	add := &cobra.Command{
		Use:   "add <title...>",
		Short: "Create a new active plan",
		Args:  cobra.MinimumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			p, err := s.AddPlan(joinArgs(args))
			if err != nil {
				return err
			}
			fmt.Fprintf(cmd.OutOrStdout(), "plan #%d %s\n", p.ID, p.Title)
			return nil
		},
	}

	var listJSON bool
	list := &cobra.Command{
		Use:   "list",
		Short: "List plans",
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
			if listJSON {
				type planRow struct {
					ID     uint64 `json:"id"`
					Title  string `json:"title"`
					Status string `json:"status"`
					Active bool   `json:"active"`
				}
				rows := make([]planRow, 0, len(plans))
				for _, p := range plans {
					rows = append(rows, planRow{p.ID, p.Title, string(p.Status), p.ID == m.ActivePlan})
				}
				return emitJSON(cmd, rows)
			}
			out := cmd.OutOrStdout()
			for _, p := range plans {
				mark := ' '
				if p.ID == m.ActivePlan {
					mark = '*'
				}
				fmt.Fprintf(out, "#%d [%s] %c %s\n", p.ID, p.Status, mark, p.Title)
			}
			return nil
		},
	}
	jsonFlag(list, &listJSON)

	var showJSON bool
	show := &cobra.Command{
		Use:   "show <id>",
		Short: "Show a plan with its tasks and notes",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			id, err := parseID(args[0])
			if err != nil {
				return err
			}
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			v, err := report.ShowPlan(s, id)
			if err != nil {
				return err
			}
			return emit(cmd, showJSON, v)
		},
	}
	jsonFlag(show, &showJSON)

	done := &cobra.Command{
		Use:   "done <id>",
		Short: "Mark a plan done",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			id, err := parseID(args[0])
			if err != nil {
				return err
			}
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			return s.SetPlanStatus(id, model.PlanDone)
		},
	}

	use := &cobra.Command{
		Use:   "use <id>",
		Short: "Set the active plan",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			id, err := parseID(args[0])
			if err != nil {
				return err
			}
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			return s.SetActivePlan(id)
		},
	}

	cmd.AddCommand(add, list, show, done, use)
	return cmd
}

// joinArgs joins positional args with spaces to form a single text field.
func joinArgs(args []string) string {
	out := ""
	for i, a := range args {
		if i > 0 {
			out += " "
		}
		out += a
	}
	return out
}

// parseID parses a base-10 unsigned 64-bit id from a command argument.
func parseID(s string) (uint64, error) {
	id, err := strconv.ParseUint(s, 10, 64)
	if err != nil {
		return 0, fmt.Errorf("invalid id %q: %w", s, err)
	}
	return id, nil
}
