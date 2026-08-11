package gui

import (
	"errors"

	"github.com/ro-ag/ptrack/internal/agentrun"
)

type AgentHandoffV2 struct {
	Generation  uint64                  `json:"generation"`
	RunID       string                  `json:"runId"`
	Association *RuntimeAssociation     `json:"association,omitempty"`
	Preview     agentrun.HandoffPreview `json:"preview"`
	EventBounds BoundedSnapshot         `json:"eventBounds"`
}

// PreviewAgentHandoffV2 explicitly generates a bounded preview. It never
// writes project summary, notes, task state, issues, or AgentRun history.
func (a *App) PreviewAgentHandoffV2(
	generation uint64,
	runID string,
) (AgentHandoffV2, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return AgentHandoffV2{}, err
	}
	defer release()
	defer s.Close()
	registry, ok := workspace.agents.(agentIntelligenceRegistry)
	if !ok {
		return AgentHandoffV2{}, errors.New("AgentRun handoff preview is unavailable")
	}
	host, err := workspaceAssociationHost(workspace, s)
	if err != nil {
		return AgentHandoffV2{}, err
	}
	workspace.associationMu.Lock()
	run, events, total, _, err := registry.IntelligenceSnapshot(
		runID,
		agentIntelligenceEventLimit,
	)
	if err != nil {
		workspace.associationMu.Unlock()
		return AgentHandoffV2{}, err
	}
	association := currentRuntimeAssociation(host, run.ID, run.Association)
	if association == nil {
		run.Association = nil
	}
	workspace.associationMu.Unlock()
	return AgentHandoffV2{
		Generation:  workspace.Generation(),
		RunID:       run.ID,
		Association: association,
		Preview:     agentrun.BuildHandoffPreview(run, events),
		EventBounds: snapshotBound(len(events), total),
	}, nil
}
