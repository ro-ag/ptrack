package gui

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
	"github.com/ro-ag/ptrack/internal/terminal"
)

type linkedLaunchFixture struct {
	app      *App
	manager  *fakeGUITerminalManager
	registry *agentrun.Registry
	root     string
	planID   uint64
	taskID   uint64
}

func newLinkedLaunchFixture(t *testing.T) linkedLaunchFixture {
	t.Helper()
	manager := &fakeGUITerminalManager{
		profiles: []terminal.Profile{
			{ID: "shell-default", Name: "Shell", Kind: terminal.ProfileShell},
			{ID: "agent-alpha", Name: "Alpha", Kind: terminal.ProfileAgent, Provider: "alpha"},
			{ID: "agent-beta", Name: "Beta", Kind: terminal.ProfileAgent, Provider: "beta"},
		},
	}
	app, root := newTerminalBindingTestApp(t, manager, nil)
	canonicalRoot, err := filepath.EvalSymlinks(root)
	if err != nil {
		t.Fatal(err)
	}
	manager.createResult = managedTerminalSession{
		SessionID:   "linked-session",
		ProfileID:   "agent-beta",
		ProfileKind: terminal.ProfileAgent,
		Provider:    "beta",
		PID:         4242,
		CWD:         canonicalRoot,
		State:       terminal.SessionRunning,
		StreamURL:   "ws://127.0.0.1/linked-session?token=opaque",
	}
	s, err := store.Open(filepath.Join(root, ".ptrack", "ptrack.db"))
	if err != nil {
		t.Fatal(err)
	}
	if err := s.SetGoal("Ship $(touch must-not-run) as inert project data"); err != nil {
		t.Fatal(err)
	}
	plan, err := s.AddPlan("Linked plan")
	if err != nil {
		t.Fatal(err)
	}
	task, err := s.AddTask(plan.ID, "Linked task")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.AddNote(model.TargetTask, task.ID, "durable task decision"); err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	registry := agentrun.NewRegistry(agentrun.Config{ProjectRoot: canonicalRoot})
	app.workspace.agents = registry
	t.Cleanup(func() {
		ctx, cancel := context.WithTimeout(context.Background(), time.Second)
		defer cancel()
		_ = registry.Shutdown(ctx)
	})
	return linkedLaunchFixture{
		app: app, manager: manager, registry: registry, root: canonicalRoot,
		planID: plan.ID, taskID: task.ID,
	}
}

func (f linkedLaunchFixture) taskPointer() association.PointerV1 {
	return association.PointerV1{
		Version: association.VersionV1,
		PlanID:  f.planID,
		TaskID:  f.taskID,
	}
}

func TestLaunchLinkedAgentUsesExactInstalledProfileContextAndSharedAssociation(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	marker := filepath.Join(fixture.root, "must-not-run")
	broker := &fakeWorkspaceCapabilityBroker{token: "host-minted-token"}
	fixture.app.workspace.capabilities = broker

	result, err := fixture.app.LaunchLinkedAgentV2(
		1,
		"agent-beta",
		"",
		31,
		111,
		fixture.taskPointer(),
	)
	if err != nil {
		t.Fatal(err)
	}
	call := fixture.manager.lastCreate()
	if call.profileID != "agent-beta" || call.cwd != fixture.root ||
		call.rows != 31 || call.columns != 111 {
		t.Fatalf("linked create = %#v", call)
	}
	if result.SessionID != "linked-session" || result.ProfileID != "agent-beta" ||
		result.Generation != 1 || result.AssociationRevision != 1 ||
		!result.LinkedLaunch {
		t.Fatalf("linked result = %#v", result)
	}
	if !reflect.DeepEqual(broker.issuedProfiles, []string{"agent-beta"}) ||
		broker.boundSession != "linked-session" {
		t.Fatalf("capability binding = issued %v bound %q", broker.issuedProfiles, broker.boundSession)
	}
	wantEnvironmentKeys := []string{
		LinkedLaunchContextEnvironment,
		"PTRACK_CAPABILITY_GENERATION",
		"PTRACK_CAPABILITY_PROFILE",
		"PTRACK_CAPABILITY_PROJECT",
		"PTRACK_CAPABILITY_TOKEN",
	}
	gotEnvironmentKeys := make([]string, 0, len(call.environment))
	for key := range call.environment {
		gotEnvironmentKeys = append(gotEnvironmentKeys, key)
	}
	for _, want := range wantEnvironmentKeys {
		if _, ok := call.environment[want]; !ok {
			t.Fatalf("linked environment missing %q: %#v", want, call.environment)
		}
	}
	if len(gotEnvironmentKeys) != len(wantEnvironmentKeys) {
		t.Fatalf("linked environment grants unexpected data: %#v", call.environment)
	}
	var document struct {
		Notice string `json:"notice"`
		Goal   string `json:"goal"`
		Plan   *struct {
			ID uint64 `json:"id"`
		} `json:"plan"`
		Task *struct {
			ID uint64 `json:"id"`
		} `json:"task"`
	}
	if err := json.Unmarshal([]byte(call.environment[LinkedLaunchContextEnvironment]), &document); err != nil {
		t.Fatalf("decode linked context: %v", err)
	}
	if !strings.Contains(document.Notice, "UNTRUSTED") ||
		document.Plan == nil || document.Plan.ID != fixture.planID ||
		document.Task == nil || document.Task.ID != fixture.taskID ||
		!strings.Contains(document.Goal, "$(touch must-not-run)") {
		t.Fatalf("linked context document = %#v", document)
	}
	if _, err := os.Stat(marker); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("context data caused a filesystem effect: %v", err)
	}

	terminalAssociation := fixture.manager.association
	runs := fixture.registry.Snapshot(8)
	if terminalAssociation == nil || len(runs) != 1 || runs[0].Association == nil {
		t.Fatalf("linked associations = terminal %#v runs %#v", terminalAssociation, runs)
	}
	if terminalAssociation.Target != runs[0].Association.Target ||
		terminalAssociation.Revision != 1 || runs[0].Association.Revision != 1 ||
		terminalAssociation.Generation != 1 || runs[0].Association.Generation != 1 {
		t.Fatalf("terminal/run association mismatch = %#v / %#v", terminalAssociation, runs[0].Association)
	}
}

func TestLinkedLaunchTelemetryReachesTaskIntelligenceAndHandoff(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	fixture.manager.profiles[2].Provider = "codex"
	fixture.manager.createResult.Provider = "codex"
	server, err := agentrun.StartIntegrationServer(fixture.registry, agentrun.IntegrationConfig{
		GlobalHome: t.TempDir(), ProjectRoot: fixture.root, Generation: 1,
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = server.Shutdown(context.Background()) })
	fixture.app.workspace.agents = &workspaceAgentResources{
		registry: fixture.registry, integration: server,
		root: fixture.root, globalHome: t.TempDir(),
	}

	if _, err := fixture.app.LaunchLinkedAgentV2(
		1, "agent-beta", "", 24, 80, fixture.taskPointer(),
	); err != nil {
		t.Fatal(err)
	}
	launch := fixture.manager.lastCreate()
	endpoint := launch.environment[AgentEventEndpointEnvironment]
	token := launch.environment[AgentEventTokenEnvironment]
	if endpoint != server.EventEndpoint() || token == "" {
		t.Fatalf("launched event environment = %#v", launch.environment)
	}
	payload, err := json.Marshal(agentrun.ProviderEvent{
		ModelVersion: agentrun.ProviderEventModelVersion,
		ID:           "file-1", Sequence: 1, Type: "item.completed",
		Category: agentrun.EventFile, Paths: []string{"internal/agentrun/event.go"},
	})
	if err != nil {
		t.Fatal(err)
	}
	request, err := http.NewRequest(http.MethodPost, endpoint, bytes.NewReader(payload))
	if err != nil {
		t.Fatal(err)
	}
	request.Header.Set("Authorization", "Bearer "+token)
	request.Header.Set("Content-Type", "application/json")
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatal(err)
	}
	if response.StatusCode != http.StatusCreated {
		body, _ := io.ReadAll(response.Body)
		t.Fatalf("launched event status = %d: %s", response.StatusCode, body)
	}
	_ = response.Body.Close()

	detail, err := fixture.app.GetTaskDetailV2(1, fixture.taskID)
	if err != nil {
		t.Fatal(err)
	}
	if len(detail.AgentIntelligence) != 1 ||
		detail.AgentIntelligence[0].EventBounds.Total != 1 ||
		detail.AgentIntelligence[0].Association == nil ||
		detail.AgentIntelligence[0].Association.TaskID != fixture.taskID {
		t.Fatalf("task intelligence = %#v", detail.AgentIntelligence)
	}
	foundFile := false
	for _, suggestion := range detail.AgentIntelligence[0].Suggestions {
		foundFile = foundFile || suggestion.Path == "internal/agentrun/event.go"
	}
	if !foundFile {
		t.Fatalf("task intelligence lacks launched file evidence: %#v", detail.AgentIntelligence[0])
	}
	encodedDetail, err := json.Marshal(detail)
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(encodedDetail, []byte(token)) {
		t.Fatal("task detail exposed the launched event token")
	}
	handoff, err := fixture.app.PreviewAgentHandoffV2(
		1,
		detail.AgentIntelligence[0].RunID,
	)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(handoff.Preview.Text, "internal/agentrun/event.go") {
		t.Fatalf("launched handoff = %#v", handoff)
	}
	if err := fixture.app.CloseTerminalV2(1, "linked-session", true); err != nil {
		t.Fatal(err)
	}
	revokedRequest, err := http.NewRequest(http.MethodPost, endpoint, bytes.NewReader(payload))
	if err != nil {
		t.Fatal(err)
	}
	revokedRequest.Header.Set("Authorization", "Bearer "+token)
	revokedResponse, err := http.DefaultClient.Do(revokedRequest)
	if err != nil {
		t.Fatal(err)
	}
	if revokedResponse.StatusCode != http.StatusUnauthorized {
		t.Fatalf("event token after terminal close status = %d", revokedResponse.StatusCode)
	}
	_ = revokedResponse.Body.Close()
}

func TestOrdinaryAgentTelemetryWaitsForBindingAndReachesOverview(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	fixture.manager.profiles[2].Provider = "codex"
	fixture.manager.createResult.Provider = "codex"
	server, err := agentrun.StartIntegrationServer(fixture.registry, agentrun.IntegrationConfig{
		GlobalHome: t.TempDir(), ProjectRoot: fixture.root, Generation: 1,
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = server.Shutdown(context.Background()) })
	fixture.app.workspace.agents = &workspaceAgentResources{
		registry: fixture.registry, integration: server,
		root: fixture.root, globalHome: t.TempDir(),
	}
	payload, err := json.Marshal(agentrun.ProviderEvent{
		ModelVersion: agentrun.ProviderEventModelVersion,
		ID:           "startup-file-1", Sequence: 1, Type: "item.completed",
		Category: agentrun.EventFile, Paths: []string{"internal/gui/terminal.go"},
	})
	if err != nil {
		t.Fatal(err)
	}
	type postResult struct {
		response *http.Response
		err      error
	}
	posted := make(chan postResult, 1)
	fixture.manager.createWithEnvHook = func(environment map[string]string) {
		endpoint := environment[AgentEventEndpointEnvironment]
		token := environment[AgentEventTokenEnvironment]
		started := make(chan struct{})
		go func() {
			request, requestErr := http.NewRequest(http.MethodPost, endpoint, bytes.NewReader(payload))
			if requestErr != nil {
				posted <- postResult{err: requestErr}
				return
			}
			request.Header.Set("Authorization", "Bearer "+token)
			request.Header.Set("Content-Type", "application/json")
			close(started)
			response, requestErr := (&http.Client{Timeout: 3 * time.Second}).Do(request)
			posted <- postResult{response: response, err: requestErr}
		}()
		<-started
	}
	launched, err := fixture.app.CreateTerminalV2(1, "agent-beta", "", 24, 80)
	if err != nil {
		t.Fatal(err)
	}
	result := <-posted
	if result.err != nil {
		t.Fatal(result.err)
	}
	if result.response.StatusCode != http.StatusCreated {
		body, _ := io.ReadAll(result.response.Body)
		t.Fatalf("startup event status = %d: %s", result.response.StatusCode, body)
	}
	_ = result.response.Body.Close()

	runs := fixture.registry.Snapshot(10)
	if len(runs) != 1 || runs[0].TerminalID != launched.SessionID {
		t.Fatalf("ordinary launched runs = %#v", runs)
	}
	events, total, err := fixture.registry.EventSnapshot(runs[0].ID, 10)
	if err != nil || total != 1 || len(events) != 1 ||
		events[0].Correlation.TerminalID != launched.SessionID {
		t.Fatalf("ordinary event correlation = %#v total=%d err=%v", events, total, err)
	}
	snapshot, err := fixture.app.GetWorkspaceSnapshot(1, 0)
	if err != nil {
		t.Fatal(err)
	}
	if len(snapshot.AgentRuns.Runs) != 1 || snapshot.AgentRuns.Runs[0].Intelligence == nil ||
		snapshot.AgentRuns.Runs[0].Intelligence.EventCount != 1 {
		t.Fatalf("overview intelligence = %#v", snapshot.AgentRuns)
	}
	launchEnvironment := fixture.manager.lastCreate().environment
	if err := fixture.app.CloseTerminalV2(1, launched.SessionID, true); err != nil {
		t.Fatal(err)
	}
	revokedRequest, err := http.NewRequest(
		http.MethodPost,
		launchEnvironment[AgentEventEndpointEnvironment],
		bytes.NewReader(payload),
	)
	if err != nil {
		t.Fatal(err)
	}
	revokedRequest.Header.Set(
		"Authorization",
		"Bearer "+launchEnvironment[AgentEventTokenEnvironment],
	)
	revokedResponse, err := http.DefaultClient.Do(revokedRequest)
	if err != nil {
		t.Fatal(err)
	}
	if revokedResponse.StatusCode != http.StatusUnauthorized {
		t.Fatalf("ordinary token after close status = %d", revokedResponse.StatusCode)
	}
	_ = revokedResponse.Body.Close()
}

func TestLaunchLinkedAgentAcceptsPlanOnlyPointer(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	if _, err := fixture.app.LaunchLinkedAgentV2(
		1,
		"agent-beta",
		"",
		24,
		80,
		association.PointerV1{Version: association.VersionV1, PlanID: fixture.planID},
	); err != nil {
		t.Fatal(err)
	}
	if fixture.manager.association == nil ||
		fixture.manager.association.Target.PlanID != fixture.planID ||
		fixture.manager.association.Target.TaskID != 0 {
		t.Fatalf("plan association = %#v", fixture.manager.association)
	}
}

func TestLaunchLinkedAgentRejectsUnavailableShellStaleOutsideAndMismatchedTargetsBeforeSpawn(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	broker := &fakeWorkspaceCapabilityBroker{token: "must-not-issue"}
	fixture.app.workspace.capabilities = broker
	outside := t.TempDir()
	symlinkOutside := filepath.Join(fixture.root, "outside-link")
	if err := os.Symlink(outside, symlinkOutside); err != nil {
		t.Fatal(err)
	}
	s, err := store.Open(filepath.Join(fixture.root, ".ptrack", "ptrack.db"))
	if err != nil {
		t.Fatal(err)
	}
	otherPlan, err := s.AddPlan("Other")
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}

	tests := []struct {
		name       string
		generation uint64
		profile    string
		cwd        string
		pointer    association.PointerV1
	}{
		{name: "shell profile", generation: 1, profile: "shell-default", pointer: fixture.taskPointer()},
		{name: "unavailable profile", generation: 1, profile: "agent-missing", pointer: fixture.taskPointer()},
		{name: "inexact profile", generation: 1, profile: " agent-beta", pointer: fixture.taskPointer()},
		{name: "stale generation", generation: 2, profile: "agent-beta", pointer: fixture.taskPointer()},
		{name: "outside CWD", generation: 1, profile: "agent-beta", cwd: outside, pointer: fixture.taskPointer()},
		{name: "symlink escape", generation: 1, profile: "agent-beta", cwd: symlinkOutside, pointer: fixture.taskPointer()},
		{
			name: "association mismatch", generation: 1, profile: "agent-beta",
			pointer: association.PointerV1{
				Version: association.VersionV1,
				PlanID:  otherPlan.ID,
				TaskID:  fixture.taskID,
			},
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if _, err := fixture.app.LaunchLinkedAgentV2(
				test.generation, test.profile, test.cwd, 24, 80, test.pointer,
			); err == nil {
				t.Fatal("linked launch succeeded")
			}
		})
	}
	if len(fixture.manager.creates) != 0 || len(broker.issuedProfiles) != 0 ||
		len(fixture.registry.Snapshot(8)) != 0 {
		t.Fatalf("rejected launches created authority: creates %d issues %d runs %d",
			len(fixture.manager.creates), len(broker.issuedProfiles), len(fixture.registry.Snapshot(8)))
	}
}

func TestLaunchLinkedAgentRevokesBeforeForcedCleanupOnPostSpawnFailures(t *testing.T) {
	tests := []struct {
		name      string
		configure func(*linkedLaunchFixture, *fakeWorkspaceCapabilityBroker, *[]string)
	}{
		{
			name: "capability bind",
			configure: func(_ *linkedLaunchFixture, broker *fakeWorkspaceCapabilityBroker, _ *[]string) {
				broker.bindErr = errors.New("bind failed")
			},
		},
		{
			name: "terminal association",
			configure: func(fixture *linkedLaunchFixture, _ *fakeWorkspaceCapabilityBroker, _ *[]string) {
				fixture.manager.associationErr = errors.New("associate failed")
			},
		},
		{
			name: "AgentRun registration",
			configure: func(fixture *linkedLaunchFixture, _ *fakeWorkspaceCapabilityBroker, _ *[]string) {
				full := agentrun.NewRegistry(agentrun.Config{
					ProjectRoot: fixture.root,
					MaxRecords:  1,
				})
				if _, err := full.RegisterLaunched(agentrun.Registration{
					Profile: "agent-alpha", Provider: "alpha", PID: 7,
					TerminalID: "existing", CWD: fixture.root,
				}); err != nil {
					t.Fatalf("fill registry: %v", err)
				}
				fixture.app.workspace.agents = full
				t.Cleanup(func() {
					ctx, cancel := context.WithTimeout(context.Background(), time.Second)
					defer cancel()
					_ = full.Shutdown(ctx)
				})
			},
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			fixture := newLinkedLaunchFixture(t)
			order := []string{}
			broker := &fakeWorkspaceCapabilityBroker{
				token: "host-token",
				revokeTokenHook: func(string) {
					order = append(order, "revoke-token")
				},
				revokeSessionHook: func(string) {
					order = append(order, "revoke-session")
				},
			}
			fixture.manager.closeHook = func(string, bool) { order = append(order, "close") }
			fixture.app.workspace.capabilities = broker
			test.configure(&fixture, broker, &order)

			if _, err := fixture.app.LaunchLinkedAgentV2(
				1, "agent-beta", "", 24, 80, fixture.taskPointer(),
			); err == nil {
				t.Fatal("linked launch succeeded")
			}
			if len(order) != 2 || order[1] != "close" ||
				(order[0] != "revoke-token" && order[0] != "revoke-session") {
				t.Fatalf("cleanup order = %v, want revoke before close", order)
			}
			if closeCall := fixture.manager.lastClose(); closeCall.sessionID != "linked-session" || !closeCall.force {
				t.Fatalf("forced cleanup = %#v", closeCall)
			}
			if got := fixture.app.workspace.activeResourceSummary().Terminals; got != 0 {
				t.Fatalf("failed linked launch left %d terminal records", got)
			}
		})
	}
}

func TestLaunchLinkedAgentRevokesUnboundTokenWhenSpawnFails(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	spawnErr := errors.New("spawn failed")
	fixture.manager.createErrors = map[string]error{"agent-beta": spawnErr}
	broker := &fakeWorkspaceCapabilityBroker{token: "unbound-token"}
	fixture.app.workspace.capabilities = broker

	if _, err := fixture.app.LaunchLinkedAgentV2(
		1, "agent-beta", "", 24, 80, fixture.taskPointer(),
	); !errors.Is(err, spawnErr) {
		t.Fatalf("spawn error = %v", err)
	}
	if !reflect.DeepEqual(broker.revokedTokens, []string{"unbound-token"}) ||
		len(broker.revokedSessions) != 0 || len(fixture.manager.closes) != 0 ||
		len(fixture.registry.Snapshot(8)) != 0 {
		t.Fatalf("spawn cleanup = tokens %v sessions %v closes %v runs %v",
			broker.revokedTokens, broker.revokedSessions,
			fixture.manager.closes, fixture.registry.Snapshot(8))
	}
}

func TestRollbackLinkedAgentLaunchRemovesRunAndRevokesBeforeForceClose(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	order := []string{}
	broker := &fakeWorkspaceCapabilityBroker{
		token: "bound-token",
		revokeSessionHook: func(string) {
			order = append(order, "revoke")
		},
	}
	fixture.manager.closeHook = func(string, bool) { order = append(order, "close") }
	fixture.app.workspace.capabilities = broker
	result, err := fixture.app.LaunchLinkedAgentV2(
		1, "agent-beta", "", 24, 80, fixture.taskPointer(),
	)
	if err != nil {
		t.Fatal(err)
	}
	if len(fixture.registry.Snapshot(8)) != 1 {
		t.Fatal("linked run was not registered")
	}
	if err := fixture.app.RollbackLinkedAgentLaunchV2(1, result.SessionID); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(order, []string{"revoke", "close"}) {
		t.Fatalf("rollback order = %v", order)
	}
	if len(fixture.registry.Snapshot(8)) != 0 ||
		fixture.app.workspace.activeResourceSummary().Terminals != 0 {
		t.Fatalf("rollback left resources = runs %v summary %#v",
			fixture.registry.Snapshot(8), fixture.app.workspace.activeResourceSummary())
	}
	if closeCall := fixture.manager.lastClose(); !closeCall.force ||
		closeCall.sessionID != result.SessionID {
		t.Fatalf("rollback close = %#v", closeCall)
	}
}

func TestRollbackLinkedAgentLaunchRejectsOrdinaryTerminal(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	broker := &fakeWorkspaceCapabilityBroker{token: "ordinary-token"}
	fixture.app.workspace.capabilities = broker
	run, err := fixture.registry.RegisterLaunched(agentrun.Registration{
		Profile:    "agent-beta",
		Provider:   "beta",
		PID:        5252,
		TerminalID: "ordinary-session",
		CWD:        fixture.root,
	})
	if err != nil {
		t.Fatal(err)
	}
	associated, err := fixture.app.AssociateAgentRunV2(
		1,
		run.ID,
		fixture.taskPointer(),
	)
	if err != nil || associated.Target.TaskID != fixture.taskID {
		t.Fatalf("associate ordinary run: %#v, %v", associated, err)
	}

	if err := fixture.app.RollbackLinkedAgentLaunchV2(1, "ordinary-session"); err == nil {
		t.Fatal("ordinary terminal was accepted by linked rollback")
	}
	if len(fixture.manager.closes) != 0 || len(broker.revokedSessions) != 0 {
		t.Fatalf("ordinary terminal was mutated: closes %v revocations %v",
			fixture.manager.closes, broker.revokedSessions)
	}
	runs := fixture.registry.Snapshot(8)
	if len(runs) != 1 || runs[0].ID != run.ID || runs[0].Association == nil {
		t.Fatalf("ordinary run history changed: %#v", runs)
	}
}

func TestRollbackLinkedAgentLaunchKeepsProvenanceUntilCloseSucceeds(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	broker := &fakeWorkspaceCapabilityBroker{token: "bound-token"}
	fixture.app.workspace.capabilities = broker
	result, err := fixture.app.LaunchLinkedAgentV2(
		1, "agent-beta", "", 24, 80, fixture.taskPointer(),
	)
	if err != nil {
		t.Fatal(err)
	}
	closeErr := errors.New("close failed")
	fixture.manager.closeErrors = map[string]error{result.SessionID: closeErr}
	if err := fixture.app.RollbackLinkedAgentLaunchV2(1, result.SessionID); !errors.Is(err, closeErr) {
		t.Fatalf("rollback close error = %v, want %v", err, closeErr)
	}
	if len(fixture.registry.Snapshot(8)) != 1 ||
		!fixture.registry.HasLinkedTerminal(result.SessionID) ||
		fixture.app.workspace.activeResourceSummary().Terminals != 1 {
		t.Fatalf("failed rollback consumed retry state: runs %#v summary %#v",
			fixture.registry.Snapshot(8), fixture.app.workspace.activeResourceSummary())
	}
	delete(fixture.manager.closeErrors, result.SessionID)
	if err := fixture.app.RollbackLinkedAgentLaunchV2(1, result.SessionID); err != nil {
		t.Fatal(err)
	}
	if len(fixture.registry.Snapshot(8)) != 0 ||
		fixture.app.workspace.activeResourceSummary().Terminals != 0 {
		t.Fatalf("retry left resources: runs %#v summary %#v",
			fixture.registry.Snapshot(8), fixture.app.workspace.activeResourceSummary())
	}
}
