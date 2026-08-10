package cli

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

func TestNoteListLabelsTypedMemoryAndKeepsLegacyShape(t *testing.T) {
	t.Setenv("PTRACK_HOME", filepath.Join(t.TempDir(), "ptrack-home"))
	chdirTemp(t)
	if _, err := runCmd(t, "init", "--goal", "G"); err != nil {
		t.Fatal(err)
	}
	s, err := openProject()
	if err != nil {
		t.Fatal(err)
	}
	_, err = s.WriteMemory(store.MemoryWriteRequest{
		RequestID: "typed-cli-memory", Kind: model.MemoryDecision,
		Body: "use atomic writes", Target: model.TargetProject,
		WorkspaceGeneration: 1, SessionID: "session", AssociationRevision: 1,
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.AddNote(model.TargetProject, 0, "legacy note"); err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	out, err := runCmd(t, "note", "list", "--limit", "0")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out, "(decision · project) use atomic writes") ||
		!strings.Contains(out, "(project) legacy note") {
		t.Fatalf("typed note list = %q", out)
	}
	jsonOut, err := runCmd(t, "note", "list", "--json", "--limit", "0")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(jsonOut, `"kind": "decision"`) ||
		strings.Count(jsonOut, `"kind"`) != 1 {
		t.Fatalf("typed note JSON = %q", jsonOut)
	}
}
