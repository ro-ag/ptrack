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
	// `ptrack` with no subcommand launches the dashboard; outside a project it
	// prints a friendly getting-started hint instead of a bare error. When the
	// process was started without a controlling terminal — double-clicking the
	// app bundle from Finder or the Dock — a TUI can never render, so open the
	// desktop GUI instead.
	cli.RunNoArgs = func() error {
		if !hasInteractiveTerminal() {
			return cli.RunGUI("", 0)
		}
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

// hasInteractiveTerminal reports whether stdin and stdout are both character
// devices. Finder/Dock launches attach neither, so the desktop GUI is the only
// usable interface in that case.
func hasInteractiveTerminal() bool {
	for _, file := range []*os.File{os.Stdin, os.Stdout} {
		info, err := file.Stat()
		if err != nil || info.Mode()&os.ModeCharDevice == 0 {
			return false
		}
	}
	return true
}
