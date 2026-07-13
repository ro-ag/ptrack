package cli

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestInitWritesGuide(t *testing.T) {
	t.Setenv("PTRACK_HOME", filepath.Join(t.TempDir(), "home"))
	dir := chdirTemp(t)
	mustRun(t, "init", "--goal", "G")

	for _, name := range []string{"AGENTS.md", "CLAUDE.md"} {
		data, err := os.ReadFile(filepath.Join(dir, name))
		if err != nil {
			t.Fatalf("expected %s written: %v", name, err)
		}
		if !strings.Contains(string(data), "ptrack context") {
			t.Errorf("%s missing guide body", name)
		}
	}
}

func TestInitNoGuideSkips(t *testing.T) {
	t.Setenv("PTRACK_HOME", filepath.Join(t.TempDir(), "home"))
	dir := chdirTemp(t)
	mustRun(t, "init", "--no-guide")

	if _, err := os.Stat(filepath.Join(dir, "AGENTS.md")); !os.IsNotExist(err) {
		t.Errorf("--no-guide should not write AGENTS.md (err=%v)", err)
	}
}

func TestGuidePrint(t *testing.T) {
	out := mustRun(t, "guide", "--print")
	if !strings.Contains(out, "ptrack context") || !strings.Contains(out, "summary set") {
		t.Errorf("guide --print missing content:\n%s", out)
	}
}
