package guide

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestInstallCreatesFiles(t *testing.T) {
	dir := t.TempDir()
	written, err := Install(dir, DefaultFiles, "")
	if err != nil {
		t.Fatal(err)
	}
	if len(written) != len(DefaultFiles) {
		t.Fatalf("wrote %d files, want %d", len(written), len(DefaultFiles))
	}
	for _, name := range DefaultFiles {
		data, err := os.ReadFile(filepath.Join(dir, name))
		if err != nil {
			t.Fatalf("read %s: %v", name, err)
		}
		s := string(data)
		if !strings.Contains(s, beginMarker) || !strings.Contains(s, endMarker) {
			t.Errorf("%s missing markers", name)
		}
		if !strings.Contains(s, "ptrack context") {
			t.Errorf("%s missing guide body", name)
		}
	}
}

func TestInstallIdempotent(t *testing.T) {
	dir := t.TempDir()
	if _, err := Install(dir, DefaultFiles, ""); err != nil {
		t.Fatal(err)
	}
	written, err := Install(dir, DefaultFiles, "")
	if err != nil {
		t.Fatal(err)
	}
	if len(written) != 0 {
		t.Errorf("second install rewrote %v, want no-op", written)
	}
}

func TestInstallWithExtra(t *testing.T) {
	dir := t.TempDir()
	extra := "## Working agreements\n\n- Branch first."
	if _, err := Install(dir, []string{"AGENTS.md"}, extra); err != nil {
		t.Fatal(err)
	}
	data, _ := os.ReadFile(filepath.Join(dir, "AGENTS.md"))
	s := string(data)
	if !strings.Contains(s, "Working agreements") || !strings.Contains(s, "Branch first.") {
		t.Errorf("extra guidelines missing:\n%s", s)
	}
	if !strings.Contains(s, "ptrack context") {
		t.Errorf("built-in body missing:\n%s", s)
	}
	// Changing extra rewrites the block idempotently (one block only).
	if _, err := Install(dir, []string{"AGENTS.md"}, extra+"\n- No AI attribution."); err != nil {
		t.Fatal(err)
	}
	data, _ = os.ReadFile(filepath.Join(dir, "AGENTS.md"))
	if strings.Count(string(data), beginMarker) != 1 {
		t.Errorf("want exactly one block after extra change:\n%s", data)
	}
	if !strings.Contains(string(data), "No AI attribution.") {
		t.Errorf("updated extra not applied:\n%s", data)
	}
}

func TestInstallPreservesSurroundingContent(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "AGENTS.md")
	original := "# My rules\n\nDo not break things.\n"
	if err := os.WriteFile(path, []byte(original), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := Install(dir, []string{"AGENTS.md"}, ""); err != nil {
		t.Fatal(err)
	}
	data, _ := os.ReadFile(path)
	s := string(data)
	if !strings.Contains(s, "Do not break things.") {
		t.Errorf("original content lost:\n%s", s)
	}
	if !strings.Contains(s, beginMarker) {
		t.Errorf("guide not appended:\n%s", s)
	}
}

func TestUpsertReplacesInPlace(t *testing.T) {
	// A file with an existing (stale) block gets exactly one block after upsert.
	content := "intro\n\n" + beginMarker + "\nOLD\n" + endMarker + "\n\noutro\n"
	updated, changed := upsert(content, Block(""))
	if !changed {
		t.Fatal("expected change")
	}
	if strings.Count(updated, beginMarker) != 1 {
		t.Errorf("want exactly one block:\n%s", updated)
	}
	if strings.Contains(updated, "OLD") {
		t.Errorf("stale content retained:\n%s", updated)
	}
	if !strings.Contains(updated, "intro") || !strings.Contains(updated, "outro") {
		t.Errorf("surrounding content lost:\n%s", updated)
	}
}
