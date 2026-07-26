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
)

const (
	defaultLeaseDuration = 30 * time.Second
	defaultSweepInterval = 5 * time.Second
	defaultSnapshotLimit = 64
	defaultMaxRecords    = 1_024
)

var (
	ErrInvalidLease    = errors.New("invalid AgentRun lease")
	ErrRunNotFound     = errors.New("AgentRun not found")
	ErrRegistryClosed  = errors.New("AgentRun registry is closed")
	ErrRegistryFull    = errors.New("AgentRun registry is full")
	ErrAdmissionFenced = errors.New("AgentRun admission is fenced")
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
	PlanID     uint64
	TaskID     uint64
	TerminalID string
	CWD        string
}

type Exit struct {
	Code       int       `json:"code"`
	Result     string    `json:"result"`
	OccurredAt time.Time `json:"occurredAt"`
}

type Run struct {
	ID              string           `json:"id"`
	Profile         string           `json:"profile"`
	Provider        string           `json:"provider"`
	PID             int              `json:"pid"`
	ProcessState    ProcessState     `json:"processState"`
	LeaseState      LeaseState       `json:"leaseState"`
	ProjectRoot     string           `json:"projectRoot"`
	PlanID          uint64           `json:"planId"`
	TaskID          uint64           `json:"taskId"`
	TerminalID      string           `json:"terminalId"`
	CWD             string           `json:"cwd"`
	StartedAt       time.Time        `json:"startedAt"`
	LastActivityAt  time.Time        `json:"lastActivityAt"`
	LastHeartbeatAt time.Time        `json:"lastHeartbeatAt"`
	State           State            `json:"state"`
	Exit            *Exit            `json:"exit,omitempty"`
	Kind            RegistrationKind `json:"registrationKind"`
}

type Lease struct {
	Run        Run
	LeaseToken string
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
}

type record struct {
	run        Run
	leaseToken string
}

type Registry struct {
	projectRoot   string
	leaseDuration time.Duration
	now           func() time.Time
	ticker        Ticker
	maxRecords    int

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
		ctx:           ctx,
		cancel:        cancel,
		records:       make(map[string]*record),
		shutdownDone:  make(chan struct{}),
	}
	go registry.runSweeper()
	return registry
}

func (r *Registry) RegisterLaunched(registration Registration) (Run, error) {
	if registration.PID <= 0 || registration.TerminalID == "" {
		return Run{}, errors.New("launched AgentRun requires PID and terminal")
	}
	return r.register(registration, RegistrationLaunched)
}

func (r *Registry) RegisterExternal(registration Registration) (Lease, error) {
	run, err := r.register(registration, RegistrationExternal)
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
		PlanID:          registration.PlanID,
		TaskID:          registration.TaskID,
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
	r.records[id] = &record{run: run, leaseToken: token}
	return cloneRun(run), nil
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
	now := r.now()
	entry.run.LastHeartbeatAt = now
	entry.run.LastActivityAt = now
	entry.run.LeaseState = LeaseActive
	entry.run.State = StateRunning
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
	return nil
}

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

func (r *Registry) RecordTerminalExit(terminalID string, code int, result string) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, entry := range r.records {
		if entry.run.Kind == RegistrationLaunched &&
			entry.run.TerminalID == terminalID {
			r.recordExitLocked(entry, code, result)
			return true
		}
	}
	return false
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
	now := r.now()
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
		}
	}
}

func (r *Registry) Snapshot(limit int) []Run {
	if limit <= 0 || limit > defaultSnapshotLimit {
		limit = defaultSnapshotLimit
	}
	r.mu.Lock()
	runs := make([]Run, 0, len(r.records))
	for _, entry := range r.records {
		runs = append(runs, cloneRun(entry.run))
	}
	r.mu.Unlock()
	sort.SliceStable(runs, func(i, j int) bool {
		return runs[i].LastActivityAt.After(runs[j].LastActivityAt)
	})
	if len(runs) > limit {
		runs = runs[:limit]
	}
	return runs
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

func (r *Registry) Shutdown(ctx context.Context) error {
	r.shutdownOnce.Do(func() {
		r.mu.Lock()
		r.closed = true
		r.mu.Unlock()
		r.cancel()
	})
	select {
	case <-r.shutdownDone:
		return nil
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
	return run
}
