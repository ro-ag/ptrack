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

func TestUpsertOrphanedBeginMarker(t *testing.T) {
	// A begin marker with no end must not spawn a second block, and must keep
	// the surrounding human text.
	content := "# Doc\nintro\n\n" + beginMarker + "\nhalf-written note\n"
	updated, changed := upsert(content, Block(""))
	if !changed {
		t.Fatal("expected change")
	}
	if strings.Count(updated, beginMarker) != 1 {
		t.Errorf("want exactly one begin marker, got %d:\n%s", strings.Count(updated, beginMarker), updated)
	}
	if strings.Count(updated, endMarker) != 1 {
		t.Errorf("want exactly one end marker:\n%s", updated)
	}
	if !strings.Contains(updated, "half-written note") {
		t.Errorf("human text lost:\n%s", updated)
	}
	if !strings.Contains(updated, "intro") {
		t.Errorf("intro lost:\n%s", updated)
	}
}

func TestUpsertDuplicateBlocks(t *testing.T) {
	one := beginMarker + "\nOLD A\n" + endMarker + "\n"
	two := beginMarker + "\nOLD B\n" + endMarker + "\n"
	content := "top\n\n" + one + "\nmiddle\n\n" + two + "\nbottom\n"
	updated, changed := upsert(content, Block(""))
	if !changed {
		t.Fatal("expected change")
	}
	if strings.Count(updated, beginMarker) != 1 {
		t.Errorf("want exactly one block, got %d:\n%s", strings.Count(updated, beginMarker), updated)
	}
	for _, w := range []string{"top", "middle", "bottom"} {
		if !strings.Contains(updated, w) {
			t.Errorf("human text %q lost:\n%s", w, updated)
		}
	}
	if strings.Contains(updated, "OLD A") || strings.Contains(updated, "OLD B") {
		t.Errorf("stale block content retained:\n%s", updated)
	}
}

func TestUpsertNormalizedOutputIsIdempotent(t *testing.T) {
	// After normalizing a messy file, a second upsert is a no-op.
	messy := "doc\n\n" + beginMarker + "\norphan\n"
	once, _ := upsert(messy, Block(""))
	twice, changed := upsert(once, Block(""))
	if changed {
		t.Errorf("second upsert changed a normalized file:\n%s", twice)
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
