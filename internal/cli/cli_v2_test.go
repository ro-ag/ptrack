package cli

import (
	"path/filepath"
	"strings"
	"testing"
)

// seedProject initializes a project with one active plan and three tasks in
// distinct statuses, returning nothing (state lives in the cwd/global home).
func seedProject(t *testing.T) {
	t.Helper()
	t.Setenv("PTRACK_HOME", filepath.Join(t.TempDir(), "home"))
	chdirTemp(t)
	mustRun(t, "init", "--goal", "Ship service")
	mustRun(t, "plan", "add", "Storage")
	mustRun(t, "plan", "use", "1")
	mustRun(t, "task", "add", "buckets")
	mustRun(t, "task", "add", "crud")
	mustRun(t, "task", "add", "backup")
	mustRun(t, "task", "start", "1")
	mustRun(t, "task", "block", "3")
}

func mustRun(t *testing.T, args ...string) string {
	t.Helper()
	out, err := runCmd(t, args...)
	if err != nil {
		t.Fatalf("%v: %v\n%s", args, err, out)
	}
	return out
}

func TestNextCommand(t *testing.T) {
	seedProject(t)
	out := mustRun(t, "next")
	if !strings.Contains(out, "buckets") { // the doing task
		t.Errorf("next missing doing task:\n%s", out)
	}
}

func TestBoardCommand(t *testing.T) {
	seedProject(t)
	out := mustRun(t, "board")
	for _, w := range []string{"Todo (1)", "Doing (1)", "Blocked (1)", "Done (0)", "crud", "buckets", "backup"} {
		if !strings.Contains(out, w) {
			t.Errorf("board missing %q:\n%s", w, out)
		}
	}
}

func TestTaskListStatusFilter(t *testing.T) {
	seedProject(t)
	out := mustRun(t, "task", "list", "--status", "doing,blocked")
	if !strings.Contains(out, "buckets") || !strings.Contains(out, "backup") {
		t.Errorf("filter missing expected tasks:\n%s", out)
	}
	if strings.Contains(out, "crud") {
		t.Errorf("filter should exclude todo 'crud':\n%s", out)
	}
	if _, err := runCmd(t, "task", "list", "--status", "bogus"); err == nil {
		t.Error("expected error for invalid status")
	}
}

func TestSearchCommand(t *testing.T) {
	seedProject(t)
	out := mustRun(t, "search", "crud")
	if !strings.Contains(out, "crud") {
		t.Errorf("search missing match:\n%s", out)
	}
}

func TestNoteListCommand(t *testing.T) {
	seedProject(t)
	mustRun(t, "note", "add", "chose bbolt", "--task", "1")
	out := mustRun(t, "note", "list")
	if !strings.Contains(out, "chose bbolt") {
		t.Errorf("note list missing note:\n%s", out)
	}
}

func TestInitNestedRefused(t *testing.T) {
	seedProject(t)
	// From the same project dir, a second init must refuse.
	if _, err := runCmd(t, "init"); err == nil {
		t.Fatal("expected init to refuse inside an existing project")
	}
	// With --force it should proceed (creates .ptrack in cwd; already exists here
	// though, so this specific dir errors on already-exists — use a subdir).
}
