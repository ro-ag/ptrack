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
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

func TestGetAgentIntelligenceV2SuggestsBoundedContextWithoutMutation(t *testing.T) {
	app, projectRoot := newTerminalBindingTestApp(t, &fakeGUITerminalManager{}, nil)
	planID, taskID, _ := seedAssociationCatalog(t, projectRoot)
	dbPath := filepath.Join(projectRoot, ".ptrack", "ptrack.db")
	s, err := store.Open(dbPath)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.WriteMemory(store.MemoryWriteRequest{
		RequestID: "decision-1", Kind: model.MemoryDecision,
		Body:   "Keep provider payloads behind the normalized boundary.",
		Target: model.TargetTask, TargetID: taskID, PlanID: planID,
		WorkspaceGeneration: 1, SessionID: "test-session", AssociationRevision: 1,
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := s.AddIssue(
		"Verify provider ordering", "", model.SeverityHigh, taskID,
	); err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}

	policy := agentrun.DefaultEventPrivacyPolicy()
	policy.AllowSummaries = true
	registry := agentrun.NewRegistry(agentrun.Config{ProjectRoot: projectRoot, EventPolicy: &policy})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	app.workspace.agents = registry
	lease, err := registry.RegisterExternal(agentrun.Registration{
		Profile: "wrapper", Provider: "codex", CWD: projectRoot,
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
		ModelVersion: agentrun.EventModelVersion, SourceID: "file-1", SourceSequence: 1,
		Kind: agentrun.EventFile, Phase: agentrun.EventProgress, Subject: "write",
		Paths: []string{"internal/agentrun/intelligence.go"},
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := registry.RecordEvent(lease.Run.ID, lease.LeaseToken, agentrun.EventObservation{
		ModelVersion: agentrun.EventModelVersion, SourceID: "summary-2", SourceSequence: 2,
		Kind: agentrun.EventSummary, Phase: agentrun.EventCompleted,
		Summary: "SUMMARY_TEXT_CANARY token=TOKEN_VALUE_CANARY",
	}); err != nil {
		t.Fatal(err)
	}

	before, err := readSuggestionMemoryState(dbPath, taskID)
	if err != nil {
		t.Fatal(err)
	}
	result, err := app.GetAgentIntelligenceV2(1, lease.Run.ID)
	if err != nil {
		t.Fatal(err)
	}
	after, err := readSuggestionMemoryState(dbPath, taskID)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(before, after) {
		t.Fatal("read-only intelligence request mutated project storage")
	}
	if result.Generation != 1 || result.RunID != lease.Run.ID ||
		result.Association == nil || result.Association.PlanID != planID ||
		result.Association.TaskID != taskID || result.EventBounds.Total != 2 ||
		result.Intelligence.State != agentrun.IntelligenceWorking {
		t.Fatalf("agent intelligence = %#v", result)
	}
	wantKinds := map[AgentSuggestionKind]bool{
		AgentSuggestionContext:  false,
		AgentSuggestionFile:     false,
		AgentSuggestionDecision: false,
		AgentSuggestionIssue:    false,
	}
	for _, suggestion := range result.Suggestions {
		wantKinds[suggestion.Kind] = true
	}
	for kind, found := range wantKinds {
		if !found {
			t.Fatalf("suggestions lack %q: %#v", kind, result.Suggestions)
		}
	}
	encoded, err := json.Marshal(result)
	if err != nil {
		t.Fatal(err)
	}
	for _, forbidden := range []string{
		"SUMMARY_TEXT_CANARY", "TOKEN_VALUE_CANARY",
		`"provider":`, `"projectRoot":`, `"summary":`, `"paths":`,
	} {
		if strings.Contains(string(encoded), forbidden) {
			t.Fatalf("intelligence DTO contains forbidden %q: %s", forbidden, encoded)
		}
	}
}

type suggestionMemoryState struct {
	Goal       string
	Summary    string
	ActivePlan uint64
	Task       model.Task
	Notes      []model.Note
	Issues     []model.Issue
}

func readSuggestionMemoryState(
	dbPath string,
	taskID uint64,
) (suggestionMemoryState, error) {
	s, err := store.Open(dbPath)
	if err != nil {
		return suggestionMemoryState{}, err
	}
	defer s.Close()
	meta, err := s.GetMeta()
	if err != nil {
		return suggestionMemoryState{}, err
	}
	task, err := s.GetTask(taskID)
	if err != nil {
		return suggestionMemoryState{}, err
	}
	notes, err := s.ListNotes()
	if err != nil {
		return suggestionMemoryState{}, err
	}
	issues, err := s.ListIssues()
	if err != nil {
		return suggestionMemoryState{}, err
	}
	return suggestionMemoryState{
		Goal: meta.Goal, Summary: meta.Summary, ActivePlan: meta.ActivePlan,
		Task: task, Notes: notes, Issues: issues,
	}, nil
}

func TestGetAgentIntelligenceV2RejectsStaleGenerationAndUnknownRun(t *testing.T) {
	app, _ := newTerminalBindingTestApp(t, &fakeGUITerminalManager{}, nil)
	registry := agentrun.NewRegistry(agentrun.Config{ProjectRoot: app.workspace.root})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	app.workspace.agents = registry
	if _, err := app.GetAgentIntelligenceV2(2, "missing"); !errors.Is(err, errStaleWorkspaceGeneration) {
		t.Fatalf("stale generation error = %v", err)
	}
	if _, err := app.GetAgentIntelligenceV2(1, "missing"); !errors.Is(err, agentrun.ErrRunNotFound) {
		t.Fatalf("unknown run error = %v", err)
	}
}

func TestBuildAgentSuggestionsScopesIssuesToCurrentAssociation(t *testing.T) {
	storage := suggestionScopeStore{
		plans: map[uint64]model.Plan{
			1: {ID: 1, Title: "one"},
			2: {ID: 2, Title: "two"},
		},
		tasks: map[uint64]model.Task{
			11: {ID: 11, PlanID: 1, Title: "one-task"},
			22: {ID: 22, PlanID: 2, Title: "two-task"},
		},
		issues: []model.Issue{
			{ID: 1, Title: "project", Status: model.IssueOpen},
			{ID: 2, Title: "plan one", Status: model.IssueOpen, TaskID: 11},
			{ID: 3, Title: "plan two", Status: model.IssueOpen, TaskID: 22},
		},
	}
	tests := []struct {
		name        string
		association *RuntimeAssociation
		wantIssueID uint64
	}{
		{name: "project", wantIssueID: 1},
		{name: "plan", association: &RuntimeAssociation{PlanID: 1, Revision: 1}, wantIssueID: 2},
		{name: "task", association: &RuntimeAssociation{PlanID: 1, TaskID: 11, Revision: 1}, wantIssueID: 2},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			suggestions, _, err := buildAgentSuggestions(
				storage,
				agentrun.Run{ID: "run-1", ProjectRoot: "/project"},
				test.association,
				nil,
			)
			if err != nil {
				t.Fatal(err)
			}
			issueIDs := []uint64{}
			for _, suggestion := range suggestions {
				if suggestion.Kind == AgentSuggestionIssue {
					issueIDs = append(issueIDs, suggestion.TargetID)
				}
			}
			if !reflect.DeepEqual(issueIDs, []uint64{test.wantIssueID}) {
				t.Fatalf("issue suggestions = %v, want [%d]", issueIDs, test.wantIssueID)
			}
		})
	}
}

func TestTaskIntelligenceRejectsSameGenerationReassociation(t *testing.T) {
	run := AgentRuntimeSummary{
		RunID:       "run-1",
		Association: &RuntimeAssociation{PlanID: 1, TaskID: 11, Revision: 3},
	}
	current := AgentIntelligenceV2{
		RunID:       "run-1",
		Association: &RuntimeAssociation{PlanID: 1, TaskID: 11, Revision: 3},
	}
	if !agentIntelligenceMatchesTaskSnapshot(run, current, 11) {
		t.Fatal("exact task association snapshot was rejected")
	}
	for _, changed := range []*RuntimeAssociation{
		nil,
		{PlanID: 1, TaskID: 22, Revision: 4},
		{PlanID: 1, TaskID: 11, Revision: 4},
		{PlanID: 2, TaskID: 11, Revision: 3},
	} {
		candidate := current
		candidate.Association = changed
		if agentIntelligenceMatchesTaskSnapshot(run, candidate, 11) {
			t.Fatalf("reassociated intelligence was accepted: %#v", changed)
		}
	}
}

type suggestionScopeStore struct {
	plans  map[uint64]model.Plan
	tasks  map[uint64]model.Task
	issues []model.Issue
}

func (s suggestionScopeStore) GetPlan(id uint64) (model.Plan, error) {
	plan, ok := s.plans[id]
	if !ok {
		return model.Plan{}, errors.New("plan missing")
	}
	return plan, nil
}

func (s suggestionScopeStore) GetTask(id uint64) (model.Task, error) {
	task, ok := s.tasks[id]
	if !ok {
		return model.Task{}, errors.New("task missing")
	}
	return task, nil
}

func (s suggestionScopeStore) RecentNotes(int) ([]model.Note, error) {
	return []model.Note{}, nil
}

func (s suggestionScopeStore) ListIssues() ([]model.Issue, error) {
	return append([]model.Issue{}, s.issues...), nil
}
