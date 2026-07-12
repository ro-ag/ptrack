// Package cli implements the agent-facing, scriptable command surface of
// ptrack. Every command is non-interactive and exits non-zero on error.
package cli

import (
	"errors"
	"os"
	"path/filepath"

	"github.com/ro-ag/ptrack/internal/store"
)

// openProject locates and opens the current project's database. It walks up
// from the working directory for a .ptrack/ptrack.db file. On success it
// best-effort registers the project in the global registry so LastSeen stays
// fresh; global-store failures are ignored and never fail the command.
func openProject() (*store.Store, error) {
	cwd, err := os.Getwd()
	if err != nil {
		return nil, err
	}
	dbPath, err := store.FindProjectDB(cwd)
	if err != nil {
		if errors.Is(err, store.ErrNoProject) {
			return nil, store.ErrNoProject
		}
		return nil, err
	}
	s, err := store.Open(dbPath)
	if err != nil {
		return nil, err
	}
	registerProjectBestEffort(projectRoot(dbPath))
	return s, nil
}

// registerProjectBestEffort opens the global store and records the project. Any
// error is ignored: the registry is a convenience, not a prerequisite.
func registerProjectBestEffort(root string) {
	g, err := store.OpenGlobal()
	if err != nil {
		return
	}
	defer g.Close()
	_ = g.RegisterProject(filepath.Base(root), root)
}

// projectRoot returns the project directory that contains the .ptrack folder
// for the given db path (<root>/.ptrack/ptrack.db -> <root>).
func projectRoot(dbPath string) string {
	return filepath.Dir(filepath.Dir(dbPath))
}
