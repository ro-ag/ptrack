package gui

import (
	"context"
	"encoding/json"
	"fmt"
	"reflect"
	"slices"
	"strings"
	"testing"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/store"
	"github.com/ro-ag/ptrack/internal/terminal"
)

type runtimeTestTerminalManager struct {
	*fakeGUITerminalManager
	sessions []terminal.SessionInfo
}

func (m *runtimeTestTerminalManager) SessionSnapshot(limit int) []terminal.SessionInfo {
	limit = min(limit, len(m.sessions))
	return append([]terminal.SessionInfo{}, m.sessions[:limit]...)
}

func runtimeProjectionFixture(
	t *testing.T,
) (string, *store.Store, *association.Host, uint64, uint64, uint64) {
	t.Helper()
	root := t.TempDir()
	s, err := store.Open(root + "/ptrack.db")
	if err != nil {
		t.Fatal(err)
	}
	plan, err := s.AddPlan("Runtime plan")
	if err != nil {
		t.Fatal(err)
	}
	task, err := s.AddTask(plan.ID, "Runtime task")
	if err != nil {
		t.Fatal(err)
	}
	otherPlan, err := s.AddPlan("Other plan")
	if err != nil {
		t.Fatal(err)
	}
	otherTask, err := s.AddTask(otherPlan.ID, "Other task")
	if err != nil {
		t.Fatal(err)
	}
	host, err := association.NewHost(root, 7, storeAssociationCatalog{store: s})
	if err != nil {
		t.Fatal(err)
	}
	return host.ProjectRoot(), s, host, plan.ID, task.ID, otherTask.ID
}

func bindRuntime(
	t *testing.T,
	host *association.Host,
	liveID string,
	planID, taskID uint64,
) *association.AssociationV1 {
	t.Helper()
	bound, err := host.Bind(liveID, association.PointerV1{
		Version: association.VersionV1,
		PlanID:  planID,
		TaskID:  taskID,
	}, nil)
	if err != nil {
		t.Fatal(err)
	}
	return &bound
}

func TestRuntimeProjectionUsesOnlyCurrentValidatedAssociationsAndNoContent(t *testing.T) {
	root, s, host, planID, taskID, otherTaskID := runtimeProjectionFixture(t)
	defer s.Close()
	currentTerminal := bindRuntime(t, host, "terminal-current", planID, taskID)
	planTerminal := bindRuntime(t, host, "terminal-plan", planID, 0)
	exitedTerminal := bindRuntime(t, host, "terminal-exited", planID, taskID)
	wrongProject := *currentTerminal
	wrongProject.LiveID = "terminal-wrong-project"
	wrongProject.ProjectRoot = root + "-other"
	staleGeneration := *currentTerminal
	staleGeneration.LiveID = "terminal-stale"
	staleGeneration.Generation--
	wrongLiveID := *currentTerminal
	wrongLiveID.LiveID = "another-terminal"
	invalidOwnership := *currentTerminal
	invalidOwnership.LiveID = "terminal-invalid-task"
	invalidOwnership.Target.TaskID = otherTaskID

	sessions := []terminal.SessionInfo{
		{ID: "terminal-current", ProfileID: "TERMINAL_PROFILE_CONTENT_CANARY", ProfileKind: terminal.ProfileAgent, CWD: root + "/TERMINAL_CWD_SECRET_CANARY", State: terminal.SessionRunning, Association: currentTerminal},
		{ID: "terminal-plan", ProfileID: "agent-b", ProfileKind: terminal.ProfileAgent, State: terminal.SessionRunning, Association: planTerminal},
		{ID: "terminal-exited", ProfileID: "agent-a", ProfileKind: terminal.ProfileAgent, State: terminal.SessionExited, Association: exitedTerminal},
		{ID: "terminal-wrong-project", ProfileID: "agent-a", ProfileKind: terminal.ProfileAgent, State: terminal.SessionRunning, Association: &wrongProject},
		{ID: "terminal-stale", ProfileID: "agent-a", ProfileKind: terminal.ProfileAgent, State: terminal.SessionRunning, Association: &staleGeneration},
		{ID: "terminal-wrong-live", ProfileID: "agent-a", ProfileKind: terminal.ProfileAgent, State: terminal.SessionRunning, Association: &wrongLiveID},
		{ID: "terminal-invalid-task", ProfileID: "agent-a", ProfileKind: terminal.ProfileAgent, State: terminal.SessionRunning, Association: &invalidOwnership},
	}

	pairedRun := bindRuntime(t, host, "run-paired", planID, taskID)
	mismatchedRun := *bindRuntime(t, host, "run-mismatch", planID, taskID)
	mismatchedRun.Revision++
	externalRun := bindRuntime(t, host, "run-external", planID, taskID)
	staleRun := bindRuntime(t, host, "run-stale", planID, taskID)
	exitedRun := bindRuntime(t, host, "run-exited", planID, taskID)
	planRun := bindRuntime(t, host, "run-plan", planID, 0)
	wrongProjectRun := *externalRun
	wrongProjectRun.LiveID = "run-wrong-project"
	wrongProjectRun.ProjectRoot = root + "-other"
	runs := []agentrun.Run{
		{ID: "run-paired", Profile: "AGENT_PROFILE_CONTENT_CANARY", Provider: "AGENT_PROVIDER_CONTENT_CANARY", TerminalID: "terminal-current", CWD: root + "/AGENT_CWD_SECRET_CANARY", Kind: agentrun.RegistrationLaunched, State: agentrun.StateRunning, ProcessState: agentrun.ProcessRunning, LeaseState: agentrun.LeaseNone, Association: pairedRun},
		{ID: "run-mismatch", Profile: "agent-a", Provider: "a", TerminalID: "terminal-current", Kind: agentrun.RegistrationLaunched, State: agentrun.StateRunning, ProcessState: agentrun.ProcessRunning, LeaseState: agentrun.LeaseNone, Association: &mismatchedRun},
		{ID: "run-external", Profile: "external-a", Provider: "external", Kind: agentrun.RegistrationExternal, State: agentrun.StateRunning, ProcessState: agentrun.ProcessUnknown, LeaseState: agentrun.LeaseActive, Association: externalRun},
		{ID: "run-stale", Profile: "external-b", Provider: "external", Kind: agentrun.RegistrationExternal, State: agentrun.StateStale, ProcessState: agentrun.ProcessUnknown, LeaseState: agentrun.LeaseExpired, Association: staleRun},
		{ID: "run-exited", Profile: "external-c", Provider: "external", Kind: agentrun.RegistrationExternal, State: agentrun.StateExited, ProcessState: agentrun.ProcessExited, LeaseState: agentrun.LeaseExpired, Association: exitedRun, Exit: &agentrun.Exit{Result: "RAW_RESULT_SECRET_CANARY"}},
		{ID: "run-plan", Profile: "external-d", Provider: "external", Kind: agentrun.RegistrationExternal, State: agentrun.StateRunning, ProcessState: agentrun.ProcessUnknown, LeaseState: agentrun.LeaseActive, Association: planRun},
		{ID: "run-wrong-project", Profile: "external-f", Provider: "external", Kind: agentrun.RegistrationExternal, State: agentrun.StateRunning, ProcessState: agentrun.ProcessUnknown, LeaseState: agentrun.LeaseActive, Association: &wrongProjectRun},
		{ID: "run-unlinked", Profile: "external-e", Provider: "external", Kind: agentrun.RegistrationExternal, State: agentrun.StateRunning, ProcessState: agentrun.ProcessUnknown, LeaseState: agentrun.LeaseActive},
	}

	projection := buildRuntimeProjection(host, sessions, runs)
	detail := taskLinkedRuntime(projection, taskID)
	if detail.Summary.Terminals != 2 || detail.Summary.LiveTerminals != 1 ||
		detail.Summary.Agents != 5 || detail.Summary.LiveAgents != 3 ||
		detail.Summary.TerminalBackedRuns != 2 || detail.Summary.ExternalRuns != 3 {
		t.Fatalf("task runtime summary = %#v", detail.Summary)
	}
	paired := slices.IndexFunc(detail.Agents, func(run AgentRuntimeSummary) bool {
		return run.RunID == "run-paired"
	})
	if paired < 0 || !detail.Agents[paired].CorrespondingTerminal {
		t.Fatalf("paired terminal-backed run = %#v", detail.Agents)
	}
	mismatched := slices.IndexFunc(detail.Agents, func(run AgentRuntimeSummary) bool {
		return run.RunID == "run-mismatch"
	})
	if mismatched < 0 || !detail.Agents[mismatched].TerminalPresent ||
		detail.Agents[mismatched].CorrespondingTerminal {
		t.Fatalf("mismatched terminal-backed run was paired: %#v", detail.Agents)
	}
	planOnly := slices.IndexFunc(projection.terminals, func(session TerminalRuntimeSummary) bool {
		return session.SessionID == "terminal-plan"
	})
	if planOnly < 0 || projection.terminals[planOnly].Association == nil ||
		projection.terminals[planOnly].Association.TaskID != 0 {
		t.Fatalf("plan-only terminal missing from project runtime: %#v", projection.terminals)
	}
	for _, id := range []string{
		"terminal-wrong-project", "terminal-stale", "terminal-wrong-live", "terminal-invalid-task",
	} {
		index := slices.IndexFunc(projection.terminals, func(session TerminalRuntimeSummary) bool {
			return session.SessionID == id
		})
		if index < 0 || projection.terminals[index].Association != nil {
			t.Fatalf("invalid association contributed for %q: %#v", id, projection.terminals)
		}
	}
	wrongProjectAgent := slices.IndexFunc(
		projection.agents,
		func(run AgentRuntimeSummary) bool { return run.RunID == "run-wrong-project" },
	)
	if wrongProjectAgent < 0 || projection.agents[wrongProjectAgent].Association != nil {
		t.Fatalf("wrong-project AgentRun contributed: %#v", projection.agents)
	}

	encoded, err := json.Marshal(struct {
		Terminals []TerminalRuntimeSummary `json:"terminals"`
		Agents    []AgentRuntimeSummary    `json:"agents"`
	}{projection.terminals, projection.agents})
	if err != nil {
		t.Fatal(err)
	}
	for _, forbidden := range []string{
		"RAW_RESULT_SECRET_CANARY", "PROFILE_CONTENT_CANARY", "PROVIDER_CONTENT_CANARY",
		"CWD_SECRET_CANARY",
		"cwd", "pid", "token", "output", "prompt",
	} {
		if strings.Contains(strings.ToLower(string(encoded)), strings.ToLower(forbidden)) {
			t.Fatalf("content-free runtime JSON contains %q: %s", forbidden, encoded)
		}
	}
}

func TestRuntimeProjectionIsDeterministicAndBounded(t *testing.T) {
	_, s, host, planID, taskID, _ := runtimeProjectionFixture(t)
	defer s.Close()
	sessions := make([]terminal.SessionInfo, 0, linkedRuntimeEntryLimit+6)
	runs := make([]agentrun.Run, 0, linkedRuntimeEntryLimit+6)
	for index := 0; index < linkedRuntimeEntryLimit+6; index++ {
		sessionID := fmt.Sprintf("terminal-%02d", index)
		runID := fmt.Sprintf("run-%02d", index)
		sessions = append(sessions, terminal.SessionInfo{
			ID: sessionID, ProfileID: "shell", ProfileKind: terminal.ProfileShell,
			State:       terminal.SessionRunning,
			Association: bindRuntime(t, host, sessionID, planID, taskID),
		})
		runs = append(runs, agentrun.Run{
			ID: runID, Profile: "external", Provider: "test",
			Kind: agentrun.RegistrationExternal, State: agentrun.StateRunning,
			ProcessState: agentrun.ProcessUnknown, LeaseState: agentrun.LeaseActive,
			Association: bindRuntime(t, host, runID, planID, taskID),
		})
	}
	forward := buildRuntimeProjection(host, sessions, runs)
	slices.Reverse(sessions)
	slices.Reverse(runs)
	reverse := buildRuntimeProjection(host, sessions, runs)
	if !reflect.DeepEqual(forward, reverse) {
		t.Fatalf("runtime ordering depends on input order")
	}
	if len(forward.terminals) != linkedRuntimeEntryLimit ||
		len(forward.agents) != linkedRuntimeEntryLimit ||
		forward.terminalBounds.More != 6 || forward.agentBounds.More != 6 ||
		forward.terminals[0].SessionID != "terminal-00" ||
		forward.agents[0].RunID != "run-00" {
		t.Fatalf("runtime bounds/order = %#v", forward)
	}
	detail := taskLinkedRuntime(forward, taskID)
	if detail.Summary.Terminals != linkedRuntimeEntryLimit+6 ||
		detail.Summary.Agents != linkedRuntimeEntryLimit+6 ||
		len(detail.Terminals) != linkedRuntimeEntryLimit ||
		len(detail.Agents) != linkedRuntimeEntryLimit ||
		detail.TerminalRowsMore != 6 || detail.AgentRowsMore != 6 ||
		detail.Summary.Truncated {
		t.Fatalf("per-task aggregation was capped before summary: %#v", detail)
	}
	truncated := taskLinkedRuntime(runtimeProjection{sourcesTruncated: true}, taskID)
	if !truncated.Summary.Truncated {
		t.Fatalf("candidate truncation was not explicit: %#v", truncated)
	}
}

func TestWorkspaceAndTaskDetailShareLinkedRuntimeProjection(t *testing.T) {
	app := seedApp(t)
	s, err := store.Open(app.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	host, err := association.NewHost(
		app.workspace.root,
		app.workspace.Generation(),
		storeAssociationCatalog{store: s},
	)
	if err != nil {
		t.Fatal(err)
	}
	terminalAssociation := bindRuntime(t, host, "linked-terminal", 1, 1)
	planAssociation := bindRuntime(t, host, "plan-terminal", 1, 0)
	manager := &runtimeTestTerminalManager{
		fakeGUITerminalManager: &fakeGUITerminalManager{},
		sessions: []terminal.SessionInfo{
			{ID: "linked-terminal", ProfileID: "agent-a", ProfileKind: terminal.ProfileAgent, State: terminal.SessionRunning, Association: terminalAssociation},
			{ID: "plan-terminal", ProfileID: "agent-b", ProfileKind: terminal.ProfileAgent, State: terminal.SessionRunning, Association: planAssociation},
		},
	}
	registry := agentrun.NewRegistry(agentrun.Config{ProjectRoot: app.workspace.root})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	if _, err := registry.RegisterLinkedLaunched(agentrun.Registration{
		Profile: "agent-a", Provider: "a", PID: 42,
		TerminalID: "linked-terminal", CWD: app.workspace.root,
	}, host, association.PointerV1{Version: 1, PlanID: 1, TaskID: 1}); err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	app.workspace.terminals = manager
	app.workspace.agents = registry
	app.gitSnapshots = fakeGitSnapshotter{}

	snapshot, err := app.GetWorkspaceSnapshot(1, 1)
	if err != nil {
		t.Fatal(err)
	}
	card := snapshot.Tracking.Board.Columns[0].Tasks[0]
	if card.LinkedRuntime == nil || card.LinkedRuntime.Terminals != 1 ||
		card.LinkedRuntime.Agents != 1 || card.LinkedRuntime.LiveAgents != 1 {
		t.Fatalf("task card linked runtime = %#v", card.LinkedRuntime)
	}
	if len(snapshot.Terminals.Sessions) != 2 ||
		snapshot.Terminals.Sessions[0].Association.TaskID != 0 &&
			snapshot.Terminals.Sessions[1].Association.TaskID != 0 {
		t.Fatalf("plan-linked session missing from project intelligence: %#v", snapshot.Terminals)
	}
	detail, err := app.GetTaskDetailV2(1, 1)
	if err != nil {
		t.Fatal(err)
	}
	if detail.LinkedRuntime.Summary.Terminals != 1 ||
		detail.LinkedRuntime.Summary.Agents != 1 ||
		len(detail.LinkedRuntime.Terminals) != 1 ||
		len(detail.LinkedRuntime.Agents) != 1 ||
		!detail.LinkedRuntime.Agents[0].CorrespondingTerminal {
		t.Fatalf("task detail linked runtime = %#v", detail.LinkedRuntime)
	}
	otherDetail, err := app.GetTaskDetailV2(1, 2)
	if err != nil {
		t.Fatal(err)
	}
	if otherDetail.Task.LinkedRuntime != nil ||
		len(otherDetail.LinkedRuntime.Terminals) != 0 ||
		len(otherDetail.LinkedRuntime.Agents) != 0 {
		t.Fatalf("plan-only runtime attributed to task #2: %#v", otherDetail)
	}
}
