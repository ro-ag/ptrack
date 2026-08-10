package gui

import (
	"context"
	"os"
	"time"
)

const (
	// workspaceDataChangedEvent tells the frontend the project database was
	// written (by any process) and open views should reload.
	workspaceDataChangedEvent = "workspace:data-changed"
	// workspaceRuntimeChangedEvent tells the frontend that generation-scoped
	// terminal or AgentRun association/lifecycle state changed.
	workspaceRuntimeChangedEvent = "workspace:runtime-changed"

	workspaceWatchInterval = 2 * time.Second
	workspaceWatchDebounce = 500 * time.Millisecond
)

// workspaceFileState is the comparable fingerprint of the project database.
type workspaceFileState struct {
	exists  bool
	size    int64
	modTime time.Time
}

func statWorkspaceFile(path string) workspaceFileState {
	info, err := os.Stat(path)
	if err != nil {
		return workspaceFileState{}
	}
	return workspaceFileState{
		exists:  true,
		size:    info.Size(),
		modTime: info.ModTime(),
	}
}

// startWorkspaceWatcher (re)starts the polling database watcher for the
// published workspace. It is a no-op before startup or during shutdown.
func (a *App) startWorkspaceWatcher(workspace *WorkspaceContext) {
	a.lifecycleMu.Lock()
	if a.wailsContext == nil || a.shuttingDown || a.monitorCtx == nil {
		a.lifecycleMu.Unlock()
		return
	}
	if a.watcherCancel != nil {
		a.watcherCancel()
	}
	ctx, cancel := context.WithCancel(a.monitorCtx)
	a.watcherCancel = cancel
	a.watcherGeneration = workspace.Generation()
	a.monitorWG.Add(1)
	a.lifecycleMu.Unlock()

	dbPath := workspace.dbPath
	generation := workspace.Generation()
	go func() {
		defer a.monitorWG.Done()
		watchWorkspaceData(ctx, dbPath, workspaceWatchInterval, workspaceWatchDebounce, func() {
			a.emitTerminalEvent(workspaceDataChangedEvent, generation)
		})
	}()
}

// stopWorkspaceWatcher cancels the active database watcher, if any.
func (a *App) stopWorkspaceWatcher() {
	a.lifecycleMu.Lock()
	cancel := a.watcherCancel
	a.watcherCancel = nil
	a.watcherGeneration = 0
	a.lifecycleMu.Unlock()
	if cancel != nil {
		cancel()
	}
}

// watchWorkspaceData polls the project database every interval and invokes
// emit once per burst of changes, debounce after the last observed change.
func watchWorkspaceData(
	ctx context.Context,
	dbPath string,
	interval time.Duration,
	debounce time.Duration,
	emit func(),
) {
	previous := statWorkspaceFile(dbPath)
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	var debounceTimer *time.Timer
	var debounceC <-chan time.Time
	stopDebounce := func() {
		if debounceTimer != nil {
			debounceTimer.Stop()
			debounceTimer = nil
			debounceC = nil
		}
	}
	defer stopDebounce()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			current := statWorkspaceFile(dbPath)
			if current == previous {
				continue
			}
			previous = current
			if debounceTimer == nil {
				debounceTimer = time.NewTimer(debounce)
				debounceC = debounceTimer.C
			} else {
				if !debounceTimer.Stop() {
					select {
					case <-debounceTimer.C:
					default:
					}
				}
				debounceTimer.Reset(debounce)
			}
		case <-debounceC:
			debounceTimer = nil
			debounceC = nil
			emit()
		}
	}
}
