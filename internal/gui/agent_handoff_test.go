package gui

import (
	"context"
	"encoding/json"
	"errors"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
)

func TestPreviewAgentHandoffV2IsExplicitBoundedAndNonMutating(t *testing.T) {
	app, projectRoot := newTerminalBindingTestApp(t, &fakeGUITerminalManager{}, nil)
	planID, taskID, _ := seedAssociationCatalog(t, projectRoot)
	dbPath := filepath.Join(projectRoot, ".ptrack", "ptrack.db")
	policy := agentrun.DefaultEventPrivacyPolicy()
	policy.AllowSummaries = true
	registry := agentrun.NewRegistry(agentrun.Config{ProjectRoot: projectRoot, EventPolicy: &policy})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	app.workspace.agents = registry
	lease, err := registry.RegisterExternal(agentrun.Registration{
		Profile: "wrapper", Provider: "claude", CWD: projectRoot,
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := app.AssociateAgentRunV2(1, lease.Run.ID, association.PointerV1{
		Version: association.VersionV1, PlanID: planID, TaskID: taskID,
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := registry.RecordEvent(lease.Run.ID, lease.LeaseToken, agentrun.EventObservation{
		ModelVersion: agentrun.EventModelVersion, SourceID: "summary-1", SourceSequence: 1,
		Kind: agentrun.EventSummary, Phase: agentrun.EventCompleted,
		Summary: "Bearer HANDOFF_API_SECRET completed the adapter coverage.",
	}); err != nil {
		t.Fatal(err)
	}
	before, err := readSuggestionMemoryState(dbPath, taskID)
	if err != nil {
		t.Fatal(err)
	}
	result, err := app.PreviewAgentHandoffV2(1, lease.Run.ID)
	if err != nil {
		t.Fatal(err)
	}
	after, err := readSuggestionMemoryState(dbPath, taskID)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(before, after) {
		t.Fatal("handoff preview mutated project memory")
	}
	if result.Generation != 1 || result.RunID != lease.Run.ID ||
		result.Association == nil || result.Association.TaskID != taskID ||
		result.EventBounds.Total != 1 ||
		!strings.Contains(result.Preview.Text, "task #1") ||
		strings.Contains(result.Preview.Text, "HANDOFF_API_SECRET") {
		t.Fatalf("handoff preview = %#v", result)
	}
	encoded, err := json.Marshal(result)
	if err != nil {
		t.Fatal(err)
	}
	for _, forbidden := range []string{`"provider":`, `"projectRoot":`, `"summary":`, "HANDOFF_API_SECRET"} {
		if strings.Contains(string(encoded), forbidden) {
			t.Fatalf("handoff DTO contains forbidden %q: %s", forbidden, encoded)
		}
	}
}

func TestPreviewAgentHandoffV2RejectsStaleGenerationAndUnknownRun(t *testing.T) {
	app, _ := newTerminalBindingTestApp(t, &fakeGUITerminalManager{}, nil)
	registry := agentrun.NewRegistry(agentrun.Config{ProjectRoot: app.workspace.root})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	app.workspace.agents = registry
	if _, err := app.PreviewAgentHandoffV2(2, "missing"); !errors.Is(err, errStaleWorkspaceGeneration) {
		t.Fatalf("stale handoff generation error = %v", err)
	}
	if _, err := app.PreviewAgentHandoffV2(1, "missing"); !errors.Is(err, agentrun.ErrRunNotFound) {
		t.Fatalf("unknown handoff run error = %v", err)
	}
}
