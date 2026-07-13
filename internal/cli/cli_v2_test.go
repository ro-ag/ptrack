package cli

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/ro-ag/ptrack/internal/store"
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

func TestInitSyncsSameProject(t *testing.T) {
	seedProject(t)
	// Re-running init in the same project refreshes rather than erroring.
	out, err := runCmd(t, "init")
	if err != nil {
		t.Fatalf("re-init should sync, got error: %v\n%s", err, out)
	}
	if !strings.Contains(out, "already initialized") {
		t.Errorf("expected sync message:\n%s", out)
	}
}

func TestInitRefusesGenuineNesting(t *testing.T) {
	t.Setenv("PTRACK_HOME", filepath.Join(t.TempDir(), "home"))
	root := chdirTemp(t)
	mustRun(t, "init") // project at root

	sub := filepath.Join(root, "sub")
	if err := os.MkdirAll(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	prev, _ := os.Getwd()
	if err := os.Chdir(sub); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chdir(prev) })

	// In a subdir (no git boundary), init would target sub — a different root
	// than the existing project at root — so it must refuse without --force.
	if _, err := runCmd(t, "init"); err == nil {
		t.Fatal("expected nesting refusal in subdir")
	}
	// With --force it proceeds.
	if _, err := runCmd(t, "init", "--force"); err != nil {
		t.Fatalf("--force should nest, got: %v", err)
	}
}

func openTestStore(t *testing.T) *store.Store {
	t.Helper()
	cwd, _ := os.Getwd()
	db, err := store.FindProjectDB(cwd)
	if err != nil {
		t.Fatal(err)
	}
	s, err := store.Open(db)
	if err != nil {
		t.Fatal(err)
	}
	return s
}
