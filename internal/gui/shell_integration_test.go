package gui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestEnsureShellPathAppendsManagedBlock(t *testing.T) {
	profile := filepath.Join(t.TempDir(), ".zprofile")
	changed, err := ensureShellPath(profile, "/Applications/P-TRACK.app/Contents/MacOS")
	if err != nil {
		t.Fatalf("ensureShellPath: %v", err)
	}
	if !changed {
		t.Fatal("expected the profile to be created")
	}
	data, err := os.ReadFile(profile)
	if err != nil {
		t.Fatalf("read profile: %v", err)
	}
	content := string(data)
	if !strings.Contains(content, shellPathMarkerBegin) || !strings.Contains(content, shellPathMarkerEnd) {
		t.Fatalf("managed markers missing:\n%s", content)
	}
	if !strings.Contains(content, `export PATH="$PATH:/Applications/P-TRACK.app/Contents/MacOS"`) {
		t.Fatalf("PATH entry missing:\n%s", content)
	}
}

func TestEnsureShellPathIsIdempotent(t *testing.T) {
	profile := filepath.Join(t.TempDir(), ".zprofile")
	if _, err := ensureShellPath(profile, "/bin/ptrack-app"); err != nil {
		t.Fatalf("first install: %v", err)
	}
	before, _ := os.ReadFile(profile)
	changed, err := ensureShellPath(profile, "/bin/ptrack-app")
	if err != nil {
		t.Fatalf("second install: %v", err)
	}
	if changed {
		t.Fatal("second install reported a change")
	}
	after, _ := os.ReadFile(profile)
	if string(before) != string(after) {
		t.Fatal("second install modified the profile")
	}
}

func TestEnsureShellPathPreservesExistingContent(t *testing.T) {
	profile := filepath.Join(t.TempDir(), ".zprofile")
	existing := "export EDITOR=vim\n# no trailing newline:"
	if err := os.WriteFile(profile, []byte(existing), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := ensureShellPath(profile, "/bin/ptrack-app"); err != nil {
		t.Fatalf("ensureShellPath: %v", err)
	}
	data, _ := os.ReadFile(profile)
	content := string(data)
	if !strings.HasPrefix(content, existing) {
		t.Fatalf("existing content clobbered:\n%s", content)
	}
	if !strings.Contains(content, ":\n"+shellPathMarkerBegin) {
		t.Fatalf("block not separated from trailing content:\n%s", content)
	}
}
