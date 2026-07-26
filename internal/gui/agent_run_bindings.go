package gui

import (
	"errors"

	"github.com/ro-ag/ptrack/internal/agentrun"
)

const agentRunSnapshotLimit = 64

type AgentRunsV2 struct {
	Generation uint64         `json:"generation"`
	Runs       []agentrun.Run `json:"runs"`
}

func (a *App) GetAgentRunsV2(generation uint64) (AgentRunsV2, error) {
	workspace, err := a.currentWorkspace(generation)
	if err != nil {
		return AgentRunsV2{}, err
	}
	release, err := workspace.beginOperation(generation, false)
	if err != nil {
		return AgentRunsV2{}, err
	}
	defer release()
	registry := workspace.agentRegistry()
	if registry == nil {
		return AgentRunsV2{}, errors.New("AgentRun registry is unavailable")
	}
	return AgentRunsV2{
		Generation: workspace.Generation(),
		Runs:       registry.Snapshot(agentRunSnapshotLimit),
	}, nil
}
