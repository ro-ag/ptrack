package cli

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// runCmd executes the root command tree with the given args, capturing stdout
// and stderr into buffers. It returns the captured output and any execution
// error. Output is captured rather than going through os.Exit.
func runCmd(t *testing.T, args ...string) (string, error) {
	t.Helper()
	root := newRootCmd()
	var out, errOut bytes.Buffer
	root.SetOut(&out)
	root.SetErr(&errOut)
	root.SetArgs(args)
	err := root.Execute()
	combined := out.String() + errOut.String()
	return combined, err
}

// chdirTemp changes into a fresh temp directory for the duration of the test.
func chdirTemp(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	prev, err := os.Getwd()
	if err != nil {
		t.Fatalf("getwd: %v", err)
	}
	if err := os.Chdir(dir); err != nil {
		t.Fatalf("chdir: %v", err)
	}
	t.Cleanup(func() { _ = os.Chdir(prev) })
	return dir
}

func TestIntegrationFlow(t *testing.T) {
	// Isolate the global ptrack home so OpenGlobal never touches the user's
	// real ~/.ptrack.
	t.Setenv("PTRACK_HOME", filepath.Join(t.TempDir(), "ptrack-home"))
	chdirTemp(t)

	if _, err := runCmd(t, "init", "--goal", "G"); err != nil {
		t.Fatalf("init: %v", err)
	}
	if _, err := runCmd(t, "plan", "add", "P"); err != nil {
		t.Fatalf("plan add: %v", err)
	}
	if _, err := runCmd(t, "plan", "use", "1"); err != nil {
		t.Fatalf("plan use: %v", err)
	}
	if _, err := runCmd(t, "task", "add", "T"); err != nil {
		t.Fatalf("task add: %v", err)
	}

	out, err := runCmd(t, "context")
	if err != nil {
		t.Fatalf("context: %v", err)
	}
	if !strings.Contains(out, "T") {
		t.Errorf("context output missing task title \"T\":\n%s", out)
	}
	if !strings.Contains(out, "G") {
		t.Errorf("context output missing goal \"G\":\n%s", out)
	}
}

func TestParseIDInvalid(t *testing.T) {
	if _, err := parseID("abc"); err == nil {
		t.Fatal("expected error for non-numeric id")
	}
}

func TestProjectRoot(t *testing.T) {
	got := projectRoot(filepath.Join("/tmp", "proj", ".ptrack", "ptrack.db"))
	want := filepath.Join("/tmp", "proj")
	if got != want {
		t.Errorf("projectRoot = %q want %q", got, want)
	}
}

// Ensure the root command wired up every expected subcommand.
func TestRootHasSubcommands(t *testing.T) {
	root := newRootCmd()
	want := []string{"init", "goal", "summary", "plan", "task", "note", "context", "status", "projects", "backup"}
	got := map[string]bool{}
	for _, c := range root.Commands() {
		got[c.Name()] = true
	}
	for _, name := range want {
		if !got[name] {
			t.Errorf("root missing subcommand %q", name)
		}
	}
}
