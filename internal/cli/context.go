package cli

import (
	"fmt"

	"github.com/ro-ag/ptrack/internal/report"
	"github.com/spf13/cobra"
)

// newContextCmd builds `ptrack context`: prints the restore digest as Markdown
// by default, or as indented JSON when --json is set.
func newContextCmd() *cobra.Command {
	var asJSON bool
	cmd := &cobra.Command{
		Use:   "context",
		Short: "Print the restore digest (Markdown by default, --json for JSON)",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			out := cmd.OutOrStdout()
			if asJSON {
				b, err := report.JSON(s)
				if err != nil {
					return err
				}
				fmt.Fprintln(out, string(b))
				return nil
			}
			md, err := report.Markdown(s)
			if err != nil {
				return err
			}
			fmt.Fprint(out, md)
			return nil
		},
	}
	cmd.Flags().BoolVar(&asJSON, "json", false, "emit JSON instead of Markdown")
	return cmd
}
