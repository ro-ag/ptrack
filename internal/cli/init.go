package cli

import (
	"errors"
	"fmt"
	"os"

	"github.com/ro-ag/ptrack/internal/guide"
	"github.com/ro-ag/ptrack/internal/store"
	"github.com/spf13/cobra"
)

// newInitCmd builds the `ptrack init` command: create a project database at the
// git root (or an explicit --root), optionally seed a goal, register it
// globally, and print the created database path. It refuses to create a nested
// project when one already exists up the directory tree unless --force is given.
func newInitCmd() *cobra.Command {
	var (
		goal    string
		root    string
		force   bool
		noGuide bool
	)
	cmd := &cobra.Command{
		Use:   "init",
		Short: "Create or refresh a ptrack project in the current repository",
		Long: "Create a .ptrack/ptrack.db database at the git root (or cwd, or an\n" +
			"explicit --root) and optionally seed a north-star goal. Run again in the\n" +
			"same project to refresh the agent guide (a no-op if unchanged). Refuses\n" +
			"only to nest a NEW project inside a different existing one, unless --force.",
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			cwd, err := os.Getwd()
			if err != nil {
				return err
			}
			targetDir, err := store.ResolveProjectDir(root)
			if err != nil {
				return err
			}

			// If a project already exists at/above cwd, decide: same project =>
			// refresh (sync); a different (ancestor) project => nesting, refuse
			// unless --force.
			if existing, ferr := store.FindProjectDB(cwd); ferr == nil {
				existingRoot := projectRoot(existing)
				if existingRoot == targetDir {
					return syncProject(cmd, existing, existingRoot, goal, noGuide)
				}
				if !force {
					return fmt.Errorf("already inside ptrack project at %s; run 'ptrack guide' to refresh docs, or 'ptrack init --force' to nest a new project", existingRoot)
				}
			} else if !errors.Is(ferr, store.ErrNoProject) {
				return ferr
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
			out := cmd.OutOrStdout()
			fmt.Fprintln(out, dbPath)
			return writeGuide(cmd, projectRoot(dbPath), noGuide)
		},
	}
	cmd.Flags().StringVar(&goal, "goal", "", "initial north-star goal text")
	cmd.Flags().StringVar(&root, "root", "", "explicit project directory (default: git root, else cwd)")
	cmd.Flags().BoolVar(&force, "force", false, "create even if a different project already exists above")
	cmd.Flags().BoolVar(&noGuide, "no-guide", false, "do not write the ptrack agent guide into AGENTS.md/CLAUDE.md")
	return cmd
}

// syncProject handles `init` run inside an already-initialized project: it
// optionally updates the goal and refreshes the agent guide, rather than
// erroring.
func syncProject(cmd *cobra.Command, dbPath, root, goal string, noGuide bool) error {
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
	registerProjectBestEffort(root)
	fmt.Fprintf(cmd.OutOrStdout(), "project already initialized at %s\n", dbPath)
	return writeGuide(cmd, root, noGuide)
}

// writeGuide installs/refreshes the agent guide into root's instruction files
// unless noGuide is set, reporting what it wrote.
func writeGuide(cmd *cobra.Command, root string, noGuide bool) error {
	if noGuide {
		return nil
	}
	extra, err := globalGuideExtra()
	if err != nil {
		return err
	}
	written, err := guide.Install(root, guide.DefaultFiles, extra)
	if err != nil {
		return err
	}
	out := cmd.OutOrStdout()
	if len(written) == 0 {
		fmt.Fprintln(out, "agent guide already up to date")
		return nil
	}
	for _, f := range written {
		fmt.Fprintf(out, "wrote agent guide to %s\n", f)
	}
	return nil
}
