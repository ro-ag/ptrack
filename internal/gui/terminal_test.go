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

func TestResizeAndCloseTerminalDelegateOrderingForceAndErrors(t *testing.T) {
	invalidSessionErr := errors.New("session not found")
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
	if err := app.CloseTerminal("missing", false); !errors.Is(err, invalidSessionErr) {
		t.Fatalf("invalid close error = %v, want %v", err, invalidSessionErr)
	}
	if err := app.ResizeTerminal("broken", 24, 80); !errors.Is(err, resizeErr) {
		t.Fatalf("resize manager error = %v, want %v", err, resizeErr)
	}
	if err := app.CloseTerminal("broken", true); !errors.Is(err, closeErr) {
		t.Fatalf("close manager error = %v, want %v", err, closeErr)
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

	if err := app.CloseTerminal("broken", false); !errors.Is(err, closeErr) {
		t.Fatalf("CloseTerminal error = %v, want %v", err, closeErr)
	}
	if len(events) != 0 {
		t.Fatalf("failed close emitted events: %#v", events)
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

	shutdownCalls    int
	shutdownErr      error
	shutdownCalled   chan struct{}
	createStartOnce  sync.Once
	shutdownCallOnce sync.Once
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
	return m.closeErrors[sessionID]
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
	profileID string
	cwd       string
	rows      int
	columns   int
}

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
