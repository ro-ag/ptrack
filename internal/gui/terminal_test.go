package gui

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/terminal"
)

func TestGetTerminalProfilesReturnsSafeMetadataAndPropagatesError(t *testing.T) {
	manager := &fakeGUITerminalManager{
		profiles: []terminal.Profile{{
			ID:         "shell-default",
			Name:       "Default shell",
			Kind:       terminal.ProfileShell,
			Executable: "/test/shell",
			Args:       []string{"-l"},
			Env:        map[string]string{"MODE": "safe"},
		}},
	}
	app, _ := newTerminalBindingTestApp(t, manager, nil)

	profiles, err := app.GetTerminalProfiles()
	if err != nil {
		t.Fatalf("GetTerminalProfiles: %v", err)
	}
	if len(profiles) != 1 ||
		profiles[0].ID != "shell-default" ||
		profiles[0].Name != "Default shell" ||
		profiles[0].Kind != terminal.ProfileShell {
		t.Fatalf("safe profile metadata = %#v", profiles)
	}
	if profiles[0].Executable != "" || len(profiles[0].Args) != 0 || len(profiles[0].Env) != 0 {
		t.Fatalf("profile exposes launch configuration: %#v", profiles[0])
	}

	profilesErr := errors.New("discover profiles")
	manager.profilesErr = profilesErr
	if _, err := app.GetTerminalProfiles(); !errors.Is(err, profilesErr) {
		t.Fatalf("GetTerminalProfiles error = %v, want %v", err, profilesErr)
	}
}

func TestGetTerminalProfilesOrdersShellDefaultBeforeAgentInput(t *testing.T) {
	manager := &fakeGUITerminalManager{
		profiles: []terminal.Profile{
			{ID: "agent-codex", Name: "Codex", Kind: terminal.ProfileAgent},
			{ID: "shell-z", Name: "Z shell", Kind: terminal.ProfileShell},
			{ID: "shell-default", Name: "Default shell", Kind: terminal.ProfileShell},
		},
	}
	app, _ := newTerminalBindingTestApp(t, manager, nil)
	profiles, err := app.GetTerminalProfiles()
	if err != nil {
		t.Fatalf("GetTerminalProfiles: %v", err)
	}
	got := make([]string, 0, len(profiles))
	for _, profile := range profiles {
		got = append(got, profile.ID)
	}
	want := []string{"shell-default", "shell-z", "agent-codex"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("safe profile order = %v, want %v", got, want)
	}
}

func TestCreateTerminalWaitsForStartupBeforeCreatingAndEmitting(t *testing.T) {
	manager := &fakeGUITerminalManager{
		createResult: managedTerminalSession{
			SessionID: "session",
			ProfileID: "shell-default",
			CWD:       "/test/project",
			State:     terminal.SessionRunning,
			StreamURL: "ws://127.0.0.1:49152/terminal/session?token=opaque",
		},
	}
	var emitted []emittedTerminalEvent
	var emitMu sync.Mutex
	emitter := terminalEventEmitter(func(ctx context.Context, name string, payload any) {
		emitMu.Lock()
		defer emitMu.Unlock()
		emitted = append(emitted, emittedTerminalEvent{ctx: ctx, name: name, payload: payload})
	})
	projectRoot := t.TempDir()
	if err := os.Mkdir(filepath.Join(projectRoot, ".ptrack"), 0o755); err != nil {
		t.Fatalf("create project metadata directory: %v", err)
	}
	app, err := newAppWithTerminal(
		filepath.Join(projectRoot, ".ptrack", "ptrack.db"),
		0,
		manager,
		emitter,
	)
	if err != nil {
		t.Fatalf("newAppWithTerminal: %v", err)
	}

	createDone := make(chan error, 1)
	go func() {
		_, createErr := app.CreateTerminal("shell-default", "", 24, 80)
		createDone <- createErr
	}()
	runtime.Gosched()
	manager.mu.Lock()
	createCallsBeforeStartup := len(manager.creates)
	manager.mu.Unlock()
	if createCallsBeforeStartup != 0 {
		t.Fatal("terminal was created before Wails startup completed")
	}

	startupContext := context.Background()
	app.onStartup(startupContext)
	if err := <-createDone; err != nil {
		t.Fatalf("CreateTerminal: %v", err)
	}
	emitMu.Lock()
	defer emitMu.Unlock()
	if len(emitted) != 1 || emitted[0].ctx != startupContext || emitted[0].name != "terminal:status" {
		t.Fatalf("events after startup = %#v", emitted)
	}
}

func TestCreateTerminalSelectsProfileDefaultsCanonicalCWDAndPreservesDimensionOrder(t *testing.T) {
	realRoot := t.TempDir()
	if err := os.Mkdir(filepath.Join(realRoot, ".ptrack"), 0o755); err != nil {
		t.Fatalf("create project metadata directory: %v", err)
	}
	aliasParent := t.TempDir()
	aliasRoot := filepath.Join(aliasParent, "project-link")
	if err := os.Symlink(realRoot, aliasRoot); err != nil {
		t.Fatalf("create project-root symlink: %v", err)
	}
	dbPath := filepath.Join(aliasRoot, ".ptrack", "ptrack.db")

	manager := &fakeGUITerminalManager{
		profiles: []terminal.Profile{{
			ID:         "agent-codex",
			Name:       "Codex",
			Kind:       terminal.ProfileAgent,
			Executable: "/test/codex",
		}},
		createResult: managedTerminalSession{
			SessionID: "opaque-session-id",
			ProfileID: "agent-codex",
			CWD:       realRoot,
			State:     terminal.SessionRunning,
			StreamURL: "ws://127.0.0.1:49152/terminal/opaque-session-id?token=opaque",
		},
	}
	app, err := newAppWithTerminal(dbPath, 0, manager, func(context.Context, string, any) {})
	if err != nil {
		t.Fatalf("newAppWithTerminal: %v", err)
	}
	app.onStartup(context.Background())
	canonicalRoot, err := filepath.EvalSymlinks(realRoot)
	if err != nil {
		t.Fatalf("canonicalize expected project root: %v", err)
	}

	got, err := app.CreateTerminal("agent-codex", "", 37, 119)
	if err != nil {
		t.Fatalf("CreateTerminal: %v", err)
	}
	call := manager.lastCreate()
	if call.profileID != "agent-codex" {
		t.Fatalf("selected profile = %q, want agent-codex", call.profileID)
	}
	if call.cwd != canonicalRoot {
		t.Fatalf("default CWD = %q, want canonical project root %q", call.cwd, canonicalRoot)
	}
	if call.rows != 37 || call.columns != 119 {
		t.Fatalf("dimensions = rows %d columns %d, want rows 37 columns 119", call.rows, call.columns)
	}
	if got.SessionID != manager.createResult.SessionID ||
		got.ProfileID != manager.createResult.ProfileID ||
		got.CWD != manager.createResult.CWD ||
		got.State != manager.createResult.State ||
		got.StreamURL != manager.createResult.StreamURL {
		t.Fatalf("CreateTerminal result = %#v, want manager metadata %#v", got, manager.createResult)
	}
}

func TestCreateTerminalPassesExplicitCWDAndRejectsInvalidProfile(t *testing.T) {
	manager := &fakeGUITerminalManager{
		profiles: []terminal.Profile{{
			ID:         "shell-default",
			Name:       "Default shell",
			Kind:       terminal.ProfileShell,
			Executable: "/test/shell",
		}},
		createResult: managedTerminalSession{
			SessionID: "session",
			ProfileID: "shell-default",
			State:     terminal.SessionRunning,
			StreamURL: "ws://127.0.0.1:49152/terminal/session?token=opaque",
		},
		createErrors: map[string]error{
			"missing-profile": errors.New("profile not found"),
		},
	}
	app, _ := newTerminalBindingTestApp(t, manager, nil)
	explicitCWD := t.TempDir()
	manager.createResult.CWD = explicitCWD

	if _, err := app.CreateTerminal("shell-default", explicitCWD, 24, 80); err != nil {
		t.Fatalf("CreateTerminal explicit CWD: %v", err)
	}
	if got := manager.lastCreate().cwd; got != explicitCWD {
		t.Fatalf("manager CWD = %q, want explicit CWD %q", got, explicitCWD)
	}

	wantErr := manager.createErrors["missing-profile"]
	if _, err := app.CreateTerminal("missing-profile", "", 24, 80); !errors.Is(err, wantErr) {
		t.Fatalf("invalid profile error = %v, want %v", err, wantErr)
	}
}

func TestAgentTerminalReceivesHostMintedCapabilityIdentityAndRevokesOnClose(t *testing.T) {
	manager := &fakeGUITerminalManager{
		profiles: []terminal.Profile{{ID: "agent-codex", Name: "Codex", Kind: terminal.ProfileAgent}},
		createResult: managedTerminalSession{
			SessionID: "session-1", ProfileID: "agent-codex", ProfileKind: terminal.ProfileAgent,
			CWD: t.TempDir(), State: terminal.SessionRunning,
		},
	}
	app, projectRoot := newTerminalBindingTestApp(t, manager, nil)
	projectRoot, _ = filepath.EvalSymlinks(projectRoot)
	broker := &fakeWorkspaceCapabilityBroker{token: "host-minted-token"}
	app.workspace.capabilities = broker
	if _, err := app.CreateTerminal("agent-codex", "", 24, 80); err != nil {
		t.Fatal(err)
	}
	call := manager.lastCreate()
	if call.environment["PTRACK_CAPABILITY_TOKEN"] != "host-minted-token" ||
		call.environment["PTRACK_CAPABILITY_PROFILE"] != "agent-codex" ||
		call.environment["PTRACK_CAPABILITY_PROJECT"] != projectRoot ||
		call.environment["PTRACK_CAPABILITY_GENERATION"] != "1" {
		t.Fatalf("capability environment = %#v", call.environment)
	}
	if broker.boundToken != "host-minted-token" || broker.boundSession != "session-1" {
		t.Fatalf("bound identity = %q / %q", broker.boundToken, broker.boundSession)
	}
	if err := app.CloseTerminal("session-1", true); err != nil {
		t.Fatal(err)
	}
	if len(broker.revokedSessions) != 1 || broker.revokedSessions[0] != "session-1" {
		t.Fatalf("revoked sessions = %v", broker.revokedSessions)
	}
}

func TestTerminalAttachmentLeaseExpiresUnclaimedSessionAfterRevocation(t *testing.T) {
	attach := make(chan struct{})
	timer := make(chan time.Time, 1)
	var expireMu sync.Mutex
	expired := false
	order := make(chan string, 2)
	closed := make(chan struct{})
	manager := &fakeGUITerminalManager{
		profiles: []terminal.Profile{{ID: "agent-codex", Name: "Codex", Kind: terminal.ProfileAgent}},
		createResult: managedTerminalSession{
			SessionID: "unclaimed", ProfileID: "agent-codex", ProfileKind: terminal.ProfileAgent,
			CWD: t.TempDir(), State: terminal.SessionRunning, attachSignal: attach,
			expireUnattached: func() bool {
				expireMu.Lock()
				defer expireMu.Unlock()
				if expired {
					return false
				}
				expired = true
				return true
			},
		},
		closeHook: func(string, bool) {
			order <- "close"
			close(closed)
		},
	}
	app, _ := newTerminalBindingTestApp(t, manager, nil)
	app.terminalAttachLease = time.Minute
	app.terminalAttachAfter = func(time.Duration) <-chan time.Time { return timer }
	broker := &fakeWorkspaceCapabilityBroker{
		token:             "host-minted-token",
		revokeSessionHook: func(string) { order <- "revoke" },
	}
	app.workspace.capabilities = broker

	if _, err := app.CreateTerminal("agent-codex", "", 24, 80); err != nil {
		t.Fatalf("CreateTerminal: %v", err)
	}
	timer <- time.Now()
	select {
	case <-closed:
	case <-time.After(time.Second):
		t.Fatal("unclaimed terminal lease did not close session")
	}
	waitContext, cancelWait := context.WithTimeout(context.Background(), time.Second)
	defer cancelWait()
	if err := app.terminalOps.WaitContext(waitContext); err != nil {
		t.Fatalf("wait for terminal lease cleanup: %v", err)
	}
	if first, second := <-order, <-order; first != "revoke" || second != "close" {
		t.Fatalf("lease cleanup order = %q then %q, want revoke then close", first, second)
	}
	if call := manager.lastClose(); call.sessionID != "unclaimed" || !call.force {
		t.Fatalf("lease close = %#v, want forced unclaimed close", call)
	}
	if got := app.workspace.activeResourceSummary().Terminals; got != 0 {
		t.Fatalf("active terminals after lease expiry = %d, want 0", got)
	}
	manager.mu.Lock()
	closeCalls := len(manager.closes)
	manager.mu.Unlock()
	if closeCalls != 1 {
		t.Fatalf("lease close calls = %d, want exactly 1", closeCalls)
	}
}

func TestTerminalAttachmentCancelsLeaseAndWorkspaceShutdownOwnsUnclaimedCleanup(t *testing.T) {
	t.Run("attachment cancels", func(t *testing.T) {
		attach := make(chan struct{})
		close(attach)
		timer := make(chan time.Time, 1)
		manager := &fakeGUITerminalManager{
			profiles: []terminal.Profile{{ID: "shell-default", Name: "Shell", Kind: terminal.ProfileShell}},
			createResult: managedTerminalSession{
				SessionID: "attached", ProfileID: "shell-default", State: terminal.SessionRunning,
				attachSignal: attach, expireUnattached: func() bool { t.Fatal("attached session expired"); return false },
			},
		}
		app, _ := newTerminalBindingTestApp(t, manager, nil)
		app.terminalAttachAfter = func(time.Duration) <-chan time.Time { return timer }
		if _, err := app.CreateTerminal("shell-default", "", 24, 80); err != nil {
			t.Fatalf("CreateTerminal: %v", err)
		}
		waitContext, cancelWait := context.WithTimeout(context.Background(), time.Second)
		defer cancelWait()
		if err := app.terminalOps.WaitContext(waitContext); err != nil {
			t.Fatalf("wait for attachment lease cancellation: %v", err)
		}
		timer <- time.Now()
		manager.mu.Lock()
		defer manager.mu.Unlock()
		if len(manager.closes) != 0 {
			t.Fatalf("attached session lease closes = %v", manager.closes)
		}
	})

	t.Run("workspace shutdown owns cleanup", func(t *testing.T) {
		attach := make(chan struct{})
		timer := make(chan time.Time, 1)
		manager := &fakeGUITerminalManager{
			profiles: []terminal.Profile{{ID: "shell-default", Name: "Shell", Kind: terminal.ProfileShell}},
			createResult: managedTerminalSession{
				SessionID: "unclaimed", ProfileID: "shell-default", State: terminal.SessionRunning,
				attachSignal: attach, expireUnattached: func() bool { t.Fatal("shutdown session expired"); return false },
			},
		}
		app, _ := newTerminalBindingTestApp(t, manager, nil)
		app.terminalAttachAfter = func(time.Duration) <-chan time.Time { return timer }
		if _, err := app.CreateTerminal("shell-default", "", 24, 80); err != nil {
			t.Fatalf("CreateTerminal: %v", err)
		}
		app.onShutdown(context.Background())
		timer <- time.Now()
		app.onShutdown(context.Background())
		manager.mu.Lock()
		defer manager.mu.Unlock()
		if len(manager.closes) != 0 {
			t.Fatalf("frontend per-session closes during shutdown = %v", manager.closes)
		}
		if manager.shutdownCalls != 1 {
			t.Fatalf("manager shutdown calls = %d, want 1", manager.shutdownCalls)
		}
	})
}

func TestCapabilityIdentityFailsClosedWhenProfileMetadataIsUnavailable(t *testing.T) {
	profilesErr := errors.New("profile discovery failed")
	manager := &fakeGUITerminalManager{
		profilesErr: profilesErr,
		createResult: managedTerminalSession{
			SessionID: "must-not-start", ProfileID: "agent-codex",
		},
	}
	app, _ := newTerminalBindingTestApp(t, manager, nil)
	app.workspace.capabilities = &fakeWorkspaceCapabilityBroker{token: "unused"}
	if _, err := app.CreateTerminal("agent-codex", "", 24, 80); !errors.Is(err, profilesErr) {
		t.Fatalf("CreateTerminal error = %v, want %v", err, profilesErr)
	}
	if len(manager.creates) != 0 {
		t.Fatalf("terminal was launched despite missing identity metadata: %v", manager.creates)
	}
}

func TestTerminalSessionJSONContainsOnlySafeOpaqueMetadata(t *testing.T) {
	manager := &fakeGUITerminalManager{
		createResult: managedTerminalSession{
			SessionID: "opaque-session",
			ProfileID: "shell-default",
			CWD:       "/test/project",
			State:     terminal.SessionRunning,
			StreamURL: "ws://127.0.0.1:49152/terminal/opaque-session?token=opaque",
		},
	}
	app, _ := newTerminalBindingTestApp(t, manager, nil)

	result, err := app.CreateTerminal("shell-default", "", 24, 80)
	if err != nil {
		t.Fatalf("CreateTerminal: %v", err)
	}
	encoded, err := json.Marshal(result)
	if err != nil {
		t.Fatalf("marshal TerminalSession: %v", err)
	}
	var fields map[string]any
	if err := json.Unmarshal(encoded, &fields); err != nil {
		t.Fatalf("decode TerminalSession JSON: %v", err)
	}
	wantFields := map[string]bool{
		"sessionId": true,
		"profileId": true,
		"cwd":       true,
		"state":     true,
		"streamUrl": true,
	}
	for field := range fields {
		if !wantFields[field] {
			t.Fatalf("TerminalSession exposes unexpected field %q in %s", field, encoded)
		}
		delete(wantFields, field)
	}
	if len(wantFields) != 0 {
		t.Fatalf("TerminalSession is missing fields %#v in %s", wantFields, encoded)
	}
	for index := 0; index < reflect.TypeOf(result).NumField(); index++ {
		name := strings.ToLower(reflect.TypeOf(result).Field(index).Name)
		if strings.Contains(name, "env") || strings.Contains(name, "token") {
			t.Fatalf("TerminalSession has forbidden backend field %q", name)
		}
	}
}

func TestValidateTerminalCWDsV2IsBoundedAndDoesNotCreateSessions(t *testing.T) {
	manager := &fakeGUITerminalManager{}
	app, projectRoot := newTerminalBindingTestApp(t, manager, nil)
	valid := filepath.Join(projectRoot, "saved")
	if err := os.Mkdir(valid, 0o755); err != nil {
		t.Fatal(err)
	}
	revision := app.workspace.resourceRevisionValue()
	result, err := app.ValidateTerminalCWDsV2(0, []string{"", valid, filepath.Join(projectRoot, "missing")})
	if err != nil {
		t.Fatalf("ValidateTerminalCWDsV2: %v", err)
	}
	if len(result.Results) != 3 || result.Results[0].CWD != "" || !result.Results[0].Valid {
		t.Fatalf("validation results = %#v", result.Results)
	}
	if !result.Results[1].Valid || result.Results[1].CWD != valid || result.Results[2].Valid {
		t.Fatalf("validation results = %#v", result.Results)
	}
	if len(manager.creates) != 0 || len(manager.closes) != 0 {
		t.Fatalf("CWD validation touched terminal sessions: creates=%d closes=%d", len(manager.creates), len(manager.closes))
	}
	if got := app.workspace.resourceRevisionValue(); got != revision {
		t.Fatalf("read-only CWD validation changed resource revision: got %d want %d", got, revision)
	}
	if _, err := app.ValidateTerminalCWDsV2(0, []string{valid, valid}); err == nil {
		t.Fatal("duplicate CWD validation did not fail")
	}
	if _, err := app.ValidateTerminalCWDsV2(0, make([]string, 97)); err == nil {
		t.Fatal("oversized CWD validation did not fail")
	}
}

func TestResizeAndCloseTerminalDelegateOrderingForceAndErrors(t *testing.T) {
	invalidSessionErr := terminal.ErrSessionNotFound
	resizeErr := errors.New("resize failed")
	closeErr := errors.New("close failed")
	manager := &fakeGUITerminalManager{
		resizeErrors: map[string]error{
			"missing": invalidSessionErr,
			"broken":  resizeErr,
		},
		closeErrors: map[string]error{
			"missing": invalidSessionErr,
			"broken":  closeErr,
		},
	}
	app, _ := newTerminalBindingTestApp(t, manager, nil)

	if err := app.ResizeTerminal("session", 42, 132); err != nil {
		t.Fatalf("ResizeTerminal: %v", err)
	}
	resize := manager.lastResize()
	if resize.sessionID != "session" || resize.rows != 42 || resize.columns != 132 {
		t.Fatalf("resize call = %#v, want session/rows/columns ordering", resize)
	}
	if err := app.CloseTerminal("session", true); err != nil {
		t.Fatalf("CloseTerminal force: %v", err)
	}
	closeCall := manager.lastClose()
	if closeCall.sessionID != "session" || !closeCall.force {
		t.Fatalf("close call = %#v, want forced close for session", closeCall)
	}
	if err := app.CloseTerminal("session", false); err != nil {
		t.Fatalf("CloseTerminal graceful: %v", err)
	}
	if manager.lastClose().force {
		t.Fatal("graceful close was delegated as force")
	}

	if err := app.ResizeTerminal("missing", 24, 80); !errors.Is(err, invalidSessionErr) {
		t.Fatalf("invalid resize error = %v, want %v", err, invalidSessionErr)
	}
	if err := app.CloseTerminal("missing", false); err != nil {
		t.Fatalf("missing close should be idempotent: %v", err)
	}
	if err := app.ResizeTerminal("broken", 24, 80); !errors.Is(err, resizeErr) {
		t.Fatalf("resize manager error = %v, want %v", err, resizeErr)
	}
	if err := app.CloseTerminal("broken", true); !errors.Is(err, closeErr) {
		t.Fatalf("close manager error = %v, want %v", err, closeErr)
	}
}

func TestCloseTerminalNotFoundIsIdempotentAndRevokesBeforeEveryClose(t *testing.T) {
	var order []string
	manager := &fakeGUITerminalManager{
		closeErrors: map[string]error{"missing": terminal.ErrSessionNotFound},
		closeHook: func(string, bool) {
			order = append(order, "close")
		},
	}
	app, _ := newTerminalBindingTestApp(t, manager, nil)
	broker := &fakeWorkspaceCapabilityBroker{
		revokeSessionHook: func(string) {
			order = append(order, "revoke")
		},
	}
	app.workspace.capabilities = broker

	for attempt := 0; attempt < 2; attempt++ {
		if err := app.CloseTerminal("missing", false); err != nil {
			t.Fatalf("CloseTerminal attempt %d: %v", attempt+1, err)
		}
	}

	wantOrder := []string{"revoke", "close", "revoke", "close"}
	if !reflect.DeepEqual(order, wantOrder) {
		t.Fatalf("close ordering = %v, want %v", order, wantOrder)
	}
	if len(manager.closes) != 2 || len(broker.revokedSessions) != 2 {
		t.Fatalf("repeated close calls = %d, revocations = %d, want 2 each", len(manager.closes), len(broker.revokedSessions))
	}
}

func TestCloseTerminalDoesNotEmitFalseClosingStateOnManagerError(t *testing.T) {
	closeErr := errors.New("close failed")
	manager := &fakeGUITerminalManager{
		closeErrors: map[string]error{"broken": closeErr},
	}
	var events []string
	emitter := terminalEventEmitter(func(_ context.Context, name string, _ any) {
		events = append(events, name)
	})
	app, _ := newTerminalBindingTestApp(t, manager, emitter)
	broker := &fakeWorkspaceCapabilityBroker{}
	app.workspace.capabilities = broker

	if err := app.CloseTerminal("broken", false); !errors.Is(err, closeErr) {
		t.Fatalf("CloseTerminal error = %v, want %v", err, closeErr)
	}
	if len(events) != 0 {
		t.Fatalf("failed close emitted events: %#v", events)
	}
	if len(broker.revokedSessions) != 1 || broker.revokedSessions[0] != "broken" {
		t.Fatalf("failed close left broker identity active: %v", broker.revokedSessions)
	}
}

func TestTerminalLifecycleRetainsStartupContextAndEmitsOnlyTypedStatusAndExit(t *testing.T) {
	var emitted []emittedTerminalEvent
	var emitMu sync.Mutex
	emitter := terminalEventEmitter(func(ctx context.Context, name string, payload any) {
		emitMu.Lock()
		defer emitMu.Unlock()
		emitted = append(emitted, emittedTerminalEvent{ctx: ctx, name: name, payload: payload})
	})
	manager := &fakeGUITerminalManager{}
	app, _ := newTerminalBindingTestApp(t, manager, emitter)
	type contextKey string
	startupContext := context.WithValue(context.Background(), contextKey("retained"), "yes")

	app.onStartup(startupContext)
	app.publishTerminalStatus(TerminalStatus{
		SessionID: "session",
		State:     terminal.SessionRunning,
	})
	app.publishTerminalExit(TerminalExit{
		SessionID: "session",
		ExitCode:  7,
		State:     terminal.SessionExited,
	})

	emitMu.Lock()
	defer emitMu.Unlock()
	if len(emitted) != 2 {
		t.Fatalf("emitted lifecycle events = %d, want 2: %#v", len(emitted), emitted)
	}
	if emitted[0].ctx != startupContext || emitted[1].ctx != startupContext {
		t.Fatal("terminal events did not use retained Wails startup context")
	}
	if emitted[0].name != "terminal:status" {
		t.Fatalf("status event name = %q, want terminal:status", emitted[0].name)
	}
	if emitted[1].name != "terminal:exit" {
		t.Fatalf("exit event name = %q, want terminal:exit", emitted[1].name)
	}
	if _, ok := emitted[0].payload.(TerminalStatus); !ok {
		t.Fatalf("status payload type = %T, want TerminalStatus", emitted[0].payload)
	}
	if _, ok := emitted[1].payload.(TerminalExit); !ok {
		t.Fatalf("exit payload type = %T, want TerminalExit", emitted[1].payload)
	}
	for _, event := range emitted {
		payloadType := reflect.TypeOf(event.payload)
		for index := 0; index < payloadType.NumField(); index++ {
			field := payloadType.Field(index)
			if field.Type == reflect.TypeOf([]byte(nil)) ||
				strings.Contains(strings.ToLower(field.Name), "output") ||
				strings.Contains(strings.ToLower(field.Name), "data") {
				t.Fatalf("%s exposes PTY bytes through field %q", event.name, field.Name)
			}
		}
	}
}

func TestTerminalShutdownIsIdempotent(t *testing.T) {
	manager := &fakeGUITerminalManager{}
	app, _ := newTerminalBindingTestApp(t, manager, nil)
	ctx := context.Background()

	app.onStartup(ctx)
	app.onShutdown(ctx)
	app.onShutdown(ctx)

	manager.mu.Lock()
	defer manager.mu.Unlock()
	if manager.shutdownCalls != 1 {
		t.Fatalf("manager Shutdown calls = %d, want exactly 1", manager.shutdownCalls)
	}
}

func TestTerminalShutdownWaitsForInFlightBindingBeforeManagerShutdown(t *testing.T) {
	createStarted := make(chan struct{})
	createRelease := make(chan struct{})
	shutdownCalled := make(chan struct{})
	manager := &fakeGUITerminalManager{
		createResult: managedTerminalSession{
			SessionID: "session",
			ProfileID: "shell-default",
			CWD:       "/test/project",
			State:     terminal.SessionRunning,
			StreamURL: "ws://127.0.0.1:49152/terminal/session?token=opaque",
		},
		createStarted:  createStarted,
		createRelease:  createRelease,
		shutdownCalled: shutdownCalled,
	}
	app, _ := newTerminalBindingTestApp(t, manager, nil)

	createDone := make(chan error, 1)
	go func() {
		_, err := app.CreateTerminal("shell-default", "", 24, 80)
		createDone <- err
	}()
	<-createStarted

	shutdownDone := make(chan struct{})
	go func() {
		app.onShutdown(context.Background())
		close(shutdownDone)
	}()
	<-app.shutdownStarted
	select {
	case <-shutdownCalled:
		t.Fatal("manager shutdown raced an in-flight terminal binding")
	case <-time.After(50 * time.Millisecond):
	}

	close(createRelease)
	if err := <-createDone; err != nil {
		t.Fatalf("CreateTerminal: %v", err)
	}
	<-shutdownDone
	select {
	case <-shutdownCalled:
	default:
		t.Fatal("manager shutdown was not called after the binding completed")
	}
}

func TestTerminalShutdownWaitsAreBounded(t *testing.T) {
	app, _ := newTerminalBindingTestApp(t, &fakeGUITerminalManager{}, nil)
	app.shutdownWaitTimeout = 20 * time.Millisecond
	app.terminalOps.Add(1)
	app.monitorWG.Add(1)

	done := make(chan struct{})
	go func() {
		app.onShutdown(context.Background())
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(250 * time.Millisecond):
		t.Fatal("application shutdown blocked on lifecycle wait groups")
	}
	app.terminalOps.Done()
	app.monitorWG.Done()
}

type fakeGUITerminalManager struct {
	mu sync.Mutex

	profiles    []terminal.Profile
	profilesErr error

	creates       []terminalCreateCall
	createResult  managedTerminalSession
	createErrors  map[string]error
	createStarted chan struct{}
	createRelease chan struct{}

	resizes      []terminalResizeCall
	resizeErrors map[string]error

	closes      []terminalCloseCall
	closeErrors map[string]error
	closeHook   func(string, bool)

	shutdownCalls        int
	shutdownErr          error
	shutdownCalled       chan struct{}
	createStartOnce      sync.Once
	shutdownCallOnce     sync.Once
	association          *association.AssociationV1
	associationErr       error
	associationCommitErr error
	closedSessions       map[string]bool
}

func (m *fakeGUITerminalManager) Profiles() ([]terminal.Profile, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.profilesErr != nil {
		return nil, m.profilesErr
	}
	return m.profiles, nil
}

func (m *fakeGUITerminalManager) Create(
	profileID string,
	cwd string,
	rows int,
	columns int,
) (managedTerminalSession, error) {
	if m.createStarted != nil {
		m.createStartOnce.Do(func() {
			close(m.createStarted)
		})
	}
	if m.createRelease != nil {
		<-m.createRelease
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	m.creates = append(m.creates, terminalCreateCall{
		profileID: profileID,
		cwd:       cwd,
		rows:      rows,
		columns:   columns,
	})
	if err := m.createErrors[profileID]; err != nil {
		return managedTerminalSession{}, err
	}
	return m.createResult, nil
}

func (m *fakeGUITerminalManager) CreateWithEnv(
	profileID string,
	cwd string,
	rows int,
	columns int,
	environment map[string]string,
) (managedTerminalSession, error) {
	result, err := m.Create(profileID, cwd, rows, columns)
	m.mu.Lock()
	if len(m.creates) > 0 {
		m.creates[len(m.creates)-1].environment = environment
	}
	m.mu.Unlock()
	return result, err
}

func (m *fakeGUITerminalManager) Resize(sessionID string, rows, columns int) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.resizes = append(m.resizes, terminalResizeCall{
		sessionID: sessionID,
		rows:      rows,
		columns:   columns,
	})
	return m.resizeErrors[sessionID]
}

func (m *fakeGUITerminalManager) Close(sessionID string, force bool) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.closes = append(m.closes, terminalCloseCall{sessionID: sessionID, force: force})
	if m.closedSessions == nil {
		m.closedSessions = make(map[string]bool)
	}
	m.closedSessions[sessionID] = true
	if m.closeHook != nil {
		m.closeHook(sessionID, force)
	}
	return m.closeErrors[sessionID]
}

func (m *fakeGUITerminalManager) Associate(
	sessionID string,
	host *association.Host,
	pointer association.PointerV1,
) (association.AssociationV1, error) {
	m.mu.Lock()
	if m.associationErr != nil {
		err := m.associationErr
		m.mu.Unlock()
		return association.AssociationV1{}, err
	}
	previous := m.association
	m.mu.Unlock()
	next, err := host.Bind(sessionID, pointer, previous)
	if err != nil {
		return association.AssociationV1{}, err
	}
	m.mu.Lock()
	m.association = &next
	m.mu.Unlock()
	return next, nil
}

func (m *fakeGUITerminalManager) PrepareAssociationChange(
	sessionID string,
	host *association.Host,
	pointer association.PointerV1,
	expectedRevision uint64,
) (terminal.AssociationChange, error) {
	m.mu.Lock()
	if sessionID == "" || m.closedSessions[sessionID] ||
		(m.createResult.SessionID != "" && m.createResult.SessionID != sessionID) ||
		(m.createResult.SessionID == sessionID && m.createResult.State != terminal.SessionRunning) {
		m.mu.Unlock()
		return terminal.AssociationChange{}, terminal.ErrSessionNotFound
	}
	var previous *association.AssociationV1
	if m.association != nil {
		copy := *m.association
		previous = &copy
	}
	if (previous == nil && expectedRevision != 0) ||
		(previous != nil && previous.Revision != expectedRevision) {
		m.mu.Unlock()
		return terminal.AssociationChange{}, association.ErrStaleAssociation
	}
	m.mu.Unlock()
	next, err := host.Bind(sessionID, pointer, previous)
	if err != nil {
		return terminal.AssociationChange{}, err
	}
	return terminal.AssociationChange{
		SessionID: sessionID,
		Previous:  previous,
		Next:      next,
	}, nil
}

func (m *fakeGUITerminalManager) CommitAssociationChange(
	change terminal.AssociationChange,
) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.associationCommitErr != nil {
		return m.associationCommitErr
	}
	if m.closedSessions[change.SessionID] ||
		!fakeAssociationsEqual(m.association, change.Previous) {
		return association.ErrStaleAssociation
	}
	next := change.Next
	m.association = &next
	return nil
}

func (m *fakeGUITerminalManager) RollbackAssociationChange(
	change terminal.AssociationChange,
) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if !fakeAssociationsEqual(m.association, &change.Next) {
		return association.ErrStaleAssociation
	}
	if change.Previous == nil {
		m.association = nil
	} else {
		previous := *change.Previous
		m.association = &previous
	}
	return nil
}

func (m *fakeGUITerminalManager) SessionInfo(
	sessionID string,
) (terminal.SessionInfo, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if sessionID == "" || m.closedSessions[sessionID] ||
		m.createResult.SessionID != sessionID {
		return terminal.SessionInfo{}, terminal.ErrSessionNotFound
	}
	info := terminal.SessionInfo{
		ID:          sessionID,
		ProfileID:   m.createResult.ProfileID,
		ProfileKind: m.createResult.ProfileKind,
		Provider:    m.createResult.Provider,
		PID:         m.createResult.PID,
		CWD:         m.createResult.CWD,
		State:       m.createResult.State,
	}
	if m.association != nil {
		copy := *m.association
		info.Association = &copy
	}
	return info, nil
}

func (m *fakeGUITerminalManager) WithLiveAssociation(
	sessionID string,
	expectedRevision uint64,
	use func(association.AssociationV1) error,
) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if sessionID == "" || m.closedSessions[sessionID] ||
		m.createResult.SessionID != sessionID ||
		m.createResult.State != terminal.SessionRunning || m.association == nil ||
		m.association.Revision != expectedRevision {
		return association.ErrStaleAssociation
	}
	copy := *m.association
	return use(copy)
}

func (m *fakeGUITerminalManager) WithExactSessionSnapshot(
	maximum int,
	use func([]terminal.SessionInfo) error,
) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if maximum <= 0 || use == nil {
		return errors.New("exact terminal snapshot callback and limit are required")
	}
	snapshot := []terminal.SessionInfo{}
	if m.createResult.SessionID != "" &&
		!m.closedSessions[m.createResult.SessionID] {
		info := terminal.SessionInfo{
			ID:          m.createResult.SessionID,
			ProfileID:   m.createResult.ProfileID,
			ProfileKind: m.createResult.ProfileKind,
			Provider:    m.createResult.Provider,
			PID:         m.createResult.PID,
			CWD:         m.createResult.CWD,
			State:       m.createResult.State,
		}
		if m.association != nil {
			copy := *m.association
			info.Association = &copy
		}
		snapshot = append(snapshot, info)
	}
	if len(snapshot) > maximum {
		return terminal.ErrSnapshotLimit
	}
	return use(snapshot)
}

func fakeAssociationsEqual(
	left, right *association.AssociationV1,
) bool {
	if left == nil || right == nil {
		return left == nil && right == nil
	}
	return *left == *right
}

func (m *fakeGUITerminalManager) Shutdown(context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.shutdownCalls++
	if m.shutdownCalled != nil {
		m.shutdownCallOnce.Do(func() {
			close(m.shutdownCalled)
		})
	}
	return m.shutdownErr
}

func (m *fakeGUITerminalManager) lastCreate() terminalCreateCall {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.creates[len(m.creates)-1]
}

func (m *fakeGUITerminalManager) lastResize() terminalResizeCall {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.resizes[len(m.resizes)-1]
}

func (m *fakeGUITerminalManager) lastClose() terminalCloseCall {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.closes[len(m.closes)-1]
}

type terminalCreateCall struct {
	profileID   string
	cwd         string
	rows        int
	columns     int
	environment map[string]string
}

type fakeWorkspaceCapabilityBroker struct {
	token             string
	issueErr          error
	bindErr           error
	issuedProfiles    []string
	boundToken        string
	boundSession      string
	revokedTokens     []string
	revokedSessions   []string
	revokeTokenHook   func(string)
	revokeSessionHook func(string)
}

func (b *fakeWorkspaceCapabilityBroker) IssueSessionToken(profile string) (string, error) {
	b.issuedProfiles = append(b.issuedProfiles, profile)
	return b.token, b.issueErr
}
func (b *fakeWorkspaceCapabilityBroker) BindSession(token, sessionID string) error {
	if b.bindErr != nil {
		return b.bindErr
	}
	b.boundToken, b.boundSession = token, sessionID
	return nil
}
func (b *fakeWorkspaceCapabilityBroker) RevokeToken(token string) {
	b.revokedTokens = append(b.revokedTokens, token)
	if b.revokeTokenHook != nil {
		b.revokeTokenHook(token)
	}
}
func (b *fakeWorkspaceCapabilityBroker) RevokeSession(sessionID string) {
	b.revokedSessions = append(b.revokedSessions, sessionID)
	if b.revokeSessionHook != nil {
		b.revokeSessionHook(sessionID)
	}
}
func (b *fakeWorkspaceCapabilityBroker) RevokeCapability(uint64)        {}
func (b *fakeWorkspaceCapabilityBroker) Shutdown(context.Context) error { return nil }

type terminalResizeCall struct {
	sessionID string
	rows      int
	columns   int
}

type terminalCloseCall struct {
	sessionID string
	force     bool
}

type emittedTerminalEvent struct {
	ctx     context.Context
	name    string
	payload any
}

func newTerminalBindingTestApp(
	t *testing.T,
	manager terminalManager,
	emitter terminalEventEmitter,
) (*App, string) {
	t.Helper()
	if emitter == nil {
		emitter = func(context.Context, string, any) {}
	}
	projectRoot := t.TempDir()
	if err := os.Mkdir(filepath.Join(projectRoot, ".ptrack"), 0o755); err != nil {
		t.Fatalf("create project metadata directory: %v", err)
	}
	dbPath := filepath.Join(projectRoot, ".ptrack", "ptrack.db")
	app, err := newAppWithTerminal(dbPath, 0, manager, emitter)
	if err != nil {
		t.Fatalf("newAppWithTerminal: %v", err)
	}
	app.onStartup(context.Background())
	return app, projectRoot
}
