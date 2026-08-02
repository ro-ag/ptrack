//go:build bindings || (desktop && (production || dev))

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
	cli.RunGUI = func(path string, planID uint64) error {
		assets, err := fs.Sub(guiAssets, "frontend/dist")
		if err != nil {
			return err
		}
		return gui.Run(path, planID, assets)
	}
	// `ptrack` with no subcommand launches the terminal dashboard; outside a
	// project it prints a friendly getting-started hint instead of a bare
	// error. The desktop app bundle never relies on this path: its launcher
	// always invokes the explicit `ptrack gui` subcommand.
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
