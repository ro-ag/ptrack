package cli

import "github.com/spf13/cobra"

// newGUICmd builds the canonical desktop workspace command. An empty path lets
// the GUI resolve the current directory at startup.
func newGUICmd() *cobra.Command {
	return &cobra.Command{
		Use:   "gui [PATH]",
		Short: "Open the p-track desktop project workspace",
		Args:  cobra.MaximumNArgs(1),
		RunE: func(_ *cobra.Command, args []string) error {
			path := ""
			if len(args) == 1 {
				path = args[0]
			}
			return RunGUI(path, 0)
		},
	}
}
