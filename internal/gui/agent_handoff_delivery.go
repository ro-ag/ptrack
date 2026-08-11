package gui

import (
	"errors"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
)

type agentHandoffExactRegistry interface {
	agentIntelligenceRegistry
	WithExactRuntimeSnapshot(int, func([]agentrun.Run) error) error
}

type AgentHandoffAcknowledgementV2 struct {
	Generation uint64 `json:"generation"`
	ID         string `json:"id"`
	Removed    bool   `json:"removed"`
}

// SendAgentHandoffV2 creates an immutable, memory-only proposal after exact
// source and target validation. It delivers no provider input or authority.
func (a *App) SendAgentHandoffV2(
	generation uint64,
	sourceRunID string,
	targetRunID string,
	expectedSourceAssociationRevision uint64,
	expectedTargetAssociationRevision uint64,
) (AgentHandoffEnvelopeV2, error) {
	if sourceRunID == "" || targetRunID == "" || sourceRunID == targetRunID {
		return AgentHandoffEnvelopeV2{}, ErrAgentHandoffSameRun
	}
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return AgentHandoffEnvelopeV2{}, err
	}
	defer release()
	defer s.Close()
	registry, ok := workspace.agents.(agentHandoffExactRegistry)
	if !ok {
		return AgentHandoffEnvelopeV2{}, errors.New("AgentRun handoff delivery is unavailable")
	}
	host, err := workspaceAssociationHost(workspace, s)
	if err != nil {
		return AgentHandoffEnvelopeV2{}, err
	}
	workspace.associationMu.Lock()
	defer workspace.associationMu.Unlock()
	source, target, sourceAssociation, targetAssociation, err := exactHandoffPair(
		registry, host, sourceRunID, targetRunID,
	)
	if err != nil {
		return AgentHandoffEnvelopeV2{}, err
	}
	if runtimeAssociationRevision(sourceAssociation) != expectedSourceAssociationRevision ||
		runtimeAssociationRevision(targetAssociation) != expectedTargetAssociationRevision {
		return AgentHandoffEnvelopeV2{}, ErrAgentHandoffStale
	}
	previewRun, events, _, _, err := registry.IntelligenceSnapshot(
		sourceRunID,
		agentIntelligenceEventLimit,
	)
	if err != nil {
		return AgentHandoffEnvelopeV2{}, err
	}
	if !exactAgentEvidenceSnapshot(source, previewRun) {
		return AgentHandoffEnvelopeV2{}, ErrAgentHandoffStale
	}
	if sourceAssociation == nil {
		previewRun.Association = nil
	}
	// Revalidate after preview construction so exit, stale revival, or
	// reassociation cannot race proposal publication.
	currentSource, currentTarget, currentSourceAssociation,
		currentTargetAssociation, err := exactHandoffPair(
		registry, host, sourceRunID, targetRunID,
	)
	if err != nil || source.LifecycleRevision != currentSource.LifecycleRevision ||
		target.LifecycleRevision != currentTarget.LifecycleRevision ||
		!runtimeAssociationsEqualOrNil(sourceAssociation, currentSourceAssociation) ||
		!runtimeAssociationsEqualOrNil(targetAssociation, currentTargetAssociation) {
		return AgentHandoffEnvelopeV2{}, ErrAgentHandoffStale
	}
	id, err := randomWorkspaceToken()
	if err != nil {
		return AgentHandoffEnvelopeV2{}, err
	}
	now := workspace.handoffs.now().UTC()
	envelope := agentHandoffEnvelope{
		ID: id, Generation: workspace.Generation(),
		SourceRunID: sourceRunID, TargetRunID: targetRunID,
		SourceLifecycleRevision: source.LifecycleRevision,
		TargetLifecycleRevision: target.LifecycleRevision,
		SourceAssociation:       cloneRuntimeAssociation(sourceAssociation),
		TargetAssociation:       cloneRuntimeAssociation(targetAssociation),
		Preview:                 agentrun.BuildHandoffPreview(previewRun, events),
		CreatedAt:               now, ExpiresAt: now.Add(agentHandoffTTL),
	}
	if err := workspace.handoffs.add(envelope); err != nil {
		return AgentHandoffEnvelopeV2{}, err
	}
	workspace.bumpResourceRevision()
	a.publishWorkspaceRuntimeChanged(workspace)
	return projectAgentHandoffEnvelope(envelope), nil
}

// AcknowledgeAgentHandoffV2 removes a proposal only for its exact target after
// both runs and associations are revalidated. It changes no project state.
func (a *App) AcknowledgeAgentHandoffV2(
	generation uint64,
	id string,
	targetRunID string,
) (AgentHandoffAcknowledgementV2, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return AgentHandoffAcknowledgementV2{}, err
	}
	defer release()
	defer s.Close()
	envelope, exists := workspace.handoffs.get(id)
	if !exists || envelope.Generation != workspace.Generation() ||
		envelope.TargetRunID != targetRunID {
		return AgentHandoffAcknowledgementV2{}, ErrAgentHandoffStale
	}
	registry, ok := workspace.agents.(agentHandoffExactRegistry)
	if !ok {
		return AgentHandoffAcknowledgementV2{}, errors.New("AgentRun handoff delivery is unavailable")
	}
	host, err := workspaceAssociationHost(workspace, s)
	if err != nil {
		return AgentHandoffAcknowledgementV2{}, err
	}
	workspace.associationMu.Lock()
	defer workspace.associationMu.Unlock()
	source, target, sourceAssociation, targetAssociation, err := exactHandoffPair(
		registry, host, envelope.SourceRunID, envelope.TargetRunID,
	)
	if err != nil || !handoffEnvelopeMatches(
		envelope, source, target, sourceAssociation, targetAssociation,
	) {
		workspace.handoffs.remove(id)
		return AgentHandoffAcknowledgementV2{}, ErrAgentHandoffStale
	}
	if !workspace.handoffs.remove(id) {
		return AgentHandoffAcknowledgementV2{}, ErrAgentHandoffStale
	}
	workspace.bumpResourceRevision()
	a.publishWorkspaceRuntimeChanged(workspace)
	return AgentHandoffAcknowledgementV2{
		Generation: workspace.Generation(), ID: id, Removed: true,
	}, nil
}

func exactHandoffPair(
	registry agentHandoffExactRegistry,
	host *association.Host,
	sourceRunID string,
	targetRunID string,
) (agentrun.Run, agentrun.Run, *RuntimeAssociation, *RuntimeAssociation, error) {
	var source, target agentrun.Run
	err := registry.WithExactRuntimeSnapshot(linkedRuntimeCandidateLimit, func(runs []agentrun.Run) error {
		for _, run := range runs {
			switch run.ID {
			case sourceRunID:
				source = run
			case targetRunID:
				target = run
			}
		}
		return nil
	})
	if err != nil {
		return agentrun.Run{}, agentrun.Run{}, nil, nil, err
	}
	if source.ID == "" || target.ID == "" ||
		!agentRunIsLive(source) || !agentRunIsLive(target) {
		return agentrun.Run{}, agentrun.Run{}, nil, nil, ErrAgentHandoffInactive
	}
	sourceAssociation, err := validatedHandoffAssociation(host, source)
	if err != nil {
		return agentrun.Run{}, agentrun.Run{}, nil, nil, err
	}
	targetAssociation, err := validatedHandoffAssociation(host, target)
	if err != nil {
		return agentrun.Run{}, agentrun.Run{}, nil, nil, err
	}
	return source, target, sourceAssociation, targetAssociation, nil
}

func validatedHandoffAssociation(
	host *association.Host,
	run agentrun.Run,
) (*RuntimeAssociation, error) {
	current := currentRuntimeAssociation(host, run.ID, run.Association)
	if run.Association != nil && current == nil {
		return nil, ErrAgentHandoffStale
	}
	return current, nil
}

func runtimeAssociationsEqualOrNil(left, right *RuntimeAssociation) bool {
	return (left == nil && right == nil) || runtimeAssociationsEqual(left, right)
}

func runtimeAssociationRevision(current *RuntimeAssociation) uint64 {
	if current == nil {
		return 0
	}
	return current.Revision
}

func handoffEnvelopeMatches(
	envelope agentHandoffEnvelope,
	source agentrun.Run,
	target agentrun.Run,
	sourceAssociation *RuntimeAssociation,
	targetAssociation *RuntimeAssociation,
) bool {
	return envelope.SourceRunID == source.ID && envelope.TargetRunID == target.ID &&
		envelope.SourceLifecycleRevision == source.LifecycleRevision &&
		envelope.TargetLifecycleRevision == target.LifecycleRevision &&
		runtimeAssociationsEqualOrNil(envelope.SourceAssociation, sourceAssociation) &&
		runtimeAssociationsEqualOrNil(envelope.TargetAssociation, targetAssociation)
}
