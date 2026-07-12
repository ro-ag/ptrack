package cli

import (
	"fmt"

	"github.com/ro-ag/ptrack/internal/store"
	"github.com/spf13/cobra"
)

// newProjectsCmd builds `ptrack projects`: lists every registered project as
// tab-separated name, path, and last-seen timestamp.
func newProjectsCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "projects",
		Short: "List registered projects",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			g, err := store.OpenGlobal()
			if err != nil {
				return err
			}
			defer g.Close()
			refs, err := g.ListProjects()
			if err != nil {
				return err
			}
			out := cmd.OutOrStdout()
			for _, r := range refs {
				fmt.Fprintf(out, "%s\t%s\t%s\n", r.Name, r.Path, r.LastSeen.Format("2006-01-02 15:04:05"))
			}
			return nil
		},
	}
	return cmd
}
