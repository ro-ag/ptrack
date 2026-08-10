package agentrun

import (
	"context"
	"crypto/rand"
	"crypto/subtle"
	"encoding/base64"
	"errors"
	"fmt"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/ro-ag/ptrack/internal/association"
)

const (
	defaultLeaseDuration = 30 * time.Second
	defaultSweepInterval = 5 * time.Second
	defaultSnapshotLimit = 64
	defaultMaxRecords    = 1_024
)

var (
	ErrInvalidLease        = errors.New("invalid AgentRun lease")
	ErrRunNotFound         = errors.New("AgentRun not found")
	ErrRegistryClosed      = errors.New("AgentRun registry is closed")
	ErrRegistryFull        = errors.New("AgentRun registry is full")
	ErrAdmissionFenced     = errors.New("AgentRun admission is fenced")
	ErrAssociationMismatch = errors.New("AgentRun association does not correspond to terminal")
	ErrLinkedAssociation   = errors.New("linked AgentRun association requires terminal-paired mutation")
	ErrSnapshotLimit       = errors.New("AgentRun snapshot exceeds exact limit")
)

type RegistrationKind string
type State string
type ProcessState string
type LeaseState string

const (
	RegistrationLaunched RegistrationKind = "launched"
	RegistrationExternal RegistrationKind = "external"

	StateRunning State = "running"
	StateExited  State = "exited"
	StateStale   State = "stale"
	StateUnknown State = "unknown"

	ProcessRunning ProcessState = "running"
	ProcessExited  ProcessState = "exited"
	ProcessUnknown ProcessState = "unknown"

	LeaseNone    LeaseState = "none"
	LeaseActive  LeaseState = "active"
	LeaseExpired LeaseState = "expired"
)

type Registration struct {
	Profile    string
	Provider   string
	PID        int
	TerminalID string
	CWD        string
}

type Exit struct {
	Code       int       `json:"code"`
	Result     string    `json:"result"`
	OccurredAt time.Time `json:"occurredAt"`
}

type Run struct {
	ID              string                     `json:"id"`
	Profile         string                     `json:"profile"`
	Provider        string                     `json:"provider"`
	PID             int                        `json:"pid"`
	ProcessState    ProcessState               `json:"processState"`
	LeaseState      LeaseState                 `json:"leaseState"`
	ProjectRoot     string                     `json:"projectRoot"`
	Association     *association.AssociationV1 `json:"association,omitempty"`
	TerminalID      string                     `json:"terminalId"`
	CWD             string                     `json:"cwd"`
	StartedAt       time.Time                  `json:"startedAt"`
	LastActivityAt  time.Time                  `json:"lastActivityAt"`
	LastHeartbeatAt time.Time                  `json:"lastHeartbeatAt"`
	State           State                      `json:"state"`
	Exit            *Exit                      `json:"exit,omitempty"`
	Kind            RegistrationKind           `json:"registrationKind"`
	// LifecycleRevision is an in-memory host epoch used only for exact
	// compare-and-set decisions. It is never exposed or persisted.
	LifecycleRevision uint64 `json:"-"`
}

type Lease struct {
	Run        Run
	LeaseToken string
}

type LinkedAssociationChange struct {
	RunID      string
	TerminalID string
	Previous   association.AssociationV1
	Next       association.AssociationV1
}

type Ticker interface {
	Channel() <-chan time.Time
	Stop()
}

type realTicker struct {
	value *time.Ticker
}

func (t realTicker) Channel() <-chan time.Time { return t.value.C }
func (t realTicker) Stop()                     { t.value.Stop() }

type Config struct {
	ProjectRoot   string
	LeaseDuration time.Duration
	SweepInterval time.Duration
	Now           func() time.Time
	NewTicker     func(time.Duration) Ticker
	MaxRecords    int
	// StatePath, when non-empty, mirrors a bounded snapshot of the registry to
	// disk (see persistence.go) so registered runs survive an app restart.
	// Empty keeps the registry memory-only.
	StatePath string
}

type record struct {
	run               Run
	leaseToken        string
	lifecycleRevision uint64
	// linkedLaunch is immutable provenance set only by RegisterLinkedLaunched.
	// A later host association on an ordinary run must not make it eligible for
	// the linked-launch rollback path.
	linkedLaunch bool
}

type Registry struct {
	projectRoot   string
	leaseDuration time.Duration
	now           func() time.Time
	ticker        Ticker
	maxRecords    int
	statePath     string

	ctx    context.Context
	cancel context.CancelFunc

	mu              sync.Mutex
	records         map[string]*record
	closed          bool
	admissionFences int

	shutdownOnce sync.Once
	shutdownDone chan struct{}
}

func NewRegistry(config Config) *Registry {
	leaseDuration := config.LeaseDuration
	if leaseDuration <= 0 {
		leaseDuration = defaultLeaseDuration
	}
	sweepInterval := config.SweepInterval
	if sweepInterval <= 0 {
		sweepInterval = defaultSweepInterval
	}
	now := config.Now
	if now == nil {
		now = time.Now
	}
	newTicker := config.NewTicker
	if newTicker == nil {
		newTicker = func(duration time.Duration) Ticker {
			return realTicker{value: time.NewTicker(duration)}
		}
	}
	ctx, cancel := context.WithCancel(context.Background())
	maxRecords := config.MaxRecords
	if maxRecords <= 0 {
		maxRecords = defaultMaxRecords
	}
	registry := &Registry{
		projectRoot:   filepath.Clean(config.ProjectRoot),
		leaseDuration: leaseDuration,
		now:           now,
		ticker:        newTicker(sweepInterval),
		maxRecords:    maxRecords,
		statePath:     config.StatePath,
		ctx:           ctx,
		cancel:        cancel,
		records:       make(map[string]*record),
		shutdownDone:  make(chan struct{}),
	}
	if registry.statePath != "" {
		// Best effort: a missing or damaged history must never block startup.
		// The in-memory registry is authoritative; the file is only a mirror.
		_ = registry.restoreLocked()
	}
	go registry.runSweeper()
	return registry
}

func (r *Registry) RegisterLaunched(registration Registration) (Run, error) {
	if registration.PID <= 0 || registration.TerminalID == "" {
		return Run{}, errors.New("launched AgentRun requires PID and terminal")
	}
	return r.register(registration, RegistrationLaunched, nil, nil)
}

// RegisterLinkedLaunched atomically registers a host-launched run with a
// host-validated association. No detached or partially associated record is
// published when binding fails.
func (r *Registry) RegisterLinkedLaunched(
	registration Registration,
	host *association.Host,
	pointer association.PointerV1,
) (Run, error) {
	if registration.PID <= 0 || registration.TerminalID == "" {
		return Run{}, errors.New("launched AgentRun requires PID and terminal")
	}
	return r.register(registration, RegistrationLaunched, host, &pointer)
}

// RollbackLinkedLaunched removes exactly one host-launched record when the
// surrounding linked-launch transaction cannot be published. External runs
// and records owned by another terminal can never be removed through it.
func (r *Registry) RollbackLinkedLaunched(id, terminalID string) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	entry := r.records[id]
	if entry == nil || !entry.linkedLaunch ||
		entry.run.Kind != RegistrationLaunched ||
		terminalID == "" || entry.run.TerminalID != terminalID {
		return false
	}
	delete(r.records, id)
	_ = r.saveLocked()
	return true
}

// RollbackLinkedTerminal removes host-launched records for one terminal when
// the frontend cannot commit or attach the linked tab after launch returns.
// It never touches external registrations.
func (r *Registry) RollbackLinkedTerminal(terminalID string) int {
	if terminalID == "" {
		return 0
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	removed := 0
	for id, entry := range r.records {
		if entry.linkedLaunch && entry.run.Kind == RegistrationLaunched &&
			entry.run.Association != nil &&
			entry.run.TerminalID == terminalID {
			delete(r.records, id)
			removed++
		}
	}
	if removed > 0 {
		_ = r.saveLocked()
	}
	return removed
}

// HasLinkedTerminal reports whether terminalID has immutable linked-launch
// provenance. It lets callers prove rollback authority before attempting a
// process close without consuming that proof until the close succeeds.
func (r *Registry) HasLinkedTerminal(terminalID string) bool {
	if terminalID == "" {
		return false
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, entry := range r.records {
		if entry.linkedLaunch && entry.run.Kind == RegistrationLaunched &&
			entry.run.Association != nil && entry.run.TerminalID == terminalID {
			return true
		}
	}
	return false
}

func (r *Registry) RegisterExternal(registration Registration) (Lease, error) {
	run, err := r.register(registration, RegistrationExternal, nil, nil)
	if err != nil {
		return Lease{}, err
	}
	r.mu.Lock()
	token := r.records[run.ID].leaseToken
	r.mu.Unlock()
	return Lease{Run: run, LeaseToken: token}, nil
}

func (r *Registry) register(
	registration Registration,
	kind RegistrationKind,
	host *association.Host,
	pointer *association.PointerV1,
) (Run, error) {
	registration.Profile = strings.TrimSpace(registration.Profile)
	registration.Provider = strings.TrimSpace(registration.Provider)
	if registration.Profile == "" || registration.Provider == "" {
		return Run{}, errors.New("AgentRun profile and provider are required")
	}
	if registration.CWD == "" {
		registration.CWD = r.projectRoot
	}
	cwd := filepath.Clean(registration.CWD)
	if !pathWithin(r.projectRoot, cwd) {
		return Run{}, errors.New("AgentRun CWD is outside the project")
	}
	id, err := randomOpaqueValue()
	if err != nil {
		return Run{}, err
	}
	token := ""
	if kind == RegistrationExternal {
		token, err = randomOpaqueValue()
		if err != nil {
			return Run{}, err
		}
	}
	now := r.now()
	run := Run{
		ID:              id,
		Profile:         registration.Profile,
		Provider:        registration.Provider,
		PID:             registration.PID,
		ProcessState:    ProcessUnknown,
		LeaseState:      LeaseActive,
		ProjectRoot:     r.projectRoot,
		TerminalID:      registration.TerminalID,
		CWD:             cwd,
		StartedAt:       now,
		LastActivityAt:  now,
		LastHeartbeatAt: now,
		State:           StateRunning,
		Kind:            kind,
	}
	if kind == RegistrationLaunched {
		run.ProcessState = ProcessRunning
		run.LeaseState = LeaseNone
		run.LastHeartbeatAt = time.Time{}
	}
	if pointer != nil {
		if kind != RegistrationLaunched || host == nil {
			return Run{}, errors.New("linked AgentRun requires a launched host binding")
		}
		bound, bindErr := host.Bind(id, *pointer, nil)
		if bindErr != nil {
			return Run{}, bindErr
		}
		run.Association = &bound
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.closed {
		return Run{}, ErrRegistryClosed
	}
	if r.admissionFences > 0 {
		return Run{}, ErrAdmissionFenced
	}
	if _, collision := r.records[id]; collision {
		return Run{}, errors.New("AgentRun ID collision")
	}
	if len(r.records) >= r.maxRecords && !r.evictInactiveLocked() {
		return Run{}, ErrRegistryFull
	}
	r.records[id] = &record{
		run:               run,
		leaseToken:        token,
		lifecycleRevision: 1,
		linkedLaunch:      kind == RegistrationLaunched && pointer != nil,
	}
	_ = r.saveLocked()
	return cloneRun(run), nil
}

// Associate applies a host-validated pointer to an existing live or historical
// run. Registration inputs never set this field, so external agents cannot
// self-assert project, plan, or task authority.
func (r *Registry) Associate(
	id string,
	host *association.Host,
	pointer association.PointerV1,
) (association.AssociationV1, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	entry := r.records[id]
	if entry == nil {
		return association.AssociationV1{}, ErrRunNotFound
	}
	if entry.linkedLaunch {
		return association.AssociationV1{}, ErrLinkedAssociation
	}
	next, err := host.Bind(entry.run.ID, pointer, entry.run.Association)
	if err != nil {
		return association.AssociationV1{}, err
	}
	entry.run.Association = &next
	_ = r.saveLocked()
	return next, nil
}

// IsLinkedLaunchRun reports immutable host-owned provenance. Association
// presence is deliberately not used because a detached linked launch retains
// an internal project-only association.
func (r *Registry) IsLinkedLaunchRun(runID string) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	entry := r.records[runID]
	return entry != nil && entry.linkedLaunch
}

// PrepareLinkedTerminalAssociationChange prepares the corresponding half of
// a terminal metadata mutation without publishing it. No linked-launch record
// is a valid terminal-only case; a present but mismatched record fails closed.
func (r *Registry) PrepareLinkedTerminalAssociationChange(
	terminalID string,
	terminalPrevious *association.AssociationV1,
	terminalNext association.AssociationV1,
	host *association.Host,
	pointer association.PointerV1,
) (LinkedAssociationChange, bool, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	var match *record
	for _, entry := range r.records {
		if !entry.linkedLaunch || entry.run.Kind != RegistrationLaunched ||
			entry.run.TerminalID != terminalID {
			continue
		}
		if match != nil {
			return LinkedAssociationChange{}, false, ErrAssociationMismatch
		}
		match = entry
	}
	if match == nil {
		return LinkedAssociationChange{}, false, nil
	}
	if match.run.Association == nil || terminalPrevious == nil ||
		!associationsCorrespond(match.run.Association, terminalPrevious) {
		return LinkedAssociationChange{}, false, ErrAssociationMismatch
	}
	next, err := host.Bind(match.run.ID, pointer, match.run.Association)
	if err != nil {
		return LinkedAssociationChange{}, false, err
	}
	if !associationsCorrespond(&next, &terminalNext) {
		return LinkedAssociationChange{}, false, ErrAssociationMismatch
	}
	return LinkedAssociationChange{
		RunID:      match.run.ID,
		TerminalID: terminalID,
		Previous:   *match.run.Association,
		Next:       next,
	}, true, nil
}

func (r *Registry) CommitLinkedAssociationChange(change LinkedAssociationChange) error {
	return r.applyLinkedAssociationChange(change, change.Previous, change.Next)
}

func (r *Registry) RollbackLinkedAssociationChange(change LinkedAssociationChange) error {
	return r.applyLinkedAssociationChange(change, change.Next, change.Previous)
}

func (r *Registry) applyLinkedAssociationChange(
	change LinkedAssociationChange,
	expected association.AssociationV1,
	replacement association.AssociationV1,
) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	entry := r.records[change.RunID]
	if entry == nil || !entry.linkedLaunch ||
		entry.run.Kind != RegistrationLaunched ||
		entry.run.TerminalID != change.TerminalID ||
		entry.run.Association == nil || *entry.run.Association != expected {
		return ErrAssociationMismatch
	}
	next := replacement
	entry.run.Association = &next
	_ = r.saveLocked()
	return nil
}

func associationsCorrespond(left, right *association.AssociationV1) bool {
	return left != nil && right != nil &&
		left.Version == right.Version &&
		left.ProjectRoot == right.ProjectRoot &&
		left.Generation == right.Generation &&
		left.Target == right.Target &&
		left.Revision == right.Revision
}

func (r *Registry) evictInactiveLocked() bool {
	var oldestID string
	var oldestAt time.Time
	for id, entry := range r.records {
		if runIsActive(entry.run) {
			continue
		}
		if oldestID == "" || entry.run.LastActivityAt.Before(oldestAt) {
			oldestID = id
			oldestAt = entry.run.LastActivityAt
		}
	}
	if oldestID == "" {
		return false
	}
	delete(r.records, oldestID)
	return true
}

func runIsActive(run Run) bool {
	if run.Kind == RegistrationLaunched {
		return run.State == StateRunning && run.ProcessState == ProcessRunning
	}
	return run.Kind == RegistrationExternal &&
		run.State == StateRunning &&
		run.LeaseState == LeaseActive
}

func (r *Registry) FenceAdmission() func() {
	r.mu.Lock()
	r.admissionFences++
	r.mu.Unlock()
	var once sync.Once
	return func() {
		once.Do(func() {
			r.mu.Lock()
			if r.admissionFences > 0 {
				r.admissionFences--
			}
			r.mu.Unlock()
		})
	}
}

func (r *Registry) Heartbeat(id, token string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	entry, err := r.externalRecordLocked(id, token)
	if err != nil {
		return err
	}
	if entry.run.State == StateExited {
		return ErrInvalidLease
	}
	wasActive := runIsActive(entry.run)
	now := r.now()
	entry.run.LastHeartbeatAt = now
	entry.run.LastActivityAt = now
	entry.run.LeaseState = LeaseActive
	entry.run.State = StateRunning
	if !wasActive {
		entry.lifecycleRevision++
	}
	return nil
}

func (r *Registry) ExitExternal(id, token string, code int, result string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	entry, err := r.externalRecordLocked(id, token)
	if err != nil {
		return err
	}
	r.recordExitLocked(entry, code, result)
	_ = r.saveLocked()
	return nil
}

// RecordTerminalActivity reports activity on a terminal, returning whether a
// run was updated. At most one launched run is expected to be active per
// terminal — a terminal session hosts one agent process at a time — so the
// newest signal is applied to the first still-running match.
func (r *Registry) RecordTerminalActivity(terminalID string) bool {
	return r.RecordTerminalActivityAt(terminalID, r.now())
}

func (r *Registry) RecordTerminalActivityAt(
	terminalID string,
	activityAt time.Time,
) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, entry := range r.records {
		if entry.run.Kind == RegistrationLaunched &&
			entry.run.TerminalID == terminalID &&
			entry.run.State == StateRunning {
			if activityAt.After(entry.run.LastActivityAt) {
				entry.run.LastActivityAt = activityAt
			}
			return true
		}
	}
	return false
}

// RecordTerminalExit records the exit of every not-yet-exited launched run
// hosted by the terminal, returning whether any launched run matched.
// Normally a terminal hosts a single launched run, but matching is defensive:
// iterating all records means an exited record (from, say, a restarted
// session that reused a terminal ID) can never shadow a still-running one.
func (r *Registry) RecordTerminalExit(terminalID string, code int, result string) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	matched := false
	for _, entry := range r.records {
		if entry.run.Kind != RegistrationLaunched ||
			entry.run.TerminalID != terminalID {
			continue
		}
		matched = true
		if entry.run.State != StateExited {
			r.recordExitLocked(entry, code, result)
		}
	}
	if matched {
		_ = r.saveLocked()
	}
	return matched
}

func (r *Registry) recordExitLocked(entry *record, code int, result string) {
	now := r.now()
	entry.run.State = StateExited
	entry.run.ProcessState = ProcessExited
	if entry.run.Kind == RegistrationExternal {
		entry.run.LeaseState = LeaseExpired
	}
	entry.run.LastActivityAt = now
	entry.run.Exit = &Exit{Code: code, Result: result, OccurredAt: now}
	entry.lifecycleRevision++
}

func (r *Registry) externalRecordLocked(id, token string) (*record, error) {
	entry := r.records[id]
	if entry == nil {
		return nil, ErrRunNotFound
	}
	if entry.run.Kind != RegistrationExternal || token == "" ||
		subtle.ConstantTimeCompare([]byte(entry.leaseToken), []byte(token)) != 1 {
		return nil, ErrInvalidLease
	}
	return entry, nil
}

func (r *Registry) SweepExpired() {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.sweepExpiredLocked(r.now()) {
		_ = r.saveLocked()
	}
}

func (r *Registry) sweepExpiredLocked(now time.Time) bool {
	changed := false
	for _, entry := range r.records {
		if entry.run.Kind != RegistrationExternal ||
			entry.run.State == StateExited ||
			entry.run.LeaseState != LeaseActive {
			continue
		}
		if now.Sub(entry.run.LastHeartbeatAt) > r.leaseDuration {
			entry.run.State = StateStale
			entry.run.ProcessState = ProcessUnknown
			entry.run.LeaseState = LeaseExpired
			entry.lifecycleRevision++
			changed = true
		}
	}
	return changed
}

func (r *Registry) Snapshot(limit int) []Run {
	runs, _ := r.SnapshotBounded(limit)
	return runs
}

// SnapshotBounded returns the same deterministic bounded view as Snapshot
// plus the total number of records considered before truncation.
func (r *Registry) SnapshotBounded(limit int) ([]Run, int) {
	return r.snapshotBounded(limit, defaultSnapshotLimit)
}

// RuntimeSnapshotBounded permits the larger, still hard-bounded candidate
// set used to aggregate per-task runtime state before presentation rows are
// capped. Registry storage itself is bounded by maxRecords.
func (r *Registry) RuntimeSnapshotBounded(limit int) ([]Run, int) {
	return r.snapshotBounded(limit, defaultMaxRecords)
}

// WithExactRuntimeSnapshot holds AgentRun lifecycle/lease/association state
// across one short host-side compare-and-set decision.
func (r *Registry) WithExactRuntimeSnapshot(
	maximum int,
	use func([]Run) error,
) error {
	if maximum <= 0 || use == nil {
		return errors.New("exact AgentRun snapshot callback and limit are required")
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.sweepExpiredLocked(r.now()) {
		_ = r.saveLocked()
	}
	if len(r.records) > maximum {
		return ErrSnapshotLimit
	}
	runs := make([]Run, 0, len(r.records))
	for _, entry := range r.records {
		run := cloneRun(entry.run)
		run.LifecycleRevision = entry.lifecycleRevision
		runs = append(runs, run)
	}
	sort.SliceStable(runs, func(i, j int) bool { return runs[i].ID < runs[j].ID })
	return use(runs)
}

func (r *Registry) snapshotBounded(limit, maximum int) ([]Run, int) {
	if limit <= 0 || limit > maximum {
		limit = maximum
	}
	r.mu.Lock()
	runs := make([]Run, 0, len(r.records))
	for _, entry := range r.records {
		runs = append(runs, cloneRun(entry.run))
	}
	r.mu.Unlock()
	sort.SliceStable(runs, func(i, j int) bool {
		if runs[i].LastActivityAt.Equal(runs[j].LastActivityAt) {
			return runs[i].ID < runs[j].ID
		}
		return runs[i].LastActivityAt.After(runs[j].LastActivityAt)
	})
	total := len(runs)
	if len(runs) > limit {
		runs = runs[:limit]
	}
	return runs, total
}

func (r *Registry) ActiveCount() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	active := 0
	for _, entry := range r.records {
		if runIsActive(entry.run) {
			active++
		}
	}
	return active
}

func (r *Registry) runSweeper() {
	defer close(r.shutdownDone)
	defer r.ticker.Stop()
	for {
		select {
		case <-r.ticker.Channel():
			r.SweepExpired()
		case <-r.ctx.Done():
			return
		}
	}
}

// Shutdown stops the sweeper and persists the final run-history snapshot.
// The save error is joined into the result so callers can surface history
// loss without it masking the shutdown itself.
func (r *Registry) Shutdown(ctx context.Context) error {
	var saveErr error
	r.shutdownOnce.Do(func() {
		r.mu.Lock()
		r.closed = true
		saveErr = r.saveLocked()
		r.mu.Unlock()
		r.cancel()
	})
	select {
	case <-r.shutdownDone:
		return saveErr
	case <-ctx.Done():
		return ctx.Err()
	}
}

func pathWithin(root, path string) bool {
	relative, err := filepath.Rel(filepath.Clean(root), filepath.Clean(path))
	return err == nil && relative != ".." &&
		!strings.HasPrefix(relative, ".."+string(filepath.Separator))
}

func randomOpaqueValue() (string, error) {
	value := make([]byte, 32)
	if _, err := rand.Read(value); err != nil {
		return "", fmt.Errorf("create AgentRun opaque value: %w", err)
	}
	return base64.RawURLEncoding.EncodeToString(value), nil
}

func cloneRun(run Run) Run {
	if run.Exit != nil {
		exit := *run.Exit
		run.Exit = &exit
	}
	if run.Association != nil {
		associationCopy := *run.Association
		run.Association = &associationCopy
	}
	return run
}
