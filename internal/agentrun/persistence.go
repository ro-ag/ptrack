package agentrun

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"time"
)

// Run history persistence
//
// The registry is authoritative in memory, but a bounded snapshot of its
// records is mirrored to <globalHome>/runtime/<sha256(projectRoot)>/
// agent-runs.json so registered runs survive an app restart. The mirror is
// written on state transitions (register, exit, sweep, shutdown) — never on
// heartbeats, which are far too frequent to justify disk I/O. It is advisory:
// an unreadable or corrupt file is ignored and the registry simply starts
// empty, because the live in-memory state is what matters for correctness.
//
// Restart semantics:
//   - A launched run that had not exited is marked stale with an unknown
//     process state: its hosting terminal died with the previous app instance,
//     and the new instance cannot know the outcome.
//   - An external run that had not exited keeps its state and lease token. If
//     the external agent is still alive it can resume heartbeating; otherwise
//     the regular lease sweep marks it stale within one lease duration.

const (
	persistedStateVersion = 1
	runHistoryFileName    = "agent-runs.json"
)

// persistedRegistryState is the on-disk mirror of the registry's records.
type persistedRegistryState struct {
	Version int               `json:"version"`
	SavedAt time.Time         `json:"savedAt"`
	Runs    []persistedRecord `json:"runs"`
}

type persistedRecord struct {
	Run        Run    `json:"run"`
	LeaseToken string `json:"leaseToken,omitempty"`
}

// RuntimeDir returns the private per-project runtime directory under the
// global ptrack home, creating nothing. It is shared by the integration
// descriptor (agent-registry.json) and the run history (agent-runs.json).
func RuntimeDir(globalHome, projectRoot string) (string, error) {
	absHome, err := filepath.Abs(globalHome)
	if err != nil {
		return "", fmt.Errorf("resolve AgentRun runtime home: %w", err)
	}
	absRoot, err := filepath.Abs(projectRoot)
	if err != nil {
		return "", fmt.Errorf("resolve AgentRun project root: %w", err)
	}
	hash := sha256.Sum256([]byte(filepath.Clean(absRoot)))
	return filepath.Join(absHome, "runtime", hex.EncodeToString(hash[:])), nil
}

// RunHistoryPath returns the run-history file location for a project.
func RunHistoryPath(globalHome, projectRoot string) (string, error) {
	dir, err := RuntimeDir(globalHome, projectRoot)
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, runHistoryFileName), nil
}

// restoreLocked loads the persisted history from r.statePath. Missing files
// are normal (first run); unreadable or corrupt files are ignored by the
// caller so a damaged history never blocks the app from starting.
func (r *Registry) restoreLocked() error {
	contents, err := os.ReadFile(r.statePath)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("read AgentRun history: %w", err)
	}
	var state persistedRegistryState
	if err := json.Unmarshal(contents, &state); err != nil {
		return fmt.Errorf("decode AgentRun history: %w", err)
	}
	if state.Version > persistedStateVersion {
		return fmt.Errorf("AgentRun history version %d is newer than supported %d",
			state.Version, persistedStateVersion)
	}

	// Keep at most maxRecords, preferring the most recently active runs.
	sort.SliceStable(state.Runs, func(i, j int) bool {
		return state.Runs[i].Run.LastActivityAt.After(state.Runs[j].Run.LastActivityAt)
	})
	if len(state.Runs) > r.maxRecords {
		state.Runs = state.Runs[:r.maxRecords]
	}

	for _, persisted := range state.Runs {
		run := persisted.Run
		if run.ID == "" {
			continue
		}
		if run.Kind == RegistrationLaunched && run.State != StateExited {
			// The terminal hosting this run died with the previous instance.
			run.State = StateStale
			run.ProcessState = ProcessUnknown
		}
		r.records[run.ID] = &record{run: run, leaseToken: persisted.LeaseToken}
	}
	return nil
}

// saveLocked mirrors the current records to r.statePath. It is best-effort:
// callers ignore the error because the in-memory registry stays authoritative.
// The write is atomic (temp file, fsync, rename) and serialized across
// processes with the runtime directory's flock, matching the descriptor.
func (r *Registry) saveLocked() error {
	if r.statePath == "" {
		return nil
	}
	runs := make([]persistedRecord, 0, len(r.records))
	for _, entry := range r.records {
		runs = append(runs, persistedRecord{
			Run:        cloneRun(entry.run),
			LeaseToken: entry.leaseToken,
		})
	}
	sort.SliceStable(runs, func(i, j int) bool {
		return runs[i].Run.LastActivityAt.After(runs[j].Run.LastActivityAt)
	})
	if len(runs) > r.maxRecords {
		runs = runs[:r.maxRecords]
	}
	state := persistedRegistryState{
		Version: persistedStateVersion,
		SavedAt: r.now(),
		Runs:    runs,
	}

	dir := filepath.Dir(r.statePath)
	if err := preparePrivateRuntimeDir(dir); err != nil {
		return err
	}
	unlock, err := lockPrivateDescriptor(dir)
	if err != nil {
		return err
	}
	defer func() { _ = unlock() }()

	tempToken, err := randomOpaqueValue()
	if err != nil {
		return err
	}
	tempPath := filepath.Join(dir, ".agent-runs-"+tempToken)
	file, err := openPrivateDescriptor(tempPath)
	if err != nil {
		return fmt.Errorf("create AgentRun history: %w", err)
	}
	encodeErr := json.NewEncoder(file).Encode(state)
	syncErr := file.Sync()
	closeErr := file.Close()
	if err := errors.Join(encodeErr, syncErr, closeErr); err != nil {
		_ = os.Remove(tempPath)
		return fmt.Errorf("write AgentRun history: %w", err)
	}
	if err := replacePrivateDescriptor(tempPath, r.statePath); err != nil {
		_ = os.Remove(tempPath)
		return fmt.Errorf("publish AgentRun history: %w", err)
	}
	return securePublishedDescriptor(r.statePath)
}
