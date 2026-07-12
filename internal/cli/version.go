package cli

import (
	"fmt"
	"runtime/debug"

	"github.com/spf13/cobra"
)

// Version is the ptrack build version. It is empty in plain `go build`, stamped
// at link time by release builds (-X github.com/ro-ag/ptrack/internal/cli.Version=…),
// and otherwise resolved from the module build info (as set by `go install …@vX`).
var Version = ""

// version resolves the effective version string: the link-time value if set,
// else the module version from build info, else "dev".
func version() string {
	if Version != "" {
		return Version
	}
	if bi, ok := debug.ReadBuildInfo(); ok {
		if v := bi.Main.Version; v != "" && v != "(devel)" {
			return v
		}
	}
	return "dev"
}

// newVersionCmd builds `ptrack version`.
func newVersionCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "version",
		Short: "Print the ptrack version",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			fmt.Fprintf(cmd.OutOrStdout(), "ptrack %s\n", version())
			return nil
		},
	}
}
