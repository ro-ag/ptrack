package gui

import (
	"context"
	"errors"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
)

type workspaceAgentRegistry interface {
	workspaceShutdowner
	ActiveCount() int
	Snapshot(limit int) []agentrun.Run
	SnapshotBounded(limit int) ([]agentrun.Run, int)
	RuntimeSnapshotBounded(limit int) ([]agentrun.Run, int)
	WithExactRuntimeSnapshot(int, func([]agentrun.Run) error) error
	RegisterLaunched(agentrun.Registration) (agentrun.Run, error)
	RegisterLinkedLaunched(
		agentrun.Registration,
		*association.Host,
		association.PointerV1,
	) (agentrun.Run, error)
	RollbackLinkedLaunched(runID, terminalID string) bool
	RollbackLinkedTerminal(terminalID string) int
	HasLinkedTerminal(terminalID string) bool
	IsLinkedLaunchRun(runID string) bool
	Associate(string, *association.Host, association.PointerV1) (association.AssociationV1, error)
	PrepareLinkedTerminalAssociationChange(
		string,
		*association.AssociationV1,
		association.AssociationV1,
		*association.Host,
		association.PointerV1,
	) (agentrun.LinkedAssociationChange, bool, error)
	CommitLinkedAssociationChange(agentrun.LinkedAssociationChange) error
	RollbackLinkedAssociationChange(agentrun.LinkedAssociationChange) error
	RecordTerminalActivity(terminalID string) bool
	RecordTerminalActivityAt(terminalID string, activityAt time.Time) bool
	RecordTerminalExit(terminalID string, code int, result string) bool
}

type workspaceAgentResources struct {
	registry    *agentrun.Registry
	globalHome  string
	root        string
	integration *agentrun.IntegrationServer
}

func newWorkspaceAgentResources(
	root string,
	globalHome string,
) *workspaceAgentResources {
	// The run history lives next to the integration descriptor so registered
	// runs survive app restarts and project switches. A failure to resolve
	// the path (for example an unreadable home) simply disables persistence;
	// the registry stays fully functional in memory.
	statePath, err := agentrun.RunHistoryPath(globalHome, root)
	if err != nil {
		statePath = ""
	}
	return &workspaceAgentResources{
		registry: agentrun.NewRegistry(agentrun.Config{
			ProjectRoot: root,
			StatePath:   statePath,
		}),
		globalHome: globalHome,
		root:       root,
	}
}

func (r *workspaceAgentResources) Activate(generation uint64) error {
	if r.integration != nil {
		return nil
	}
	server, err := agentrun.StartIntegrationServer(r.registry, agentrun.IntegrationConfig{
		GlobalHome:  r.globalHome,
		ProjectRoot: r.root,
		Generation:  generation,
	})
	if err != nil {
		return err
	}
	r.integration = server
	return nil
}

func (r *workspaceAgentResources) ActiveCount() int {
	return r.registry.ActiveCount()
}

func (r *workspaceAgentResources) FenceAdmission() func() {
	return r.registry.FenceAdmission()
}

func (r *workspaceAgentResources) Snapshot(limit int) []agentrun.Run {
	return r.registry.Snapshot(limit)
}

func (r *workspaceAgentResources) SnapshotBounded(limit int) ([]agentrun.Run, int) {
	return r.registry.SnapshotBounded(limit)
}

func (r *workspaceAgentResources) RuntimeSnapshotBounded(limit int) ([]agentrun.Run, int) {
	return r.registry.RuntimeSnapshotBounded(limit)
}

func (r *workspaceAgentResources) WithExactRuntimeSnapshot(
	maximum int,
	use func([]agentrun.Run) error,
) error {
	return r.registry.WithExactRuntimeSnapshot(maximum, use)
}

func (r *workspaceAgentResources) RegisterLaunched(
	registration agentrun.Registration,
) (agentrun.Run, error) {
	return r.registry.RegisterLaunched(registration)
}

func (r *workspaceAgentResources) RegisterLinkedLaunched(
	registration agentrun.Registration,
	host *association.Host,
	pointer association.PointerV1,
) (agentrun.Run, error) {
	return r.registry.RegisterLinkedLaunched(registration, host, pointer)
}

func (r *workspaceAgentResources) RollbackLinkedLaunched(runID, terminalID string) bool {
	return r.registry.RollbackLinkedLaunched(runID, terminalID)
}

func (r *workspaceAgentResources) RollbackLinkedTerminal(terminalID string) int {
	return r.registry.RollbackLinkedTerminal(terminalID)
}

func (r *workspaceAgentResources) HasLinkedTerminal(terminalID string) bool {
	return r.registry.HasLinkedTerminal(terminalID)
}

func (r *workspaceAgentResources) IsLinkedLaunchRun(runID string) bool {
	return r.registry.IsLinkedLaunchRun(runID)
}

func (r *workspaceAgentResources) Associate(
	runID string,
	host *association.Host,
	pointer association.PointerV1,
) (association.AssociationV1, error) {
	return r.registry.Associate(runID, host, pointer)
}

func (r *workspaceAgentResources) PrepareLinkedTerminalAssociationChange(
	terminalID string,
	terminalPrevious *association.AssociationV1,
	terminalNext association.AssociationV1,
	host *association.Host,
	pointer association.PointerV1,
) (agentrun.LinkedAssociationChange, bool, error) {
	return r.registry.PrepareLinkedTerminalAssociationChange(
		terminalID,
		terminalPrevious,
		terminalNext,
		host,
		pointer,
	)
}

func (r *workspaceAgentResources) CommitLinkedAssociationChange(
	change agentrun.LinkedAssociationChange,
) error {
	return r.registry.CommitLinkedAssociationChange(change)
}

func (r *workspaceAgentResources) RollbackLinkedAssociationChange(
	change agentrun.LinkedAssociationChange,
) error {
	return r.registry.RollbackLinkedAssociationChange(change)
}

func (r *workspaceAgentResources) RecordTerminalActivity(terminalID string) bool {
	return r.registry.RecordTerminalActivity(terminalID)
}

func (r *workspaceAgentResources) RecordTerminalActivityAt(
	terminalID string,
	activityAt time.Time,
) bool {
	return r.registry.RecordTerminalActivityAt(terminalID, activityAt)
}

func (r *workspaceAgentResources) RecordTerminalExit(
	terminalID string,
	code int,
	result string,
) bool {
	return r.registry.RecordTerminalExit(terminalID, code, result)
}

func (r *workspaceAgentResources) Shutdown(ctx context.Context) error {
	var integrationErr error
	if r.integration != nil {
		integrationErr = r.integration.Shutdown(ctx)
	}
	registryErr := r.registry.Shutdown(ctx)
	return errors.Join(integrationErr, registryErr)
}

func (w *WorkspaceContext) activate() error {
	if resources, ok := w.agents.(interface{ Activate(uint64) error }); ok {
		if err := resources.Activate(w.Generation()); err != nil {
			return err
		}
	}
	if resources, ok := w.capabilities.(interface{ Activate(uint64) error }); ok {
		return resources.Activate(w.Generation())
	}
	return nil
}

func (w *WorkspaceContext) capabilityBroker() workspaceCapabilityBroker {
	broker, _ := w.capabilities.(workspaceCapabilityBroker)
	return broker
}

func (w *WorkspaceContext) agentRegistry() workspaceAgentRegistry {
	registry, _ := w.agents.(workspaceAgentRegistry)
	return registry
}
