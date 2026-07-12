package cli

import (
	"fmt"

	"github.com/spf13/cobra"
)

// RunNoArgs is invoked when `ptrack` is run with no subcommand. main.go may
// override it (e.g. to launch a TUI) without cli importing the tui package.
var RunNoArgs func() error = defaultRunNoArgs

// defaultRunNoArgs prints a hint pointing the user at help and status.
func defaultRunNoArgs() error {
	fmt.Println("ptrack: nothing to do. Run 'ptrack --help' for commands or 'ptrack status' for an overview.")
	return nil
}

// newRootCmd builds the full ptrack command tree.
func newRootCmd() *cobra.Command {
	root := &cobra.Command{
		Use:   "ptrack",
		Short: "Persist AI planning state across sessions",
		Long: "ptrack persists AI planning state — goals, plans, tasks, and notes — in a\n" +
			"bbolt database so a fresh agent can reload project context. Every command is\n" +
			"non-interactive and exits non-zero on error.",
		RunE: func(cmd *cobra.Command, args []string) error {
			return RunNoArgs()
		},
		Version:      version(),
		SilenceUsage: true,
	}

	root.AddCommand(
		newInitCmd(),
		newGoalCmd(),
		newSummaryCmd(),
		newPlanCmd(),
		newTaskCmd(),
		newNoteCmd(),
		newContextCmd(),
		newNextCmd(),
		newSearchCmd(),
		newBoardCmd(),
		newStatusCmd(),
		newProjectsCmd(),
		newBackupCmd(),
		newVersionCmd(),
	)
	// Let main.go own error reporting; silence cobra's own error/usage prints.
	silence(root)
	return root
}

// silence recursively silences error and usage auto-printing on cmd and all of
// its descendants so the caller (main.go) controls error presentation.
func silence(cmd *cobra.Command) {
	cmd.SilenceErrors = true
	cmd.SilenceUsage = true
	for _, c := range cmd.Commands() {
		silence(c)
	}
}

// Execute runs the root command and returns any error it produces.
func Execute() error {
	return newRootCmd().Execute()
}
