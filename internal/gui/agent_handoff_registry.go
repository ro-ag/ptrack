package gui

import (
	"errors"
	"sort"
	"sync"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
)

const (
	agentHandoffLimit = 64
	agentHandoffTTL   = 30 * time.Minute
)

var (
	ErrAgentHandoffSameRun  = errors.New("agent handoff requires distinct source and target runs")
	ErrAgentHandoffInactive = errors.New("agent handoff requires live source and target runs")
	ErrAgentHandoffStale    = errors.New("agent handoff is stale or invalid")
	ErrAgentHandoffFull     = errors.New("agent handoff inbox is full")
)

type agentHandoffEnvelope struct {
	ID                      string
	Generation              uint64
	SourceRunID             string
	TargetRunID             string
	SourceLifecycleRevision uint64
	TargetLifecycleRevision uint64
	SourceAssociation       *RuntimeAssociation
	TargetAssociation       *RuntimeAssociation
	Preview                 agentrun.HandoffPreview
	CreatedAt               time.Time
	ExpiresAt               time.Time
}

type AgentHandoffEnvelopeV2 struct {
	ID                string                  `json:"id"`
	Generation        uint64                  `json:"generation"`
	SourceRunID       string                  `json:"sourceRunId"`
	TargetRunID       string                  `json:"targetRunId"`
	SourceAssociation *RuntimeAssociation     `json:"sourceAssociation,omitempty"`
	TargetAssociation *RuntimeAssociation     `json:"targetAssociation,omitempty"`
	Preview           agentrun.HandoffPreview `json:"preview"`
	CreatedAt         string                  `json:"createdAt"`
	ExpiresAt         string                  `json:"expiresAt"`
}

type AgentHandoffInbox struct {
	Items      []AgentHandoffEnvelopeV2 `json:"items"`
	Bounds     BoundedSnapshot          `json:"bounds"`
	Incomplete bool                     `json:"incomplete"`
}

type agentHandoffRegistry struct {
	mu      sync.Mutex
	now     func() time.Time
	records map[string]agentHandoffEnvelope
}

func newAgentHandoffRegistry(now func() time.Time) *agentHandoffRegistry {
	if now == nil {
		now = time.Now
	}
	return &agentHandoffRegistry{now: now, records: make(map[string]agentHandoffEnvelope)}
}

func (r *agentHandoffRegistry) add(envelope agentHandoffEnvelope) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.pruneExpiredLocked(r.now())
	if len(r.records) >= agentHandoffLimit {
		return ErrAgentHandoffFull
	}
	r.records[envelope.ID] = cloneAgentHandoffEnvelope(envelope)
	return nil
}

func (r *agentHandoffRegistry) get(id string) (agentHandoffEnvelope, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.pruneExpiredLocked(r.now())
	envelope, exists := r.records[id]
	return cloneAgentHandoffEnvelope(envelope), exists
}

func (r *agentHandoffRegistry) remove(id string) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, exists := r.records[id]; !exists {
		return false
	}
	delete(r.records, id)
	return true
}

func (r *agentHandoffRegistry) snapshot() []agentHandoffEnvelope {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.pruneExpiredLocked(r.now())
	items := make([]agentHandoffEnvelope, 0, len(r.records))
	for _, envelope := range r.records {
		items = append(items, cloneAgentHandoffEnvelope(envelope))
	}
	sort.Slice(items, func(i, j int) bool {
		if !items[i].CreatedAt.Equal(items[j].CreatedAt) {
			return items[i].CreatedAt.After(items[j].CreatedAt)
		}
		return items[i].ID < items[j].ID
	})
	return items
}

func (r *agentHandoffRegistry) pruneExpiredLocked(now time.Time) {
	for id, envelope := range r.records {
		if !now.Before(envelope.ExpiresAt) {
			delete(r.records, id)
		}
	}
}

func cloneAgentHandoffEnvelope(envelope agentHandoffEnvelope) agentHandoffEnvelope {
	envelope.SourceAssociation = cloneRuntimeAssociation(envelope.SourceAssociation)
	envelope.TargetAssociation = cloneRuntimeAssociation(envelope.TargetAssociation)
	envelope.Preview.IncludedEventIDs = append([]string{}, envelope.Preview.IncludedEventIDs...)
	return envelope
}

func projectAgentHandoffEnvelope(envelope agentHandoffEnvelope) AgentHandoffEnvelopeV2 {
	return AgentHandoffEnvelopeV2{
		ID: envelope.ID, Generation: envelope.Generation,
		SourceRunID: envelope.SourceRunID, TargetRunID: envelope.TargetRunID,
		SourceAssociation: cloneRuntimeAssociation(envelope.SourceAssociation),
		TargetAssociation: cloneRuntimeAssociation(envelope.TargetAssociation),
		Preview:           envelope.Preview,
		CreatedAt:         envelope.CreatedAt.UTC().Format(time.RFC3339Nano),
		ExpiresAt:         envelope.ExpiresAt.UTC().Format(time.RFC3339Nano),
	}
}

func buildAgentHandoffInbox(
	workspace *WorkspaceContext,
	projection runtimeProjection,
) AgentHandoffInbox {
	inbox := AgentHandoffInbox{
		Items:      []AgentHandoffEnvelopeV2{},
		Incomplete: projection.agentAnalysisIncomplete || projection.agentBounds.More > 0,
	}
	if workspace == nil || workspace.handoffs == nil {
		inbox.Incomplete = true
		return inbox
	}
	associations := make(map[string]*RuntimeAssociation, len(projection.agentCandidates))
	for _, run := range projection.agentCandidates {
		associations[run.RunID] = run.Association
	}
	for _, envelope := range workspace.handoffs.snapshot() {
		source, sourceExists := projection.exactAgentRuns[envelope.SourceRunID]
		target, targetExists := projection.exactAgentRuns[envelope.TargetRunID]
		if !sourceExists || !targetExists || !handoffEnvelopeMatches(
			envelope,
			source,
			target,
			associations[envelope.SourceRunID],
			associations[envelope.TargetRunID],
		) {
			workspace.handoffs.remove(envelope.ID)
			continue
		}
		inbox.Items = append(inbox.Items, projectAgentHandoffEnvelope(envelope))
	}
	inbox.Bounds = snapshotBound(len(inbox.Items), len(inbox.Items))
	return inbox
}
