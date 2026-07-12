package store

import (
	"os"
	"path/filepath"
	"testing"
)

func openGlobalTemp(t *testing.T) *Global {
	t.Helper()
	t.Setenv("PTRACK_HOME", t.TempDir())
	g, err := OpenGlobal()
	if err != nil {
		t.Fatalf("OpenGlobal: %v", err)
	}
	t.Cleanup(func() { _ = g.Close() })
	return g
}

func TestConfigSetGet(t *testing.T) {
	g := openGlobalTemp(t)
	if v, _ := g.GetConfig("editor"); v != "" {
		t.Errorf("unset config = %q want empty", v)
	}
	if err := g.SetConfig("editor", "nvim"); err != nil {
		t.Fatal(err)
	}
	if v, _ := g.GetConfig("editor"); v != "nvim" {
		t.Errorf("config = %q want nvim", v)
	}
}

func TestProjectRegistry(t *testing.T) {
	g := openGlobalTemp(t)
	if err := g.RegisterProject("alpha", t.TempDir()); err != nil {
		t.Fatal(err)
	}
	if err := g.RegisterProject("beta", t.TempDir()); err != nil {
		t.Fatal(err)
	}
	refs, err := g.ListProjects()
	if err != nil {
		t.Fatal(err)
	}
	if len(refs) != 2 {
		t.Fatalf("ListProjects = %d want 2", len(refs))
	}
	// most-recent-first: beta registered last.
	if refs[0].Name != "beta" {
		t.Errorf("first ref = %q want beta", refs[0].Name)
	}
}

func TestBackupProject(t *testing.T) {
	src := filepath.Join(t.TempDir(), ".ptrack", "ptrack.db")
	if err := os.MkdirAll(filepath.Dir(src), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(src, []byte("dbcontent"), 0o600); err != nil {
		t.Fatal(err)
	}
	dest := t.TempDir()
	backup, err := BackupProject(src, dest, 1720000000)
	if err != nil {
		t.Fatalf("BackupProject: %v", err)
	}
	data, err := os.ReadFile(backup)
	if err != nil {
		t.Fatalf("read backup: %v", err)
	}
	if string(data) != "dbcontent" {
		t.Errorf("backup content = %q want dbcontent", data)
	}
	if filepath.Ext(backup) != ".db" {
		t.Errorf("backup name %q should end .db", backup)
	}
}
