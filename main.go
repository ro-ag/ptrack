// Command ptrack persists AI planning state across sessions.
package main

import (
	"embed"
	"errors"
	"fmt"
	"io/fs"
	"os"

	"github.com/ro-ag/ptrack/internal/cli"
	"github.com/ro-ag/ptrack/internal/gui"
	"github.com/ro-ag/ptrack/internal/store"
	"github.com/ro-ag/ptrack/internal/tui"
)

//go:embed all:frontend/dist
var guiAssets embed.FS

func main() {
	// Record which ptrack version writes the database, for diagnostics.
	store.WriterVersion = cli.VersionString()
	cli.RunGUI = func(planID uint64) error {
		assets, err := fs.Sub(guiAssets, "frontend/dist")
		if err != nil {
			return err
		}
		return gui.Run(planID, assets)
	}
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
