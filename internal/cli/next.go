package cli

import (
	"github.com/ro-ag/ptrack/internal/report"
	"github.com/spf13/cobra"
)

// newNextCmd builds `ptrack next`: the single most-actionable task.
func newNextCmd() *cobra.Command {
	var asJSON bool
	cmd := &cobra.Command{
		Use:   "next",
		Short: "Print the single most-actionable task (active plan: doing, else todo)",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			v, err := report.Next(s)
			if err != nil {
				return err
			}
			return emit(cmd, asJSON, v)
		},
	}
	jsonFlag(cmd, &asJSON)
	return cmd
}
