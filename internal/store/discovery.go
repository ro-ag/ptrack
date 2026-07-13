package store

import (
	"errors"
	"os"
	"path/filepath"
)

// ErrNoProject is returned when no ptrack project is found by walking up from a
// starting directory.
var ErrNoProject = errors.New("no ptrack project found (run 'ptrack init')")

const (
	ptrackDir = ".ptrack"
	dbFile    = "ptrack.db"
	gitDir    = ".git"
)

// FindProjectDB walks up from start looking for an existing
// <dir>/.ptrack/ptrack.db. It stops after inspecting a directory that contains
// a .git/ directory (the repository boundary) and at the filesystem root.
// Returns ErrNoProject when nothing is found.
func FindProjectDB(start string) (string, error) {
	dir, err := filepath.Abs(start)
	if err != nil {
		return "", err
	}
	for {
		db := filepath.Join(dir, ptrackDir, dbFile)
		if fi, err := os.Stat(db); err == nil && !fi.IsDir() {
			return db, nil
		}
		// Stop at the repo boundary: if this dir is a git root and had no
		// .ptrack, don't escape the repository.
		if isDir(filepath.Join(dir, gitDir)) {
			return "", ErrNoProject
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return "", ErrNoProject
		}
		dir = parent
	}
}

// ResolveProjectDir reports the absolute directory where InitProject(dir) would
// create a project: dir itself, or (when dir is empty) the enclosing git root,
// else the current working directory.
func ResolveProjectDir(dir string) (string, error) {
	if dir == "" {
		cwd, err := os.Getwd()
		if err != nil {
			return "", err
		}
		dir = gitRootOr(cwd)
	}
	return filepath.Abs(dir)
}

// InitProject creates <dir>/.ptrack/ptrack.db and returns its path. If dir is
// empty, it defaults to the enclosing git root (if any) or the current working
// directory. It is an error if a project DB already exists at the chosen dir.
func InitProject(dir string) (string, error) {
	abs, err := ResolveProjectDir(dir)
	if err != nil {
		return "", err
	}
	db := filepath.Join(abs, ptrackDir, dbFile)
	if _, err := os.Stat(db); err == nil {
		return "", errors.New("ptrack project already exists at " + db)
	}
	if err := os.MkdirAll(filepath.Join(abs, ptrackDir), 0o755); err != nil {
		return "", err
	}
	return db, nil
}

// gitRootOr returns the nearest ancestor of start containing a .git directory,
// or start itself if none is found.
func gitRootOr(start string) string {
	dir := start
	for {
		if isDir(filepath.Join(dir, gitDir)) {
			return dir
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return start
		}
		dir = parent
	}
}

func isDir(path string) bool {
	fi, err := os.Stat(path)
	return err == nil && fi.IsDir()
}
