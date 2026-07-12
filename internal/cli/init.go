package cli

import (
	"fmt"

	"github.com/ro-ag/ptrack/internal/store"
	"github.com/spf13/cobra"
)

// newInitCmd builds the `ptrack init` command: create a project database in the
// current repository, optionally seed a goal, register it globally, and print
// the created database path.
func newInitCmd() *cobra.Command {
	var goal string
	cmd := &cobra.Command{
		Use:   "init",
		Short: "Create a ptrack project in the current repository",
		Long:  "Create a .ptrack/ptrack.db database at the git root (or cwd) and optionally seed a north-star goal.",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			dbPath, err := store.InitProject("")
			if err != nil {
				return err
			}
			s, err := store.Open(dbPath)
			if err != nil {
				return err
			}
			defer s.Close()
			if goal != "" {
				if err := s.SetGoal(goal); err != nil {
					return err
				}
			}
			registerProjectBestEffort(projectRoot(dbPath))
			fmt.Fprintln(cmd.OutOrStdout(), dbPath)
			return nil
		},
	}
	cmd.Flags().StringVar(&goal, "goal", "", "initial north-star goal text")
	return cmd
}
