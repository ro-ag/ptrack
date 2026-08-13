//go:build !windows

package main

import (
	"os"
	"testing"
)

func requirePrivateTestPath(t *testing.T, path string, directory bool) {
	t.Helper()
	mode := os.FileMode(0o600)
	if directory {
		mode = 0o700
	}
	if err := os.Chmod(path, mode); err != nil {
		t.Fatal(err)
	}
}

func TestLegacySourceOpenRejectsSymlinkWithoutRequiringPrivateMode(t *testing.T) {
	root := t.TempDir()
	source := root + "/source.db"
	link := root + "/link.db"
	if err := os.WriteFile(source, []byte("legacy"), 0o644); err != nil {
		t.Fatal(err)
	}
	file, err := openLegacyExportSource(source)
	if err != nil {
		t.Fatal(err)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(source, link); err != nil {
		t.Fatal(err)
	}
	if file, err = openLegacyExportSource(link); err == nil {
		_ = file.Close()
		t.Fatal("symlink legacy source was accepted")
	}
}
