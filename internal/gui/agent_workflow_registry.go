package gui

import (
	"errors"
	"sort"
	"sync"
	"time"

	"github.com/ro-ag/ptrack/internal/gitinfo"
)

const (
	agentWorkflowLimit = 64
	agentWorkflowTTL   = 5 * time.Minute
)

type AgentWorkflowKind string

const (
	AgentWorkflowValidation  AgentWorkflowKind = "validation"
	AgentWorkflowCommit      AgentWorkflowKind = "commit"
	AgentWorkflowPullRequest AgentWorkflowKind = "pullRequest"
	AgentWorkflowMerge       AgentWorkflowKind = "merge"
)

type AgentWorkflowState string

const (
	AgentWorkflowProposed AgentWorkflowState = "proposed"
	AgentWorkflowApproved AgentWorkflowState = "approved"
)

var (
	ErrAgentWorkflowKind     = errors.New("unsupported agent workflow kind")
	ErrAgentWorkflowTarget   = errors.New("workflow target branch is unavailable")
	ErrAgentWorkflowInactive = errors.New("workflow requires a live run")
	ErrAgentWorkflowStale    = errors.New("agent workflow is stale or invalid")
	ErrAgentWorkflowApproved = errors.New("agent workflow was already approved")
	ErrAgentWorkflowFull     = errors.New("agent workflow inbox is full")
)

type agentWorkflowGitBinding struct {
	Root         string
	GitDir       string
	CommonGitDir string
	Branch       string
	Head         string
	Digest       string
	TargetBranch string
	TargetHead   string
	Status       AgentWorkflowStatus
}

type agentWorkflowProposal struct {
	ID                string
	Generation        uint64
	Kind              AgentWorkflowKind
	State             AgentWorkflowState
	RunID             string
	LifecycleRevision uint64
	Association       *RuntimeAssociation
	Worktree          *gitinfo.WorktreeIdentity
	Git               agentWorkflowGitBinding
	CreatedAt         time.Time
	ExpiresAt         time.Time
	ApprovedAt        time.Time
}

type agentWorkflowRegistry struct {
	mu      sync.Mutex
	now     func() time.Time
	records map[string]agentWorkflowProposal
}

func newAgentWorkflowRegistry(now func() time.Time) *agentWorkflowRegistry {
	if now == nil {
		now = time.Now
	}
	return &agentWorkflowRegistry{now: now, records: make(map[string]agentWorkflowProposal)}
}

func (r *agentWorkflowRegistry) add(proposal agentWorkflowProposal) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.pruneLocked(r.now())
	if len(r.records) >= agentWorkflowLimit {
		return ErrAgentWorkflowFull
	}
	r.records[proposal.ID] = cloneAgentWorkflowProposal(proposal)
	return nil
}

func (r *agentWorkflowRegistry) get(id string) (agentWorkflowProposal, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.pruneLocked(r.now())
	proposal, exists := r.records[id]
	return cloneAgentWorkflowProposal(proposal), exists
}

func (r *agentWorkflowRegistry) approve(id string, approvedAt time.Time) (agentWorkflowProposal, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.pruneLocked(r.now())
	proposal, exists := r.records[id]
	if !exists {
		return agentWorkflowProposal{}, ErrAgentWorkflowStale
	}
	if proposal.State == AgentWorkflowApproved {
		return agentWorkflowProposal{}, ErrAgentWorkflowApproved
	}
	proposal.State = AgentWorkflowApproved
	proposal.ApprovedAt = approvedAt
	r.records[id] = proposal
	return cloneAgentWorkflowProposal(proposal), nil
}

func (r *agentWorkflowRegistry) remove(id string) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, exists := r.records[id]; !exists {
		return false
	}
	delete(r.records, id)
	return true
}

func (r *agentWorkflowRegistry) snapshot() []agentWorkflowProposal {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.pruneLocked(r.now())
	items := make([]agentWorkflowProposal, 0, len(r.records))
	for _, proposal := range r.records {
		items = append(items, cloneAgentWorkflowProposal(proposal))
	}
	sort.Slice(items, func(i, j int) bool {
		if !items[i].CreatedAt.Equal(items[j].CreatedAt) {
			return items[i].CreatedAt.After(items[j].CreatedAt)
		}
		return items[i].ID < items[j].ID
	})
	return items
}

func (r *agentWorkflowRegistry) pruneLocked(now time.Time) {
	for id, proposal := range r.records {
		if !now.Before(proposal.ExpiresAt) {
			delete(r.records, id)
		}
	}
}

func cloneAgentWorkflowProposal(proposal agentWorkflowProposal) agentWorkflowProposal {
	proposal.Association = cloneRuntimeAssociation(proposal.Association)
	if proposal.Worktree != nil {
		copy := *proposal.Worktree
		proposal.Worktree = &copy
	}
	return proposal
}
