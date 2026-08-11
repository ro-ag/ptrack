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
	persistedStateVersion = 3
	runHistoryFileName    = "agent-runs.json"
)

// ErrHistoryFutureVersion prevents an older process from overwriting a run
// history written by a newer p-track.
var ErrHistoryFutureVersion = errors.New("AgentRun history is newer than supported")

// persistedRegistryState is the on-disk mirror of the registry's records.
type persistedRegistryState struct {
	Version int               `json:"version"`
	SavedAt time.Time         `json:"savedAt"`
	Runs    []persistedRecord `json:"runs"`
}

type persistedRecord struct {
	Run                Run     `json:"run"`
	LeaseToken         string  `json:"leaseToken,omitempty"`
	Events             []Event `json:"events,omitempty"`
	LastSourceSequence uint64  `json:"lastSourceSequence,omitempty"`
	NextHostSequence   uint64  `json:"nextHostSequence,omitempty"`
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

// PublishRuntimeJSON atomically writes a private JSON descriptor in the
// per-project runtime directory. It lets sibling host services reuse the same
// Unix permission and Windows ACL guarantees as AgentRun descriptors.
func PublishRuntimeJSON(globalHome, projectRoot, name string, value any) (string, error) {
	if filepath.Base(name) != name || name == "." || name == "" {
		return "", errors.New("runtime descriptor name must be a base name")
	}
	directory, err := RuntimeDir(globalHome, projectRoot)
	if err != nil {
		return "", err
	}
	if err := preparePrivateRuntimeDir(directory); err != nil {
		return "", err
	}
	unlock, err := lockPrivateDescriptor(directory)
	if err != nil {
		return "", err
	}
	defer func() { _ = unlock() }()
	token, err := randomOpaqueValue()
	if err != nil {
		return "", err
	}
	temporary := filepath.Join(directory, "."+name+"-"+token)
	file, err := openPrivateDescriptor(temporary)
	if err != nil {
		return "", err
	}
	encodeErr := json.NewEncoder(file).Encode(value)
	syncErr := file.Sync()
	closeErr := file.Close()
	if err := errors.Join(encodeErr, syncErr, closeErr); err != nil {
		_ = os.Remove(temporary)
		return "", err
	}
	path := filepath.Join(directory, name)
	if err := replacePrivateDescriptor(temporary, path); err != nil {
		_ = os.Remove(temporary)
		return "", err
	}
	if err := securePublishedDescriptor(path); err != nil {
		return "", err
	}
	return path, nil
}

// RemoveRuntimeFile removes one named per-project runtime descriptor.
func RemoveRuntimeFile(globalHome, projectRoot, name string) error {
	if filepath.Base(name) != name || name == "." || name == "" {
		return errors.New("runtime descriptor name must be a base name")
	}
	directory, err := RuntimeDir(globalHome, projectRoot)
	if err != nil {
		return err
	}
	err = os.Remove(filepath.Join(directory, name))
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	return err
}

// RemoveRuntimeJSONIfEqual removes one descriptor only when it is still the
// exact value published by the caller. The descriptor lock makes the compare
// and remove atomic with respect to PublishRuntimeJSON, so an older workspace
// generation cannot remove a replacement generation's locator.
func RemoveRuntimeJSONIfEqual(globalHome, projectRoot, name string, expected any) error {
	if filepath.Base(name) != name || name == "." || name == "" {
		return errors.New("runtime descriptor name must be a base name")
	}
	directory, err := RuntimeDir(globalHome, projectRoot)
	if err != nil {
		return err
	}
	unlock, err := lockPrivateDescriptor(directory)
	if err != nil {
		return err
	}
	defer func() { _ = unlock() }()

	expectedJSON, err := json.Marshal(expected)
	if err != nil {
		return err
	}
	contents, err := os.ReadFile(filepath.Join(directory, name))
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	var actualJSON json.RawMessage
	if err := json.Unmarshal(contents, &actualJSON); err != nil || !jsonEqual(actualJSON, expectedJSON) {
		return nil
	}
	err = os.Remove(filepath.Join(directory, name))
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	return err
}

func jsonEqual(left, right []byte) bool {
	var leftValue, rightValue any
	if json.Unmarshal(left, &leftValue) != nil || json.Unmarshal(right, &rightValue) != nil {
		return false
	}
	leftJSON, leftErr := json.Marshal(leftValue)
	rightJSON, rightErr := json.Marshal(rightValue)
	return leftErr == nil && rightErr == nil && string(leftJSON) == string(rightJSON)
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
		return fmt.Errorf("%w: version %d exceeds %d",
			ErrHistoryFutureVersion, state.Version, persistedStateVersion)
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
		run.ProjectRoot = canonicalRegistryPath(run.ProjectRoot)
		run.CWD = canonicalRegistryPath(run.CWD)
		if run.ID == "" || run.ProjectRoot != r.projectRoot ||
			!pathWithin(r.projectRoot, run.CWD) {
			continue
		}
		if run.Kind == RegistrationLaunched && run.State != StateExited {
			// The terminal hosting this run died with the previous instance.
			run.State = StateStale
			run.ProcessState = ProcessUnknown
		}
		if run.Exit != nil {
			run.Exit.Result = classifyExitResult(run.Exit.Result, run.Exit.Code)
		}
		// Associations are live, generation-scoped host state. Version 1 history
		// may contain legacy planId/taskId fields; JSON decoding ignores them and
		// every restored run is deliberately detached for the new generation.
		run.Association = nil
		events := r.restoreEventsLocked(run, persisted.Events)
		lastSourceSequence := persisted.LastSourceSequence
		nextHostSequence := persisted.NextHostSequence
		for _, event := range events {
			if event.SourceSequence > lastSourceSequence {
				lastSourceSequence = event.SourceSequence
			}
			if event.HostSequence > nextHostSequence {
				nextHostSequence = event.HostSequence
			}
		}
		r.records[run.ID] = &record{
			run:                run,
			leaseToken:         persisted.LeaseToken,
			lifecycleRevision:  1,
			events:             events,
			lastSourceSequence: lastSourceSequence,
			nextHostSequence:   nextHostSequence,
		}
	}
	// A successful restore receives one normalized rewrite on the next sweep.
	// This durably removes expired/invalid evidence and applies migrations
	// instead of leaving pre-normalized private data on disk.
	r.persistenceDirty = true
	return nil
}

func (r *Registry) restoreEventsLocked(run Run, persisted []Event) []Event {
	if !r.eventPolicy.CollectionEnabled {
		return []Event{}
	}
	validated := make([]Event, 0, len(persisted))
	seenIDs := make(map[string]bool, len(persisted))
	var lastSourceSequence uint64
	var lastHostSequence uint64
	sort.SliceStable(persisted, func(i, j int) bool {
		return persisted[i].HostSequence < persisted[j].HostSequence
	})
	now := r.now()
	for _, event := range persisted {
		event.Correlation.ProjectRoot = canonicalRegistryPath(event.Correlation.ProjectRoot)
		if event.Correlation.RepositoryRoot != "" {
			event.Correlation.RepositoryRoot = canonicalRegistryPath(event.Correlation.RepositoryRoot)
		}
		if event.ModelVersion != EventModelVersion || event.ID == "" ||
			event.RunID != run.ID || event.Provider != run.Provider ||
			event.SourceSequence <= lastSourceSequence ||
			event.HostSequence <= lastHostSequence || seenIDs[event.ID] ||
			event.ObservedAt.IsZero() || event.ObservedAt.After(now.Add(maxEventClockSkew)) ||
			!validPersistedEventCorrelation(
				event.Correlation,
				run,
				r.projectRoot,
				r.repositoryRoot,
			) {
			continue
		}
		normalized, err := NormalizeEventObservation(
			r.projectRoot,
			event.ObservedAt,
			r.eventPolicy,
			EventObservation{
				ModelVersion:   event.ModelVersion,
				SourceID:       event.SourceID,
				SourceSequence: event.SourceSequence,
				Kind:           event.Kind,
				Phase:          event.Phase,
				Outcome:        event.Outcome,
				Subject:        event.Subject,
				Paths:          event.Paths,
				CommitSHA:      event.CommitSHA,
				ExitCode:       event.ExitCode,
				ErrorClass:     event.ErrorClass,
				Summary:        event.Summary,
				OccurredAt:     event.OccurredAt,
			},
		)
		if err != nil {
			continue
		}
		event.SourceID = normalized.SourceID
		event.Subject = normalized.Subject
		event.Paths = normalized.Paths
		event.CommitSHA = normalized.CommitSHA
		event.ExitCode = cloneInt(normalized.ExitCode)
		event.ErrorClass = normalized.ErrorClass
		event.Summary = normalized.Summary
		event.OccurredAt = normalized.OccurredAt
		validated = append(validated, cloneEvent(event))
		seenIDs[event.ID] = true
		lastSourceSequence = event.SourceSequence
		lastHostSequence = event.HostSequence
	}
	retained, err := RetainEvents(validated, now, r.eventPolicy)
	if err != nil {
		return []Event{}
	}
	return retained
}

// saveLocked mirrors the current records to r.statePath. It is best-effort:
// callers ignore the error because the in-memory registry stays authoritative.
// The write is atomic (temp file, fsync, rename) and serialized across
// processes with the runtime directory's flock, matching the descriptor.
func (r *Registry) saveLocked() error {
	if r.statePath == "" {
		return nil
	}
	if !r.persistenceWritable {
		return nil
	}
	if _, err := r.pruneExpiredEventsLocked(r.now()); err != nil {
		return err
	}
	runs := make([]persistedRecord, 0, len(r.records))
	for _, entry := range r.records {
		persistedRun := cloneRun(entry.run)
		persistedRun.Association = nil
		runs = append(runs, persistedRecord{
			Run:                persistedRun,
			LeaseToken:         entry.leaseToken,
			Events:             cloneEvents(entry.events),
			LastSourceSequence: entry.lastSourceSequence,
			NextHostSequence:   entry.nextHostSequence,
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

func cloneEvents(events []Event) []Event {
	clones := make([]Event, 0, len(events))
	for _, event := range events {
		clones = append(clones, cloneEvent(event))
	}
	return clones
}
