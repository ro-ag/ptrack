package cli

import (
	"fmt"
	"strings"

	"github.com/spf13/cobra"
)

// newGoalCmd builds `ptrack goal` with `show` (default) and `set <text...>`
// subcommands. `show` prints the current goal; `set` joins args and stores it.
func newGoalCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "goal",
		Short: "Show or set the project's north-star goal",
	}

	show := &cobra.Command{
		Use:   "show",
		Short: "Print the current goal",
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
			fmt.Fprintln(cmd.OutOrStdout(), m.Goal)
			return nil
		},
	}

	set := &cobra.Command{
		Use:   "set <text...>",
		Short: "Set the goal text (args joined with spaces)",
		Args:  cobra.MinimumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			return s.SetGoal(strings.Join(args, " "))
		},
	}

	cmd.AddCommand(show, set)
	// `ptrack goal` with no subcommand defaults to show.
	cmd.RunE = func(cmd *cobra.Command, args []string) error {
		return show.RunE(cmd, args)
	}
	return cmd
}
