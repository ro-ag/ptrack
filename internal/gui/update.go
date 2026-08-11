package gui

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"time"

	"github.com/ro-ag/ptrack/internal/store"
	"github.com/ro-ag/ptrack/internal/updater"
)

const (
	updateStateEvent      = "update:state-changed"
	updatePreferenceKey   = "updates.auto-check"
	updateProgressQuantum = 256 << 10
)

type UpdatePhase string

const (
	UpdateIdle             UpdatePhase = "idle"
	UpdateRecovering       UpdatePhase = "recovering"
	UpdateRecoveryRequired UpdatePhase = "recovery-required"
	UpdateChecking         UpdatePhase = "checking"
	UpdateCurrent          UpdatePhase = "current"
	UpdateAvailable        UpdatePhase = "available"
	UpdateDownloading      UpdatePhase = "downloading"
	UpdateReady            UpdatePhase = "ready"
	UpdateApplying         UpdatePhase = "applying"
	UpdateCanceling        UpdatePhase = "canceling"
	UpdateInstalled        UpdatePhase = "installed"
	UpdateActionNeeded     UpdatePhase = "action-required"
	UpdateUnavailable      UpdatePhase = "unavailable"
	UpdateError            UpdatePhase = "error"
)

type UpdateRelease struct {
	Version     string `json:"version"`
	PublishedAt string `json:"publishedAt,omitempty"`
	SizeBytes   int64  `json:"sizeBytes"`
	Notes       string `json:"notes,omitempty"`
	PageURL     string `json:"pageUrl,omitempty"`
}

type UpdateState struct {
	Revision         uint64         `json:"revision"`
	Phase            UpdatePhase    `json:"phase"`
	CurrentVersion   string         `json:"currentVersion"`
	AutomaticChecks  bool           `json:"automaticChecks"`
	Release          *UpdateRelease `json:"release,omitempty"`
	DownloadedBytes  int64          `json:"downloadedBytes"`
	TotalBytes       int64          `json:"totalBytes"`
	ChecksumVerified bool           `json:"checksumVerified"`
	LastCheckedAt    string         `json:"lastCheckedAt,omitempty"`
	Error            string         `json:"error,omitempty"`
	ApplyAction      string         `json:"applyAction,omitempty"`
	RestartRequired  bool           `json:"restartRequired"`
	ManualInstall    bool           `json:"manualInstall"`
	CleanupPending   bool           `json:"cleanupPending"`
}

type updateClient interface {
	Check(context.Context, string, updater.Target) (updater.Candidate, error)
	Stage(context.Context, updater.Candidate, updater.Target, string, updater.ProgressFunc) (updater.StagedUpdate, error)
}

type updateInstaller interface {
	Apply(context.Context, updater.StagedUpdate) (updater.ApplyResult, error)
}

type updatePreferenceStore interface {
	LoadAutomaticChecks() (bool, error)
	SaveAutomaticChecks(bool) error
}

type globalUpdatePreferences struct{}

func (globalUpdatePreferences) LoadAutomaticChecks() (bool, error) {
	global, err := store.OpenGlobal()
	if err != nil {
		return false, err
	}
	defer global.Close()
	value, err := global.GetConfig(updatePreferenceKey)
	return value == "true", err
}

func (globalUpdatePreferences) SaveAutomaticChecks(enabled bool) error {
	global, err := store.OpenGlobal()
	if err != nil {
		return err
	}
	defer global.Close()
	value := "false"
	if enabled {
		value = "true"
	}
	return global.SetConfig(updatePreferenceKey, value)
}

func configureProductionUpdater(app *App) {
	app.updateMu.Lock()
	defer app.updateMu.Unlock()
	app.updateClient = updater.NewClient()
	app.updateInstaller = updater.NewInstaller()
	app.updatePreferences = globalUpdatePreferences{}
	app.updateRoot = func() (string, error) {
		home, err := store.GlobalHome()
		if err != nil {
			return "", err
		}
		return filepath.Join(home, "updates"), nil
	}
	app.updateState.CurrentVersion = store.WriterVersion
}

func (a *App) GetUpdateState() UpdateState {
	a.updateMu.Lock()
	defer a.updateMu.Unlock()
	return cloneUpdateState(a.updateState)
}

func (a *App) SetAutomaticUpdateChecks(enabled bool) (UpdateState, error) {
	a.updatePreferenceMu.Lock()
	defer a.updatePreferenceMu.Unlock()
	a.updateMu.Lock()
	preferences := a.updatePreferences
	a.updateMu.Unlock()
	if preferences == nil {
		return a.GetUpdateState(), errors.New("update preferences are unavailable")
	}
	if err := preferences.SaveAutomaticChecks(enabled); err != nil {
		return a.GetUpdateState(), errors.New("could not save update preferences")
	}
	a.updateMu.Lock()
	if !enabled && a.updateAutomatic && a.updateCancel != nil {
		a.updateCancel()
		a.updateCancel = nil
		a.updateState.Phase = UpdateCanceling
		a.updateState.DownloadedBytes = 0
	}
	a.updateState.AutomaticChecks = enabled
	a.updateState.Revision++
	state := cloneUpdateState(a.updateState)
	a.updateMu.Unlock()
	a.emitTerminalEvent(updateStateEvent, state)
	return state, nil
}

func (a *App) CheckForUpdates() (UpdateState, error) {
	return a.checkForUpdates(false)
}

func (a *App) checkForUpdates(automatic bool) (UpdateState, error) {
	a.updateMu.Lock()
	if a.stagedUpdate != nil {
		state := cloneUpdateState(a.updateState)
		a.updateMu.Unlock()
		return state, errors.New("a verified update is already ready")
	}
	client := a.updateClient
	a.updateMu.Unlock()
	if client == nil {
		return a.GetUpdateState(), errors.New("updates are unavailable")
	}
	ctx, operation, state, done, err := a.beginUpdateOperation(UpdateChecking, 0, automatic)
	if err != nil {
		if automatic {
			return state, nil
		}
		return state, err
	}
	defer done()
	a.updateMu.Lock()
	if a.stagedUpdate != nil {
		cancel := a.updateCancel
		a.updateMu.Unlock()
		if cancel != nil {
			cancel()
		}
		return a.finishUpdateFailure(operation, "", context.Canceled)
	}
	currentVersion := a.updateState.CurrentVersion
	a.updateMu.Unlock()
	candidate, checkErr := client.Check(ctx, currentVersion, updater.Target{GOOS: runtime.GOOS, GOARCH: runtime.GOARCH})
	checkedAt := time.Now().UTC().Format(time.RFC3339)
	if checkErr != nil {
		if errors.Is(checkErr, context.Canceled) {
			return a.finishUpdateFailure(operation, "", checkErr)
		}
		return a.finishUpdateCheck(operation, checkedAt, updater.Candidate{}, checkErr)
	}
	return a.finishUpdateCheck(operation, checkedAt, candidate, nil)
}

func (a *App) DownloadUpdate(expectedVersion string) (UpdateState, error) {
	a.updateMu.Lock()
	client := a.updateClient
	root := a.updateRoot
	if a.updateCandidate == nil || a.updateCandidate.Version != expectedVersion {
		state := cloneUpdateState(a.updateState)
		a.updateMu.Unlock()
		return state, errors.New("the selected update is stale")
	}
	candidate := *a.updateCandidate
	a.updateMu.Unlock()
	if client == nil || root == nil {
		return a.GetUpdateState(), errors.New("update downloads are unavailable")
	}
	baseDir, err := root()
	if err != nil {
		return a.GetUpdateState(), errors.New("update storage is unavailable")
	}
	ctx, operation, state, done, err := a.beginUpdateOperation(UpdateDownloading, candidate.Package.SizeBytes, false)
	if err != nil {
		return state, err
	}
	defer done()
	a.updateMu.Lock()
	if a.updateCandidate == nil || *a.updateCandidate != candidate {
		cancel := a.updateCancel
		a.updateMu.Unlock()
		if cancel != nil {
			cancel()
		}
		state, _ = a.finishUpdateFailure(operation, "", context.Canceled)
		return state, errors.New("the selected update is stale")
	}
	a.updateMu.Unlock()
	lastProgress := int64(0)
	stage, stageErr := client.Stage(
		ctx,
		candidate,
		updater.Target{GOOS: runtime.GOOS, GOARCH: runtime.GOARCH},
		baseDir,
		func(progress updater.Progress) {
			if progress.Asset != "package" ||
				(progress.Downloaded != progress.Total && progress.Downloaded-lastProgress < updateProgressQuantum) {
				return
			}
			lastProgress = progress.Downloaded
			a.publishUpdateProgress(operation, progress)
		},
	)
	if stageErr != nil {
		return a.finishUpdateFailure(operation, "The update could not be downloaded safely.", stageErr)
	}
	a.updateMu.Lock()
	if operation != a.updateOperation {
		state = cloneUpdateState(a.updateState)
		a.updateMu.Unlock()
		return state, errors.New("update download was canceled")
	}
	a.updateCancel = nil
	a.updateActive = false
	a.updateAutomatic = false
	a.stagedUpdate = &stage
	a.updateState.Phase = UpdateReady
	a.updateState.DownloadedBytes = stage.SizeBytes
	a.updateState.TotalBytes = stage.SizeBytes
	a.updateState.ChecksumVerified = true
	a.updateState.Error = ""
	a.updateState.Revision++
	state = cloneUpdateState(a.updateState)
	a.updateMu.Unlock()
	a.emitTerminalEvent(updateStateEvent, state)
	return state, nil
}

func (a *App) ApplyUpdate(expectedVersion string) (UpdateState, error) {
	a.updateMu.Lock()
	installer := a.updateInstaller
	if a.stagedUpdate == nil || a.stagedUpdate.Version != expectedVersion {
		state := cloneUpdateState(a.updateState)
		a.updateMu.Unlock()
		return state, errors.New("the verified update is stale")
	}
	stage := *a.stagedUpdate
	a.updateMu.Unlock()
	if installer == nil {
		return a.GetUpdateState(), errors.New("update installation is unavailable")
	}
	ctx, operation, state, done, err := a.beginUpdateOperation(UpdateApplying, stage.SizeBytes, false)
	if err != nil {
		return state, err
	}
	defer done()
	a.updateMu.Lock()
	if a.stagedUpdate == nil || *a.stagedUpdate != stage {
		cancel := a.updateCancel
		a.updateMu.Unlock()
		if cancel != nil {
			cancel()
		}
		state, _ = a.finishUpdateFailure(operation, "", context.Canceled)
		return state, errors.New("the verified update is stale")
	}
	a.updateMu.Unlock()
	result, applyErr := installer.Apply(ctx, stage)
	if applyErr != nil {
		return a.finishUpdateFailure(operation, "The verified update could not be installed safely.", applyErr)
	}
	a.updateMu.Lock()
	if operation != a.updateOperation {
		state = cloneUpdateState(a.updateState)
		a.updateMu.Unlock()
		return state, errors.New("update installation was canceled")
	}
	a.updateCancel = nil
	a.updateActive = false
	a.updateAutomatic = false
	if result.ManualInstall {
		a.updateState.Phase = UpdateActionNeeded
	} else {
		a.updateState.Phase = UpdateInstalled
	}
	a.updateState.ApplyAction = string(result.Action)
	a.updateState.RestartRequired = result.RestartRequired
	a.updateState.ManualInstall = result.ManualInstall
	a.updateState.CleanupPending = result.CleanupPending
	a.updateState.Error = ""
	a.updateState.Revision++
	state = cloneUpdateState(a.updateState)
	a.updateMu.Unlock()
	a.emitTerminalEvent(updateStateEvent, state)
	return state, nil
}

func (a *App) CancelUpdateOperation() UpdateState {
	a.updateMu.Lock()
	if a.updateCancel != nil {
		a.updateCancel()
		a.updateCancel = nil
		a.updateState.Phase = UpdateCanceling
		a.updateState.DownloadedBytes = 0
		a.updateState.Error = ""
		a.updateState.Revision++
	}
	state := cloneUpdateState(a.updateState)
	a.updateMu.Unlock()
	a.emitTerminalEvent(updateStateEvent, state)
	return state
}

func (a *App) startUpdater() {
	a.updateMu.Lock()
	preferences := a.updatePreferences
	configured := a.updateClient != nil && preferences != nil && a.updateRoot != nil
	a.updateMu.Unlock()
	if !configured {
		return
	}
	a.updatePreferenceMu.Lock()
	automatic, err := preferences.LoadAutomaticChecks()
	if err != nil {
		automatic = false
	}
	a.updateMu.Lock()
	a.updateState.AutomaticChecks = automatic
	a.updateState.CurrentVersion = store.WriterVersion
	a.updateState.Revision++
	a.updateMu.Unlock()
	a.updatePreferenceMu.Unlock()
	a.lifecycleMu.Lock()
	if a.shuttingDown || a.monitorCtx == nil {
		a.lifecycleMu.Unlock()
		return
	}
	ctx := a.monitorCtx
	a.monitorWG.Add(1)
	a.updateMu.Lock()
	a.updateRecovering = true
	a.updateState.Phase = UpdateRecovering
	a.updateState.Revision++
	a.updateMu.Unlock()
	a.lifecycleMu.Unlock()
	go func() {
		defer a.monitorWG.Done()
		a.recoverReadyUpdate(ctx)
		a.updateMu.Lock()
		a.updateRecovering = false
		blocked := a.updateBlocked
		if !blocked && a.stagedUpdate == nil {
			a.updateState.Phase = UpdateIdle
			a.updateState.Revision++
		}
		a.updateMu.Unlock()
		a.emitTerminalEvent(updateStateEvent, a.GetUpdateState())
		if !blocked && ctx.Err() == nil {
			_, _ = a.checkForUpdates(true)
		}
	}()
}

func (a *App) beginUpdateOperation(
	phase UpdatePhase,
	totalBytes int64,
	automatic bool,
) (context.Context, uint64, UpdateState, func(), error) {
	preferenceLocked := false
	if automatic {
		a.updatePreferenceMu.Lock()
		preferenceLocked = true
	}
	defer func() {
		if preferenceLocked {
			a.updatePreferenceMu.Unlock()
		}
	}()
	a.lifecycleMu.Lock()
	monitorCtx := a.monitorCtx
	shuttingDown := a.shuttingDown
	if monitorCtx == nil || shuttingDown {
		a.lifecycleMu.Unlock()
		return nil, 0, a.GetUpdateState(), nil, errors.New("update service is not running")
	}
	a.updateMu.Lock()
	if automatic && !a.updateState.AutomaticChecks {
		state := cloneUpdateState(a.updateState)
		a.updateMu.Unlock()
		a.lifecycleMu.Unlock()
		return nil, 0, state, nil, errors.New("automatic update checks are disabled")
	}
	if a.updateBlocked {
		state := cloneUpdateState(a.updateState)
		a.updateMu.Unlock()
		a.lifecycleMu.Unlock()
		return nil, 0, state, nil, errors.New("update recovery requires attention")
	}
	if a.updateRecovering {
		state := cloneUpdateState(a.updateState)
		a.updateMu.Unlock()
		a.lifecycleMu.Unlock()
		return nil, 0, state, nil, errors.New("update recovery is still running")
	}
	if a.updateActive {
		state := cloneUpdateState(a.updateState)
		a.updateMu.Unlock()
		a.lifecycleMu.Unlock()
		return nil, 0, state, nil, errors.New("another update operation is active")
	}
	ctx, cancel := context.WithCancel(monitorCtx)
	a.updateOps.Add(1)
	a.updateCancel = cancel
	a.updateActive = true
	a.updateAutomatic = automatic
	a.updateOperation++
	operation := a.updateOperation
	a.updateState.Phase = phase
	a.updateState.Error = ""
	if phase == UpdateDownloading {
		a.updateState.DownloadedBytes = 0
		a.updateState.TotalBytes = totalBytes
		a.updateState.ChecksumVerified = false
	}
	a.updateState.Revision++
	state := cloneUpdateState(a.updateState)
	a.updateMu.Unlock()
	a.lifecycleMu.Unlock()
	if preferenceLocked {
		preferenceLocked = false
		a.updatePreferenceMu.Unlock()
	}
	a.emitTerminalEvent(updateStateEvent, state)
	return ctx, operation, state, a.updateOps.Done, nil
}

func (a *App) finishUpdateCheck(operation uint64, checkedAt string, candidate updater.Candidate, checkErr error) (UpdateState, error) {
	a.updateMu.Lock()
	if operation != a.updateOperation {
		state := cloneUpdateState(a.updateState)
		a.updateMu.Unlock()
		return state, errors.New("update check was canceled")
	}
	a.updateCancel = nil
	a.updateActive = false
	a.updateAutomatic = false
	a.updateState.LastCheckedAt = checkedAt
	a.updateState.Revision++
	var publicErr error
	switch {
	case checkErr == nil:
		a.updateCandidate = &candidate
		a.updateState.Phase = UpdateAvailable
		a.updateState.Release = releaseState(candidate)
		a.updateState.Error = ""
	case errors.Is(checkErr, updater.ErrNoUpdate):
		a.updateCandidate = nil
		a.updateState.Phase = UpdateCurrent
		a.updateState.Release = nil
		a.updateState.Error = ""
	case errors.Is(checkErr, updater.ErrDevelopmentBuild), errors.Is(checkErr, updater.ErrUnsupportedTarget):
		a.updateCandidate = nil
		a.updateState.Phase = UpdateUnavailable
		a.updateState.Release = nil
		a.updateState.Error = "Updates are unavailable for this build."
	default:
		a.updateCandidate = nil
		a.updateState.Phase = UpdateError
		a.updateState.Release = nil
		a.updateState.Error = "The GitHub Release could not be verified."
		publicErr = errors.New(a.updateState.Error)
	}
	state := cloneUpdateState(a.updateState)
	a.updateMu.Unlock()
	a.emitTerminalEvent(updateStateEvent, state)
	return state, publicErr
}

func (a *App) finishUpdateFailure(operation uint64, message string, failure error) (UpdateState, error) {
	a.updateMu.Lock()
	if operation != a.updateOperation {
		state := cloneUpdateState(a.updateState)
		a.updateMu.Unlock()
		return state, errors.New("update operation was canceled")
	}
	a.updateCancel = nil
	a.updateActive = false
	a.updateAutomatic = false
	if errors.Is(failure, context.Canceled) {
		if a.stagedUpdate != nil {
			a.updateState.Phase = UpdateReady
		} else if a.updateCandidate != nil {
			a.updateState.Phase = UpdateAvailable
		} else {
			a.updateState.Phase = UpdateIdle
		}
		a.updateState.Error = ""
	} else {
		a.updateState.Phase = UpdateError
		a.updateState.Error = message
	}
	a.updateState.Revision++
	state := cloneUpdateState(a.updateState)
	a.updateMu.Unlock()
	a.emitTerminalEvent(updateStateEvent, state)
	if errors.Is(failure, context.Canceled) {
		return state, errors.New("update operation was canceled")
	}
	return state, errors.New(message)
}

func (a *App) publishUpdateProgress(operation uint64, progress updater.Progress) {
	a.updateMu.Lock()
	if operation != a.updateOperation || a.updateState.Phase != UpdateDownloading {
		a.updateMu.Unlock()
		return
	}
	a.updateState.DownloadedBytes = progress.Downloaded
	a.updateState.TotalBytes = progress.Total
	a.updateState.Revision++
	state := cloneUpdateState(a.updateState)
	a.updateMu.Unlock()
	a.emitTerminalEvent(updateStateEvent, state)
}

func (a *App) recoverReadyUpdate(ctx context.Context) {
	a.updateMu.Lock()
	root := a.updateRoot
	current := a.updateState.CurrentVersion
	a.updateMu.Unlock()
	if root == nil {
		return
	}
	base, err := root()
	if err != nil {
		return
	}
	entries, err := os.ReadDir(base)
	if err != nil {
		return
	}
	names := make([]string, 0, len(entries))
	for _, entry := range entries {
		if entry.IsDir() && strings.HasPrefix(entry.Name(), ".stage-") {
			names = append(names, entry.Name())
		}
	}
	sort.Strings(names)
	if len(names) > 64 {
		a.blockUpdateRecovery("Too many saved updates require manual cleanup.")
		return
	}
	var best *updater.StagedUpdate
	valid := make([]updater.StagedUpdate, 0, len(names))
	for _, name := range names {
		if ctx.Err() != nil {
			return
		}
		stage, err := updater.LoadStageContext(ctx, filepath.Join(base, name))
		if err != nil || stage.GOOS != runtime.GOOS || stage.GOARCH != runtime.GOARCH {
			continue
		}
		if _, err := updater.RecoverPendingApply(ctx, stage.Root); err != nil {
			if !errors.Is(err, updater.ErrPendingStageMismatch) {
				if ctx.Err() == nil {
					a.blockUpdateRecovery("A previous update requires manual recovery.")
				}
				return
			}
		}
		valid = append(valid, stage)
		comparison, err := updater.CompareVersions(stage.Version, current)
		if err != nil || comparison <= 0 {
			continue
		}
		if best == nil {
			copy := stage
			best = &copy
			continue
		}
		if newer, _ := updater.CompareVersions(stage.Version, best.Version); newer > 0 {
			copy := stage
			best = &copy
		}
	}
	if runtime.GOOS == "linux" && pendingApplyRecordExists(base) {
		a.blockUpdateRecovery("A previous update requires manual recovery.")
		return
	}
	if best == nil {
		if ctx.Err() != nil {
			return
		}
		a.discardSupersededStages(valid, "")
		return
	}
	if ctx.Err() != nil {
		return
	}
	a.discardSupersededStages(valid, best.Root)
	a.updateMu.Lock()
	a.stagedUpdate = best
	a.updateState.Phase = UpdateReady
	a.updateState.Release = &UpdateRelease{Version: best.Version, SizeBytes: best.SizeBytes}
	a.updateState.DownloadedBytes = best.SizeBytes
	a.updateState.TotalBytes = best.SizeBytes
	a.updateState.ChecksumVerified = true
	a.updateState.Revision++
	a.updateMu.Unlock()
}

func pendingApplyRecordExists(base string) bool {
	entries, err := os.ReadDir(base)
	if err != nil {
		return true
	}
	for _, entry := range entries {
		name := entry.Name()
		if !entry.IsDir() && strings.HasPrefix(name, ".pending-apply-") && strings.HasSuffix(name, ".json") {
			return true
		}
	}
	return false
}

func (a *App) blockUpdateRecovery(message string) {
	a.updateMu.Lock()
	a.updateBlocked = true
	a.updateState.Phase = UpdateRecoveryRequired
	a.updateState.Error = message
	a.updateState.Revision++
	a.updateMu.Unlock()
}

func (a *App) discardSupersededStages(stages []updater.StagedUpdate, keepRoot string) {
	cleanupPending := false
	for _, stage := range stages {
		if stage.Root == keepRoot {
			continue
		}
		if err := os.RemoveAll(stage.Root); err != nil {
			cleanupPending = true
		}
	}
	if cleanupPending {
		a.updateMu.Lock()
		a.updateState.CleanupPending = true
		a.updateState.Revision++
		a.updateMu.Unlock()
	}
}

func releaseState(candidate updater.Candidate) *UpdateRelease {
	return &UpdateRelease{
		Version:     candidate.Version,
		PublishedAt: candidate.PublishedAt.UTC().Format(time.RFC3339),
		SizeBytes:   candidate.Package.SizeBytes,
		Notes:       candidate.Notes,
		PageURL:     candidate.PageURL,
	}
}

func cloneUpdateState(state UpdateState) UpdateState {
	if state.Release != nil {
		release := *state.Release
		state.Release = &release
	}
	return state
}
