package gui

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"sync"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
	"github.com/ro-ag/ptrack/internal/terminal"
)

const (
	taskTransitionConfirmationTTL   = 90 * time.Second
	taskTransitionConfirmationLimit = 64
	taskTransitionResourceLimit     = 1_024
)

var (
	ErrTaskTransitionConfirmationRequired = errors.New("task transition confirmation is required")
	ErrTaskTransitionConfirmationInvalid  = errors.New("task transition confirmation is invalid or stale")
	ErrTaskTransitionAdmissionPending     = errors.New("task transition must retry after resource admission completes")
)

// TaskTransitionConfirmationV3 contains only the exact active counts needed
// for user confirmation. Resource identities and association revisions stay
// backend-owned inside the opaque challenge.
type TaskTransitionConfirmationV3 struct {
	Token           string `json:"token"`
	ExpiresAt       string `json:"expiresAt"`
	ActiveTerminals int    `json:"activeTerminals"`
	ActiveAgents    int    `json:"activeAgents"`
}

// TaskTransitionResultV3 is returned by both challenge and commit phases.
type TaskTransitionResultV3 struct {
	Generation           uint64                        `json:"generation"`
	TaskID               uint64                        `json:"taskId"`
	FromStatus           string                        `json:"fromStatus"`
	ToStatus             string                        `json:"toStatus"`
	Applied              bool                          `json:"applied"`
	RequiresConfirmation bool                          `json:"requiresConfirmation"`
	Confirmation         *TaskTransitionConfirmationV3 `json:"confirmation,omitempty"`
}

type taskTransitionChallenge struct {
	Generation       uint64
	TaskID           uint64
	PlanID           uint64
	FromStatus       model.TaskStatus
	ToStatus         model.TaskStatus
	TaskUpdatedAt    time.Time
	ResourceRevision uint64
	ResourceDigest   [sha256.Size]byte
	ActiveTerminals  int
	ActiveAgents     int
	IssuedAt         time.Time
	ExpiresAt        time.Time
}

type taskTransitionChallengeRegistry struct {
	mu      sync.Mutex
	now     func() time.Time
	records map[string]taskTransitionChallenge
}

func newTaskTransitionChallengeRegistry(now func() time.Time) *taskTransitionChallengeRegistry {
	if now == nil {
		now = time.Now
	}
	return &taskTransitionChallengeRegistry{
		now: now, records: make(map[string]taskTransitionChallenge),
	}
}

func (r *taskTransitionChallengeRegistry) issue(
	record taskTransitionChallenge,
) (string, time.Time, error) {
	if r == nil {
		return "", time.Time{}, errors.New("task transition challenge registry is unavailable")
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	now := r.now()
	r.pruneExpiredLocked(now)
	if len(r.records) >= taskTransitionConfirmationLimit {
		var oldestToken string
		var oldest time.Time
		for token, candidate := range r.records {
			if oldestToken == "" || candidate.IssuedAt.Before(oldest) ||
				(candidate.IssuedAt.Equal(oldest) && token < oldestToken) {
				oldestToken = token
				oldest = candidate.IssuedAt
			}
		}
		delete(r.records, oldestToken)
	}
	for attempts := 0; attempts < 4; attempts++ {
		bytes := make([]byte, 32)
		if _, err := rand.Read(bytes); err != nil {
			return "", time.Time{}, fmt.Errorf("create task transition confirmation: %w", err)
		}
		token := base64.RawURLEncoding.EncodeToString(bytes)
		if _, collision := r.records[token]; collision {
			continue
		}
		record.IssuedAt = now
		record.ExpiresAt = now.Add(taskTransitionConfirmationTTL)
		r.records[token] = record
		return token, record.ExpiresAt, nil
	}
	return "", time.Time{}, errors.New("create unique task transition confirmation")
}

func (r *taskTransitionChallengeRegistry) consume(
	token string,
	generation uint64,
	taskID uint64,
	toStatus model.TaskStatus,
) (taskTransitionChallenge, error) {
	if r == nil || token == "" || len(token) > 128 {
		return taskTransitionChallenge{}, ErrTaskTransitionConfirmationInvalid
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	now := r.now()
	r.pruneExpiredLocked(now)
	record, exists := r.records[token]
	if exists {
		delete(r.records, token)
	}
	if !exists || !now.Before(record.ExpiresAt) ||
		record.Generation != generation || record.TaskID != taskID ||
		record.ToStatus != toStatus {
		return taskTransitionChallenge{}, ErrTaskTransitionConfirmationInvalid
	}
	return record, nil
}

func (r *taskTransitionChallengeRegistry) pruneExpiredLocked(now time.Time) {
	for token, record := range r.records {
		if !now.Before(record.ExpiresAt) {
			delete(r.records, token)
		}
	}
}

type activeTaskResource struct {
	Kind              string `json:"kind"`
	ID                string `json:"id"`
	Revision          uint64 `json:"revision"`
	State             string `json:"state"`
	ProcessState      string `json:"processState,omitempty"`
	LeaseState        string `json:"leaseState,omitempty"`
	LifecycleRevision uint64 `json:"lifecycleRevision,omitempty"`
}

type activeTaskResourceSet struct {
	Digest    [sha256.Size]byte
	Terminals int
	Agents    int
}

func activeTaskResources(
	host *association.Host,
	taskID uint64,
	sessions []terminal.SessionInfo,
	runs []agentrun.Run,
) (activeTaskResourceSet, error) {
	resources := make([]activeTaskResource, 0, len(sessions)+len(runs))
	set := activeTaskResourceSet{}
	for _, session := range sessions {
		current := currentRuntimeAssociation(host, session.ID, session.Association)
		if !terminalStateIsLive(session.State) || current == nil || current.TaskID != taskID {
			continue
		}
		set.Terminals++
		resources = append(resources, activeTaskResource{
			Kind: "terminal", ID: session.ID, Revision: current.Revision,
			State: string(session.State),
		})
	}
	for _, run := range runs {
		current := currentRuntimeAssociation(host, run.ID, run.Association)
		if !agentRunIsLive(run) || current == nil || current.TaskID != taskID {
			continue
		}
		set.Agents++
		resources = append(resources, activeTaskResource{
			Kind: "agent", ID: run.ID, Revision: current.Revision,
			State: string(run.State), ProcessState: string(run.ProcessState),
			LeaseState: string(run.LeaseState), LifecycleRevision: run.LifecycleRevision,
		})
	}
	sort.Slice(resources, func(i, j int) bool {
		if resources[i].Kind == resources[j].Kind {
			return resources[i].ID < resources[j].ID
		}
		return resources[i].Kind < resources[j].Kind
	})
	encoded, err := json.Marshal(resources)
	if err != nil {
		return activeTaskResourceSet{}, err
	}
	set.Digest = sha256.Sum256(encoded)
	return set, nil
}

// withExactTaskResources holds terminal and AgentRun lifecycle locks while the
// callback validates and, on confirmation, commits the task status CAS.
// workspace.associationMu must be held by the caller.
func withExactTaskResources(
	workspace *WorkspaceContext,
	s *store.Store,
	taskID uint64,
	use func(activeTaskResourceSet) error,
) error {
	host, err := workspaceAssociationHost(workspace, s)
	if err != nil {
		return err
	}
	withRuns := func(sessions []terminal.SessionInfo) error {
		registry := workspace.agentRegistry()
		if registry == nil {
			set, buildErr := activeTaskResources(host, taskID, sessions, nil)
			if buildErr != nil {
				return buildErr
			}
			return use(set)
		}
		return registry.WithExactRuntimeSnapshot(
			taskTransitionResourceLimit,
			func(runs []agentrun.Run) error {
				set, buildErr := activeTaskResources(host, taskID, sessions, runs)
				if buildErr != nil {
					return buildErr
				}
				return use(set)
			},
		)
	}
	manager := workspace.terminalManager()
	if manager == nil {
		return withRuns(nil)
	}
	exact, ok := manager.(terminalExactSnapshotManager)
	if !ok {
		return errors.New("exact terminal resource snapshot is unavailable")
	}
	return exact.WithExactSessionSnapshot(taskTransitionResourceLimit, withRuns)
}

func validTaskTransitionStatus(status string) (model.TaskStatus, error) {
	wanted := model.TaskStatus(status)
	for _, candidate := range statuses {
		if wanted == candidate {
			return wanted, nil
		}
	}
	return "", fmt.Errorf("invalid task status %q", status)
}

// MoveTaskV3 centralizes every confirmation-aware task status transition.
func (a *App) MoveTaskV3(
	generation uint64,
	taskID uint64,
	status string,
	confirmationToken string,
) (TaskTransitionResultV3, error) {
	wanted, err := validTaskTransitionStatus(status)
	if err != nil {
		return TaskTransitionResultV3{}, err
	}
	workspace, err := a.currentWorkspace(generation)
	if err != nil {
		return TaskTransitionResultV3{}, err
	}
	release, err := workspace.beginOperation(generation, false)
	if err != nil {
		return TaskTransitionResultV3{}, err
	}
	defer release()
	releaseAdmissionFence := workspace.fenceResourceAdmission()
	defer releaseAdmissionFence()
	if workspace.pendingResourceAdmissions() > 0 {
		return TaskTransitionResultV3{}, ErrTaskTransitionAdmissionPending
	}
	s, err := store.Open(workspace.dbPath)
	if err != nil {
		return TaskTransitionResultV3{}, err
	}
	defer s.Close()
	workspace.associationMu.Lock()
	defer workspace.associationMu.Unlock()
	if workspace.taskTransitions == nil {
		workspace.taskTransitions = newTaskTransitionChallengeRegistry(nil)
	}
	if confirmationToken != "" {
		return confirmTaskTransition(
			s, workspace, taskID, wanted, confirmationToken,
		)
	}
	return beginTaskTransition(s, workspace, taskID, wanted)
}

func beginTaskTransition(
	s *store.Store,
	workspace *WorkspaceContext,
	taskID uint64,
	wanted model.TaskStatus,
) (TaskTransitionResultV3, error) {
	resourceRevision := workspace.resourceRevisionValue()
	task, err := s.GetTask(taskID)
	if err != nil {
		if errors.Is(err, store.ErrNotFound) {
			return TaskTransitionResultV3{}, fmt.Errorf("task #%d not found", taskID)
		}
		return TaskTransitionResultV3{}, err
	}
	base := TaskTransitionResultV3{
		Generation: workspace.Generation(), TaskID: taskID,
		FromStatus: string(task.Status), ToStatus: string(wanted),
	}
	if task.Status == wanted {
		base.Applied = true
		return base, nil
	}
	err = withExactTaskResources(
		workspace,
		s,
		taskID,
		func(resources activeTaskResourceSet) error {
			if resources.Terminals == 0 && resources.Agents == 0 {
				_, transitionErr := s.CompareAndSetTaskStatus(
					taskID, task.PlanID, task.Status, task.UpdatedAt, wanted,
				)
				if transitionErr == nil {
					base.Applied = true
				}
				return transitionErr
			}
			record := taskTransitionChallenge{
				Generation: workspace.Generation(), TaskID: taskID,
				PlanID:     task.PlanID,
				FromStatus: task.Status, ToStatus: wanted,
				TaskUpdatedAt:    task.UpdatedAt,
				ResourceRevision: resourceRevision,
				ResourceDigest:   resources.Digest,
				ActiveTerminals:  resources.Terminals, ActiveAgents: resources.Agents,
			}
			token, expiresAt, issueErr := workspace.taskTransitions.issue(record)
			if issueErr != nil {
				return issueErr
			}
			base.RequiresConfirmation = true
			base.Confirmation = &TaskTransitionConfirmationV3{
				Token: token, ExpiresAt: expiresAt.UTC().Format(time.RFC3339Nano),
				ActiveTerminals: resources.Terminals, ActiveAgents: resources.Agents,
			}
			return nil
		},
	)
	return base, err
}

func confirmTaskTransition(
	s *store.Store,
	workspace *WorkspaceContext,
	taskID uint64,
	wanted model.TaskStatus,
	token string,
) (TaskTransitionResultV3, error) {
	record, err := workspace.taskTransitions.consume(
		token, workspace.Generation(), taskID, wanted,
	)
	if err != nil {
		return TaskTransitionResultV3{}, err
	}
	result := TaskTransitionResultV3{
		Generation: workspace.Generation(), TaskID: taskID,
		FromStatus: string(record.FromStatus), ToStatus: string(wanted),
	}
	if workspace.resourceRevisionValue() != record.ResourceRevision {
		return TaskTransitionResultV3{}, ErrTaskTransitionConfirmationInvalid
	}
	err = withExactTaskResources(
		workspace,
		s,
		taskID,
		func(resources activeTaskResourceSet) error {
			if resources.Digest != record.ResourceDigest ||
				resources.Terminals != record.ActiveTerminals ||
				resources.Agents != record.ActiveAgents {
				return ErrTaskTransitionConfirmationInvalid
			}
			_, transitionErr := s.CompareAndSetTaskStatus(
				taskID, record.PlanID, record.FromStatus,
				record.TaskUpdatedAt, wanted,
			)
			if transitionErr == nil {
				result.Applied = true
			}
			return transitionErr
		},
	)
	return result, err
}
