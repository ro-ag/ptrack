package cli

import (
	"fmt"
	"strings"

	"github.com/spf13/cobra"
)

// newSummaryCmd builds `ptrack summary` with `show` (default) and
// `set <text...>` subcommands, analogous to `ptrack goal`.
func newSummaryCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "summary",
		Short: "Show or set the rolling context summary",
	}

	show := &cobra.Command{
		Use:   "show",
		Short: "Print the current summary",
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
			fmt.Fprintln(cmd.OutOrStdout(), m.Summary)
			return nil
		},
	}

	set := &cobra.Command{
		Use:   "set <text...>",
		Short: "Set the summary text (args joined with spaces)",
		Args:  cobra.MinimumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			return s.SetSummary(strings.Join(args, " "))
		},
	}

	cmd.AddCommand(show, set)
	cmd.RunE = func(cmd *cobra.Command, args []string) error {
		return show.RunE(cmd, args)
	}
	return cmd
}
