//go:build bindings

// Command ptrack exposes its Wails bindings during native builds.
package main

import (
	"embed"
	"fmt"
	"io/fs"
	"os"

	"github.com/ro-ag/ptrack/internal/gui"
)

//go:embed all:frontend/dist
var guiAssets embed.FS

func main() {
	assets, err := fs.Sub(guiAssets, "frontend/dist")
	if err == nil {
		err = gui.RunBindings(assets)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
