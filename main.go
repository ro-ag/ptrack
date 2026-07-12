// Command ptrack persists AI planning state across sessions.
package main

import (
	"fmt"
	"os"

	"github.com/ro-ag/ptrack/internal/cli"
	"github.com/ro-ag/ptrack/internal/tui"
)

func main() {
	// `ptrack` with no subcommand launches the human-facing dashboard.
	cli.RunNoArgs = tui.Run
	if err := cli.Execute(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
