package cli

import (
	"errors"
	"fmt"
	"os"

	"github.com/ro-ag/ptrack/internal/store"
	"github.com/spf13/cobra"
)

// newInitCmd builds the `ptrack init` command: create a project database at the
// git root (or an explicit --root), optionally seed a goal, register it
// globally, and print the created database path. It refuses to create a nested
// project when one already exists up the directory tree unless --force is given.
func newInitCmd() *cobra.Command {
	var (
		goal  string
		root  string
		force bool
	)
	cmd := &cobra.Command{
		Use:   "init",
		Short: "Create a ptrack project in the current repository",
		Long: "Create a .ptrack/ptrack.db database at the git root (or cwd, or an\n" +
			"explicit --root) and optionally seed a north-star goal. Refuses to nest\n" +
			"inside an existing project unless --force is passed.",
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			// Guard against accidentally creating a second project shadowing one
			// that already exists at or above the working directory.
			if !force {
				cwd, err := os.Getwd()
				if err != nil {
					return err
				}
				if existing, err := store.FindProjectDB(cwd); err == nil {
					return fmt.Errorf("already inside ptrack project at %s (use --force to nest a new one)", projectRoot(existing))
				} else if !errors.Is(err, store.ErrNoProject) {
					return err
				}
			}

			dbPath, err := store.InitProject(root)
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
	cmd.Flags().StringVar(&root, "root", "", "explicit project directory (default: git root, else cwd)")
	cmd.Flags().BoolVar(&force, "force", false, "create even if a project already exists above")
	return cmd
}
