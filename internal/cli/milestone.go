package cli

import (
	"fmt"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/report"
	"github.com/spf13/cobra"
)

const dueDateLayout = "2006-01-02"

// newMilestoneCmd builds `ptrack milestone` with add, list, show, done, open,
// and due subcommands.
func newMilestoneCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:     "milestone",
		Aliases: []string{"ms"},
		Short:   "Manage milestones (high-level checkpoints grouping plans)",
	}

	var dueStr string
	add := &cobra.Command{
		Use:   "add <title...>",
		Short: "Create a new milestone",
		Args:  cobra.MinimumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			var due time.Time
			if dueStr != "" {
				d, err := time.Parse(dueDateLayout, dueStr)
				if err != nil {
					return fmt.Errorf("invalid --due %q (want YYYY-MM-DD): %w", dueStr, err)
				}
				due = d
			}
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			m, err := s.AddMilestone(joinArgs(args))
			if err != nil {
				return err
			}
			if !due.IsZero() {
				if err := s.SetMilestoneDue(m.ID, due); err != nil {
					return err
				}
			}
			fmt.Fprintf(cmd.OutOrStdout(), "milestone #%d %s\n", m.ID, m.Title)
			return nil
		},
	}
	add.Flags().StringVar(&dueStr, "due", "", "due date (YYYY-MM-DD)")

	var listJSON bool
	list := &cobra.Command{
		Use:   "list",
		Short: "List milestones",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			ms, err := s.ListMilestones()
			if err != nil {
				return err
			}
			if listJSON {
				return emitJSON(cmd, ms)
			}
			out := cmd.OutOrStdout()
			for _, m := range ms {
				due := ""
				if !m.Due.IsZero() {
					due = " (due " + m.Due.Format(dueDateLayout) + ")"
				}
				fmt.Fprintf(out, "#%d [%s] %s%s\n", m.ID, m.Status, m.Title, due)
			}
			return nil
		},
	}
	jsonFlag(list, &listJSON)

	var showJSON bool
	show := &cobra.Command{
		Use:   "show <id>",
		Short: "Show a milestone with its plans and task rollup",
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
			v, err := report.ShowMilestone(s, id)
			if err != nil {
				return err
			}
			return emit(cmd, showJSON, v)
		},
	}
	jsonFlag(show, &showJSON)

	done := &cobra.Command{
		Use:   "done <id>",
		Short: "Mark a milestone done",
		Args:  cobra.ExactArgs(1),
		RunE:  milestoneStatusSetter(model.MilestoneDone),
	}
	open := &cobra.Command{
		Use:   "open <id>",
		Short: "Reopen a milestone",
		Args:  cobra.ExactArgs(1),
		RunE:  milestoneStatusSetter(model.MilestoneOpen),
	}

	due := &cobra.Command{
		Use:   "due <id> <YYYY-MM-DD>",
		Short: "Set a milestone's due date (use '-' to clear)",
		Args:  cobra.ExactArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			id, err := parseID(args[0])
			if err != nil {
				return err
			}
			var d time.Time
			if args[1] != "-" {
				d, err = time.Parse(dueDateLayout, args[1])
				if err != nil {
					return fmt.Errorf("invalid date %q (want YYYY-MM-DD): %w", args[1], err)
				}
			}
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			return s.SetMilestoneDue(id, d)
		},
	}

	cmd.AddCommand(add, list, show, done, open, due)
	return cmd
}

func milestoneStatusSetter(st model.MilestoneStatus) func(*cobra.Command, []string) error {
	return func(cmd *cobra.Command, args []string) error {
		id, err := parseID(args[0])
		if err != nil {
			return err
		}
		s, err := openProject()
		if err != nil {
			return err
		}
		defer s.Close()
		return s.SetMilestoneStatus(id, st)
	}
}
