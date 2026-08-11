package gui

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/store"
	"github.com/ro-ag/ptrack/internal/updater"
)

func TestUpdateCheckWorksWithoutProjectAndExposesNoAssetAuthority(t *testing.T) {
	previousVersion := store.WriterVersion
	store.WriterVersion = "1.2.3"
	t.Cleanup(func() { store.WriterVersion = previousVersion })
	candidate := updateCandidateFixture()
	client := &fakeUpdateClient{check: func(context.Context, string, updater.Target) (updater.Candidate, error) {
		return candidate, nil
	}}
	app, events := newUpdateTestApp(t, client, &fakeUpdatePreferences{})
	state, err := app.CheckForUpdates()
	if err != nil {
		t.Fatal(err)
	}
	if app.GetWorkspaceState().Status != WorkspaceWelcome || state.Phase != UpdateAvailable ||
		state.Release == nil || state.Release.Version != "1.2.4" {
		t.Fatalf("state = %#v, workspace = %#v", state, app.GetWorkspaceState())
	}
	encoded, err := json.Marshal(state)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(encoded), "releases/download") || strings.Contains(string(encoded), "checksums.txt") {
		t.Fatalf("frontend state leaked asset authority: %s", encoded)
	}
	if events.count(updateStateEvent) < 2 {
		t.Fatalf("update events = %d, want at least 2", events.count(updateStateEvent))
	}
}

func TestUpdateDownloadApplyAndStaleVersionFences(t *testing.T) {
	previousVersion := store.WriterVersion
	store.WriterVersion = "1.2.3"
	t.Cleanup(func() { store.WriterVersion = previousVersion })
	candidate := updateCandidateFixture()
	staged := updater.StagedUpdate{
		Root: "/private/stage", Version: candidate.Version, GOOS: runtime.GOOS, GOARCH: runtime.GOARCH,
		SizeBytes: candidate.Package.SizeBytes, SHA256: strings.Repeat("a", 64),
	}
	client := &fakeUpdateClient{
		check: func(context.Context, string, updater.Target) (updater.Candidate, error) { return candidate, nil },
		stage: func(_ context.Context, got updater.Candidate, target updater.Target, root string, progress updater.ProgressFunc) (updater.StagedUpdate, error) {
			if got.Version != candidate.Version || target.GOOS != runtime.GOOS || root == "" {
				t.Fatalf("Stage inputs = %#v %#v %q", got, target, root)
			}
			progress(updater.Progress{Asset: "package", Downloaded: 256, Total: candidate.Package.SizeBytes})
			progress(updater.Progress{Asset: "package", Downloaded: candidate.Package.SizeBytes, Total: candidate.Package.SizeBytes})
			return staged, nil
		},
	}
	installer := &fakeUpdateInstaller{result: updater.ApplyResult{
		Version: candidate.Version, Action: updater.ApplyOpenedInstaller, ManualInstall: true,
	}}
	app, _ := newUpdateTestApp(t, client, &fakeUpdatePreferences{})
	app.updateInstaller = installer
	if _, err := app.CheckForUpdates(); err != nil {
		t.Fatal(err)
	}
	if _, err := app.DownloadUpdate("9.9.9"); err == nil {
		t.Fatal("stale download version was accepted")
	}
	state, err := app.DownloadUpdate(candidate.Version)
	if err != nil || state.Phase != UpdateReady || !state.ChecksumVerified || state.DownloadedBytes != candidate.Package.SizeBytes {
		t.Fatalf("download state = %#v, %v", state, err)
	}
	if _, err := app.ApplyUpdate("9.9.9"); err == nil {
		t.Fatal("stale apply version was accepted")
	}
	state, err = app.ApplyUpdate(candidate.Version)
	if err != nil || state.Phase != UpdateActionNeeded || !state.ManualInstall ||
		state.ApplyAction != string(updater.ApplyOpenedInstaller) || installer.applied.Version != candidate.Version {
		t.Fatalf("apply state = %#v, %v; applied=%#v", state, err, installer.applied)
	}
}

func TestUpdateChecksAreOptInAndPreferenceHandlesClose(t *testing.T) {
	previousVersion := store.WriterVersion
	store.WriterVersion = "1.2.3"
	t.Cleanup(func() { store.WriterVersion = previousVersion })
	var calls int
	client := &fakeUpdateClient{check: func(context.Context, string, updater.Target) (updater.Candidate, error) {
		calls++
		return updater.Candidate{}, updater.ErrNoUpdate
	}}
	preferences := &fakeUpdatePreferences{}
	app, _ := newUpdateTestApp(t, client, preferences)
	if calls != 0 {
		t.Fatalf("default startup contacted GitHub %d times", calls)
	}
	state, err := app.SetAutomaticUpdateChecks(true)
	if err != nil || !state.AutomaticChecks || !preferences.saved {
		t.Fatalf("SetAutomaticUpdateChecks = %#v, %v", state, err)
	}

	home := t.TempDir()
	t.Setenv("PTRACK_HOME", home)
	production := globalUpdatePreferences{}
	if err := production.SaveAutomaticChecks(true); err != nil {
		t.Fatal(err)
	}
	if enabled, err := production.LoadAutomaticChecks(); err != nil || !enabled {
		t.Fatalf("LoadAutomaticChecks = %t, %v", enabled, err)
	}
	global, err := store.OpenGlobal()
	if err != nil {
		t.Fatalf("preference store retained the global DB lock: %v", err)
	}
	_ = global.Close()
}

func TestAutomaticUpdatePreferenceWritesRemainSerialized(t *testing.T) {
	preferences := &controlledUpdatePreferences{
		trueSaved:   make(chan struct{}),
		releaseTrue: make(chan struct{}),
	}
	app, _ := newUpdateTestApp(t, &fakeUpdateClient{
		check: func(context.Context, string, updater.Target) (updater.Candidate, error) {
			return updater.Candidate{}, updater.ErrNoUpdate
		},
	}, preferences)
	trueDone := make(chan error, 1)
	go func() {
		_, err := app.SetAutomaticUpdateChecks(true)
		trueDone <- err
	}()
	<-preferences.trueSaved
	falseDone := make(chan error, 1)
	go func() {
		_, err := app.SetAutomaticUpdateChecks(false)
		falseDone <- err
	}()
	select {
	case <-falseDone:
		t.Fatal("second preference write bypassed the active write")
	case <-time.After(20 * time.Millisecond):
	}
	close(preferences.releaseTrue)
	if err := <-trueDone; err != nil {
		t.Fatal(err)
	}
	if err := <-falseDone; err != nil {
		t.Fatal(err)
	}
	preferences.mu.Lock()
	persisted := preferences.persisted
	preferences.mu.Unlock()
	if state := app.GetUpdateState(); state.AutomaticChecks || persisted {
		t.Fatalf("state automatic=%t persisted=%t, want both false", state.AutomaticChecks, persisted)
	}
}

func TestAutomaticUpdateCheckRunsOnceOnlyAfterPersistedOptIn(t *testing.T) {
	previousVersion := store.WriterVersion
	store.WriterVersion = "1.2.3"
	t.Cleanup(func() { store.WriterVersion = previousVersion })
	checked := make(chan struct{}, 2)
	client := &fakeUpdateClient{check: func(context.Context, string, updater.Target) (updater.Candidate, error) {
		checked <- struct{}{}
		return updater.Candidate{}, updater.ErrNoUpdate
	}}
	app := newWorkspaceCoordinator(nil, nil)
	app.updateClient = client
	app.updateInstaller = &fakeUpdateInstaller{}
	app.updatePreferences = &fakeUpdatePreferences{automatic: true}
	app.updateRoot = func() (string, error) { return t.TempDir(), nil }
	app.onStartup(context.Background())
	t.Cleanup(func() { app.onShutdown(context.Background()) })
	select {
	case <-checked:
	case <-time.After(time.Second):
		t.Fatal("automatic update check did not run")
	}
	select {
	case <-checked:
		t.Fatal("automatic update check ran more than once")
	case <-time.After(50 * time.Millisecond):
	}
}

func TestAutomaticUpdateOptOutDuringRecoveryPreventsGitHubRequest(t *testing.T) {
	previousVersion := store.WriterVersion
	store.WriterVersion = "1.2.3"
	t.Cleanup(func() { store.WriterVersion = previousVersion })
	recoveryStarted := make(chan struct{})
	releaseRecovery := make(chan struct{})
	checkCalls := 0
	client := &fakeUpdateClient{check: func(context.Context, string, updater.Target) (updater.Candidate, error) {
		checkCalls++
		return updater.Candidate{}, updater.ErrNoUpdate
	}}
	app := newWorkspaceCoordinator(nil, nil)
	app.updateClient = client
	app.updateInstaller = &fakeUpdateInstaller{}
	app.updatePreferences = &fakeUpdatePreferences{automatic: true}
	app.updateRoot = func() (string, error) {
		close(recoveryStarted)
		<-releaseRecovery
		return filepath.Join(t.TempDir(), "missing"), nil
	}
	app.onStartup(context.Background())
	t.Cleanup(func() { app.onShutdown(context.Background()) })
	<-recoveryStarted
	if _, err := app.SetAutomaticUpdateChecks(false); err != nil {
		t.Fatal(err)
	}
	close(releaseRecovery)
	waitCtx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if err := app.monitorWG.WaitContext(waitCtx); err != nil {
		t.Fatal(err)
	}
	if checkCalls != 0 {
		t.Fatalf("GitHub checks = %d after opt-out, want 0", checkCalls)
	}
}

func TestAutomaticUpdateAdmissionRechecksConsentAfterRecoveryEvent(t *testing.T) {
	eventStarted := make(chan struct{})
	releaseEvent := make(chan struct{})
	var blocked atomic.Bool
	emitter := func(_ context.Context, name string, _ any) {
		if name == updateStateEvent && blocked.CompareAndSwap(false, true) {
			close(eventStarted)
			<-releaseEvent
		}
	}
	checkCalls := 0
	app := newWorkspaceCoordinator(nil, emitter)
	app.updateClient = &fakeUpdateClient{check: func(context.Context, string, updater.Target) (updater.Candidate, error) {
		checkCalls++
		return updater.Candidate{}, updater.ErrNoUpdate
	}}
	app.updateInstaller = &fakeUpdateInstaller{}
	app.updatePreferences = &fakeUpdatePreferences{automatic: true}
	app.updateRoot = func() (string, error) { return filepath.Join(t.TempDir(), "missing"), nil }
	app.onStartup(context.Background())
	t.Cleanup(func() { app.onShutdown(context.Background()) })
	<-eventStarted
	if _, err := app.SetAutomaticUpdateChecks(false); err != nil {
		t.Fatal(err)
	}
	close(releaseEvent)
	waitCtx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if err := app.monitorWG.WaitContext(waitCtx); err != nil {
		t.Fatal(err)
	}
	if checkCalls != 0 {
		t.Fatalf("GitHub checks = %d after completed opt-out, want 0", checkCalls)
	}
}

func TestCancelUpdateOperationInvalidatesBlockedCheck(t *testing.T) {
	previousVersion := store.WriterVersion
	store.WriterVersion = "1.2.3"
	t.Cleanup(func() { store.WriterVersion = previousVersion })
	started := make(chan struct{})
	client := &fakeUpdateClient{check: func(ctx context.Context, _ string, _ updater.Target) (updater.Candidate, error) {
		close(started)
		<-ctx.Done()
		return updater.Candidate{}, ctx.Err()
	}}
	app, _ := newUpdateTestApp(t, client, &fakeUpdatePreferences{})
	done := make(chan error, 1)
	go func() {
		_, err := app.CheckForUpdates()
		done <- err
	}()
	<-started
	state := app.CancelUpdateOperation()
	if state.Phase != UpdateCanceling {
		t.Fatalf("cancel state = %#v", state)
	}
	if err := <-done; err == nil {
		t.Fatal("canceled check returned nil error")
	}
	if got := app.GetUpdateState(); got.Phase != UpdateIdle || got.Error != "" {
		t.Fatalf("final state = %#v", got)
	}
}

func TestCanceledUpdateKeepsSingleFlightFenceUntilWorkerExits(t *testing.T) {
	previousVersion := store.WriterVersion
	store.WriterVersion = "1.2.3"
	t.Cleanup(func() { store.WriterVersion = previousVersion })
	started := make(chan struct{})
	release := make(chan struct{})
	client := &fakeUpdateClient{check: func(ctx context.Context, _ string, _ updater.Target) (updater.Candidate, error) {
		close(started)
		<-ctx.Done()
		<-release
		return updater.Candidate{}, ctx.Err()
	}}
	app, _ := newUpdateTestApp(t, client, &fakeUpdatePreferences{})
	done := make(chan error, 1)
	go func() {
		_, err := app.CheckForUpdates()
		done <- err
	}()
	<-started
	app.CancelUpdateOperation()
	if _, err := app.CheckForUpdates(); err == nil || !strings.Contains(err.Error(), "active") {
		t.Fatalf("concurrent check error = %v, want active operation", err)
	}
	close(release)
	if err := <-done; err == nil {
		t.Fatal("canceled check returned nil error")
	}
}

func TestStartupRecoveryFailsClosedBeforeManualChecks(t *testing.T) {
	base := t.TempDir()
	for index := 0; index < 65; index++ {
		if err := os.Mkdir(filepath.Join(base, fmt.Sprintf(".stage-%02d", index)), 0o700); err != nil {
			t.Fatal(err)
		}
	}
	app := newWorkspaceCoordinator(nil, nil)
	app.updateClient = &fakeUpdateClient{check: func(context.Context, string, updater.Target) (updater.Candidate, error) {
		t.Fatal("blocked recovery contacted GitHub")
		return updater.Candidate{}, nil
	}}
	app.updateInstaller = &fakeUpdateInstaller{}
	app.updatePreferences = &fakeUpdatePreferences{}
	app.updateRoot = func() (string, error) { return base, nil }
	app.onStartup(context.Background())
	t.Cleanup(func() { app.onShutdown(context.Background()) })
	waitCtx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if err := app.monitorWG.WaitContext(waitCtx); err != nil {
		t.Fatal(err)
	}
	state, err := app.CheckForUpdates()
	if err == nil || state.Phase != UpdateRecoveryRequired || !strings.Contains(state.Error, "manual cleanup") {
		t.Fatalf("state = %#v, error = %v", state, err)
	}
}

func TestDownloadRevalidatesCandidateAfterResolvingStorage(t *testing.T) {
	previousVersion := store.WriterVersion
	store.WriterVersion = "1.2.3"
	t.Cleanup(func() { store.WriterVersion = previousVersion })
	candidate := updateCandidateFixture()
	stageCalls := 0
	client := &fakeUpdateClient{
		check: func(context.Context, string, updater.Target) (updater.Candidate, error) {
			return candidate, nil
		},
		stage: func(context.Context, updater.Candidate, updater.Target, string, updater.ProgressFunc) (updater.StagedUpdate, error) {
			stageCalls++
			return updater.StagedUpdate{}, nil
		},
	}
	app, _ := newUpdateTestApp(t, client, &fakeUpdatePreferences{})
	if _, err := app.CheckForUpdates(); err != nil {
		t.Fatal(err)
	}
	storageStarted := make(chan struct{})
	storageContinue := make(chan struct{})
	app.updateMu.Lock()
	app.updateRoot = func() (string, error) {
		close(storageStarted)
		<-storageContinue
		return t.TempDir(), nil
	}
	app.updateMu.Unlock()
	done := make(chan error, 1)
	go func() {
		_, err := app.DownloadUpdate(candidate.Version)
		done <- err
	}()
	<-storageStarted
	app.updateMu.Lock()
	replacement := candidate
	replacement.Package.SizeBytes++
	app.updateCandidate = &replacement
	app.updateMu.Unlock()
	close(storageContinue)
	if err := <-done; err == nil || !strings.Contains(err.Error(), "stale") {
		t.Fatalf("DownloadUpdate error = %v, want stale", err)
	}
	if stageCalls != 0 {
		t.Fatalf("Stage calls = %d, want 0", stageCalls)
	}
}

func TestShutdownCancelsAndWaitsForUpdateOperation(t *testing.T) {
	previousVersion := store.WriterVersion
	store.WriterVersion = "1.2.3"
	t.Cleanup(func() { store.WriterVersion = previousVersion })
	candidate := updateCandidateFixture()
	stageStarted := make(chan struct{})
	stageCanceled := make(chan struct{})
	client := &fakeUpdateClient{
		check: func(context.Context, string, updater.Target) (updater.Candidate, error) {
			return candidate, nil
		},
		stage: func(ctx context.Context, _ updater.Candidate, _ updater.Target, _ string, _ updater.ProgressFunc) (updater.StagedUpdate, error) {
			close(stageStarted)
			<-ctx.Done()
			close(stageCanceled)
			return updater.StagedUpdate{}, ctx.Err()
		},
	}
	app, _ := newUpdateTestApp(t, client, &fakeUpdatePreferences{})
	if _, err := app.CheckForUpdates(); err != nil {
		t.Fatal(err)
	}
	operationDone := make(chan error, 1)
	go func() {
		_, err := app.DownloadUpdate(candidate.Version)
		operationDone <- err
	}()
	<-stageStarted
	app.onShutdown(context.Background())
	select {
	case <-stageCanceled:
	default:
		t.Fatal("shutdown returned before canceling the update")
	}
	if err := <-operationDone; err == nil {
		t.Fatal("canceled update returned nil error")
	}
	waitCtx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if err := app.updateOps.WaitContext(waitCtx); err != nil {
		t.Fatalf("update operation remained active after shutdown: %v", err)
	}
}

func TestUpdateErrorsAreBoundedAndDoNotExposeTransportDetails(t *testing.T) {
	previousVersion := store.WriterVersion
	store.WriterVersion = "1.2.3"
	t.Cleanup(func() { store.WriterVersion = previousVersion })
	client := &fakeUpdateClient{check: func(context.Context, string, updater.Target) (updater.Candidate, error) {
		return updater.Candidate{}, errors.New("secret-token at https://example.invalid/private")
	}}
	app, _ := newUpdateTestApp(t, client, &fakeUpdatePreferences{})
	state, err := app.CheckForUpdates()
	if err == nil || state.Phase != UpdateError || strings.Contains(state.Error, "secret-token") || strings.Contains(err.Error(), "example.invalid") {
		t.Fatalf("state = %#v, error = %v", state, err)
	}
}

type fakeUpdateClient struct {
	check func(context.Context, string, updater.Target) (updater.Candidate, error)
	stage func(context.Context, updater.Candidate, updater.Target, string, updater.ProgressFunc) (updater.StagedUpdate, error)
}

func (f *fakeUpdateClient) Check(ctx context.Context, version string, target updater.Target) (updater.Candidate, error) {
	return f.check(ctx, version, target)
}

func (f *fakeUpdateClient) Stage(ctx context.Context, candidate updater.Candidate, target updater.Target, root string, progress updater.ProgressFunc) (updater.StagedUpdate, error) {
	return f.stage(ctx, candidate, target, root, progress)
}

type fakeUpdateInstaller struct {
	result  updater.ApplyResult
	applied updater.StagedUpdate
}

func (f *fakeUpdateInstaller) Apply(_ context.Context, stage updater.StagedUpdate) (updater.ApplyResult, error) {
	f.applied = stage
	return f.result, nil
}

type fakeUpdatePreferences struct {
	automatic bool
	saved     bool
	err       error
}

type controlledUpdatePreferences struct {
	mu          sync.Mutex
	persisted   bool
	trueSaved   chan struct{}
	releaseTrue chan struct{}
}

func (p *controlledUpdatePreferences) LoadAutomaticChecks() (bool, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.persisted, nil
}

func (p *controlledUpdatePreferences) SaveAutomaticChecks(enabled bool) error {
	p.mu.Lock()
	p.persisted = enabled
	p.mu.Unlock()
	if enabled {
		close(p.trueSaved)
		<-p.releaseTrue
	}
	return nil
}

func (f *fakeUpdatePreferences) LoadAutomaticChecks() (bool, error) { return f.automatic, f.err }
func (f *fakeUpdatePreferences) SaveAutomaticChecks(enabled bool) error {
	f.saved = enabled
	return f.err
}

type updateEventLog struct {
	mu     sync.Mutex
	events []string
}

func (l *updateEventLog) add(name string) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.events = append(l.events, name)
}

func (l *updateEventLog) count(name string) int {
	l.mu.Lock()
	defer l.mu.Unlock()
	count := 0
	for _, event := range l.events {
		if event == name {
			count++
		}
	}
	return count
}

func newUpdateTestApp(t *testing.T, client updateClient, preferences updatePreferenceStore) (*App, *updateEventLog) {
	t.Helper()
	events := &updateEventLog{}
	app := newWorkspaceCoordinator(nil, func(_ context.Context, name string, _ any) { events.add(name) })
	app.updateClient = client
	app.updateInstaller = &fakeUpdateInstaller{}
	app.updatePreferences = preferences
	root := filepath.Join(t.TempDir(), "updates")
	app.updateRoot = func() (string, error) { return root, nil }
	app.onStartup(context.Background())
	t.Cleanup(func() { app.onShutdown(context.Background()) })
	waitCtx, cancel := context.WithTimeout(context.Background(), time.Second)
	t.Cleanup(cancel)
	if err := app.monitorWG.WaitContext(waitCtx); err != nil {
		t.Fatalf("wait for update recovery: %v", err)
	}
	return app, events
}

func updateCandidateFixture() updater.Candidate {
	return updater.Candidate{
		Version:     "1.2.4",
		Tag:         "v1.2.4",
		PageURL:     "https://github.com/ro-ag/ptrack/releases/tag/v1.2.4",
		Notes:       "Release notes",
		PublishedAt: time.Date(2026, 8, 11, 0, 0, 0, 0, time.UTC),
		Package: updater.Asset{
			Name: "private-package", DownloadURL: "https://github.com/ro-ag/ptrack/releases/download/v1.2.4/private-package", SizeBytes: 1024,
		},
		Checksums: updater.Asset{Name: "checksums.txt", DownloadURL: "https://github.com/ro-ag/ptrack/releases/download/v1.2.4/checksums.txt", SizeBytes: 128},
	}
}
