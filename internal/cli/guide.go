package cli

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/ro-ag/ptrack/internal/guide"
	"github.com/ro-ag/ptrack/internal/store"
	"github.com/spf13/cobra"
)

// globalGuideExtra returns the contents of the user's global guide template
// (<global home>/guide.md), or "" when it does not exist. These are appended to
// the installed guide as the user's own working agreements.
func globalGuideExtra() (string, error) {
	home, err := store.GlobalHome()
	if err != nil {
		return "", err
	}
	data, err := os.ReadFile(filepath.Join(home, guide.TemplateName))
	if err != nil {
		if os.IsNotExist(err) {
			return "", nil
		}
		return "", err
	}
	return string(data), nil
}

// newGuideCmd builds `ptrack guide`: install or refresh the ptrack agent guide
// in the project's AGENTS.md/CLAUDE.md, or print it with --print.
func newGuideCmd() *cobra.Command {
	var printOnly bool
	cmd := &cobra.Command{
		Use:   "guide",
		Short: "Install or print the ptrack agent guide (how an AI agent uses ptrack)",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			out := cmd.OutOrStdout()
			extra, err := globalGuideExtra()
			if err != nil {
				return err
			}
			if printOnly {
				fmt.Fprint(out, guide.Rendered(extra))
				return nil
			}
			cwd, err := os.Getwd()
			if err != nil {
				return err
			}
			dbPath, err := store.FindProjectDB(cwd)
			if err != nil {
				return err
			}
			written, err := guide.Install(projectRoot(dbPath), guide.DefaultFiles, extra)
			if err != nil {
				return err
			}
			if len(written) == 0 {
				fmt.Fprintln(out, "agent guide already up to date")
				return nil
			}
			for _, f := range written {
				fmt.Fprintf(out, "wrote agent guide to %s\n", f)
			}
			return nil
		},
	}
	cmd.Flags().BoolVar(&printOnly, "print", false, "print the guide to stdout instead of writing files")
	return cmd
}
