package cli

import (
	"errors"
	"fmt"

	"github.com/spf13/cobra"
)

// RunNoArgs is invoked when `ptrack` is run with no subcommand. main.go may
// override it (e.g. to launch a TUI) without cli importing the tui package.
var RunNoArgs func() error = defaultRunNoArgs

// RunGUI is invoked by `ptrack gui [PATH]` and the compatible
// `ptrack board --gui` alias. main.go replaces this callback so the CLI package
// does not need to import the desktop GUI package.
var RunGUI func(path string, planID uint64) error = func(string, uint64) error {
	return errors.New("GUI support is unavailable in this build")
}

// defaultRunNoArgs prints a hint pointing the user at help and status.
func defaultRunNoArgs() error {
	fmt.Println("ptrack: nothing to do. Run 'ptrack --help' for commands or 'ptrack status' for an overview.")
	return nil
}

// newRootCmd builds the full ptrack command tree.
func newRootCmd() *cobra.Command {
	root := &cobra.Command{
		Use:   "ptrack",
		Short: "p-track keeps project plans alive across human and AI sessions",
		Long: "p-track keeps project plans alive across human and AI sessions. It stores\n" +
			"goals, plans, tasks, issues, milestones, notes, and commit context in an embedded\n" +
			"bbolt database so a fresh agent can reload project context. Every subcommand is\n" +
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
		newMilestoneCmd(),
		newPlanCmd(),
		newTaskCmd(),
		newIssueCmd(),
		newNoteCmd(),
		newCommitCmd(),
		newHookCmd(),
		newContextCmd(),
		newGuideCmd(),
		newNextCmd(),
		newSearchCmd(),
		newBoardCmd(),
		newGUICmd(),
		newStatusCmd(),
		newProjectsCmd(),
		newBackupCmd(),
		newCapabilityCmd(),
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
