package gui

import (
	"errors"
)

type AgentRunsV2 struct {
	Generation uint64                `json:"generation"`
	Runs       []AgentRuntimeSummary `json:"runs"`
	Bounds     BoundedSnapshot       `json:"bounds"`
}

func (a *App) GetAgentRunsV2(generation uint64) (AgentRunsV2, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return AgentRunsV2{}, err
	}
	defer release()
	defer s.Close()
	if workspace.agentRegistry() == nil {
		return AgentRunsV2{}, errors.New("AgentRun registry is unavailable")
	}
	projection, err := workspaceRuntimeProjection(s, workspace)
	if err != nil {
		return AgentRunsV2{}, err
	}
	return AgentRunsV2{
		Generation: workspace.Generation(),
		Runs:       projection.agents,
		Bounds:     projection.agentBounds,
	}, nil
}
