package agentrun

import (
	"os"
	"path/filepath"

	"github.com/ro-ag/ptrack/internal/association"
)

// EventCorrelation is historical, non-authoritative event-time context
// stamped from a host-owned Run. It describes where evidence was observed but
// never grants capabilities or revives an association after restart.
type EventCorrelation struct {
	ProjectRoot         string `json:"projectRoot"`
	RepositoryRoot      string `json:"repositoryRoot,omitempty"`
	TerminalID          string `json:"terminalId,omitempty"`
	PlanID              uint64 `json:"planId,omitempty"`
	TaskID              uint64 `json:"taskId,omitempty"`
	Generation          uint64 `json:"generation,omitempty"`
	AssociationRevision uint64 `json:"associationRevision,omitempty"`
}

func discoverEventRepositoryRoot(projectRoot string) string {
	if projectRoot == "" {
		return ""
	}
	current := canonicalRegistryPath(projectRoot)
	for {
		if info, err := os.Stat(filepath.Join(current, ".git")); err == nil &&
			(info.IsDir() || info.Mode().IsRegular()) {
			return current
		}
		parent := filepath.Dir(current)
		if parent == current {
			return ""
		}
		current = parent
	}
}

func eventCorrelationForRun(run Run, repositoryRoot string) EventCorrelation {
	correlation := EventCorrelation{
		ProjectRoot:    run.ProjectRoot,
		RepositoryRoot: repositoryRoot,
		TerminalID:     run.TerminalID,
	}
	current := run.Association
	if current == nil || current.Version != association.VersionV1 ||
		current.ProjectRoot != run.ProjectRoot || current.LiveID != run.ID ||
		current.Generation == 0 || current.Revision == 0 ||
		(current.Target.TaskID != 0 && current.Target.PlanID == 0) {
		return correlation
	}
	correlation.PlanID = current.Target.PlanID
	correlation.TaskID = current.Target.TaskID
	correlation.Generation = current.Generation
	correlation.AssociationRevision = current.Revision
	return correlation
}

func validPersistedEventCorrelation(
	correlation EventCorrelation,
	run Run,
	projectRoot string,
	repositoryRoot string,
) bool {
	if correlation.ProjectRoot != projectRoot || run.ProjectRoot != projectRoot ||
		correlation.TerminalID != run.TerminalID ||
		correlation.RepositoryRoot != repositoryRoot ||
		(correlation.TaskID != 0 && correlation.PlanID == 0) {
		return false
	}
	hasAssociation := correlation.Generation != 0 || correlation.AssociationRevision != 0 ||
		correlation.PlanID != 0 || correlation.TaskID != 0
	if !hasAssociation {
		return true
	}
	return correlation.Generation != 0 && correlation.AssociationRevision != 0
}
