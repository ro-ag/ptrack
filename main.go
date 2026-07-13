// Command ptrack persists AI planning state across sessions.
package main

import (
	"errors"
	"fmt"
	"os"

	"github.com/ro-ag/ptrack/internal/cli"
	"github.com/ro-ag/ptrack/internal/store"
	"github.com/ro-ag/ptrack/internal/tui"
)

func main() {
	// Record which ptrack version writes the database, for diagnostics.
	store.WriterVersion = cli.VersionString()
	// `ptrack` with no subcommand launches the dashboard; outside a project it
	// prints a friendly getting-started hint instead of a bare error.
	cli.RunNoArgs = func() error {
		err := tui.Run()
		if errors.Is(err, store.ErrNoProject) {
			fmt.Print(cli.NoProjectHint())
			return nil
		}
		return err
	}
	if err := cli.Execute(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
