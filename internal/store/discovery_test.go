package store

import (
	"os"
	"path/filepath"
	"testing"
)

func TestInitAndFindSameDir(t *testing.T) {
	dir := t.TempDir()
	db, err := InitProject(dir)
	if err != nil {
		t.Fatalf("InitProject: %v", err)
	}
	// materialize the file so FindProjectDB sees it.
	if err := os.WriteFile(db, []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	got, err := FindProjectDB(dir)
	if err != nil {
		t.Fatalf("FindProjectDB: %v", err)
	}
	if got != db {
		t.Errorf("got %q want %q", got, db)
	}
}

func TestFindFromNestedSubdir(t *testing.T) {
	root := t.TempDir()
	db, _ := InitProject(root)
	if err := os.WriteFile(db, []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	nested := filepath.Join(root, "a", "b", "c")
	if err := os.MkdirAll(nested, 0o755); err != nil {
		t.Fatal(err)
	}
	got, err := FindProjectDB(nested)
	if err != nil {
		t.Fatalf("FindProjectDB nested: %v", err)
	}
	if got != db {
		t.Errorf("got %q want %q", got, db)
	}
}

func TestFindNoProject(t *testing.T) {
	dir := t.TempDir()
	if _, err := FindProjectDB(dir); err != ErrNoProject {
		t.Errorf("want ErrNoProject, got %v", err)
	}
}

func TestFindStopsAtGitBoundary(t *testing.T) {
	// outer has a project; inner is a git root with no .ptrack. A search from
	// inner must NOT escape to outer's project.
	outer := t.TempDir()
	db, _ := InitProject(outer)
	if err := os.WriteFile(db, []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	inner := filepath.Join(outer, "sub")
	if err := os.MkdirAll(filepath.Join(inner, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	if _, err := FindProjectDB(inner); err != ErrNoProject {
		t.Errorf("want ErrNoProject at git boundary, got %v", err)
	}
}

func TestInitTwiceFails(t *testing.T) {
	dir := t.TempDir()
	db, _ := InitProject(dir)
	if err := os.WriteFile(db, []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := InitProject(dir); err == nil {
		t.Error("expected error initializing existing project")
	}
}
