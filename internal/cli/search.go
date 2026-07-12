package cli

import (
	"github.com/ro-ag/ptrack/internal/report"
	"github.com/spf13/cobra"
)

// newSearchCmd builds `ptrack search <term>`: substring match across plan and
// task titles and note bodies.
func newSearchCmd() *cobra.Command {
	var asJSON bool
	cmd := &cobra.Command{
		Use:   "search <term>",
		Short: "Search plan/task titles and note bodies",
		Args:  cobra.MinimumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			v, err := report.Search(s, joinArgs(args))
			if err != nil {
				return err
			}
			return emit(cmd, asJSON, v)
		},
	}
	jsonFlag(cmd, &asJSON)
	return cmd
}
