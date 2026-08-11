package gui

import (
	"sort"
	"strings"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/gitinfo"
	"github.com/ro-ag/ptrack/internal/model"
)

const driftFindingLimit = 64

type DriftFindingKind string
type DriftSeverity string
type DriftScope string

const (
	DriftCheckoutChangedPath DriftFindingKind = "checkoutChangedPath"
	DriftUntrackedFile       DriftFindingKind = "untrackedFile"
	DriftUnlinkedCommit      DriftFindingKind = "unlinkedCommit"
	DriftCrossTaskPath       DriftFindingKind = "crossTaskPathOverlap"
	DriftTaskSignal          DriftFindingKind = "taskDriftSignal"

	DriftSeverityInfo    DriftSeverity = "info"
	DriftSeverityWarning DriftSeverity = "warning"

	DriftScopeProject        DriftScope = "projectUnattributed"
	DriftScopeAgent          DriftScope = "agent"
	DriftScopeTaskComparison DriftScope = "taskComparison"
)

type DriftFinding struct {
	Kind          DriftFindingKind `json:"kind"`
	Severity      DriftSeverity    `json:"severity"`
	Scope         DriftScope       `json:"scope"`
	Path          string           `json:"path,omitempty"`
	SHA           string           `json:"sha,omitempty"`
	RunIDs        []string         `json:"runIds"`
	PlanIDs       []uint64         `json:"planIds"`
	TaskIDs       []uint64         `json:"taskIds"`
	EvidenceCount int              `json:"evidenceCount"`
}

type DriftSnapshot struct {
	State      SnapshotState   `json:"state"`
	Findings   []DriftFinding  `json:"findings"`
	Bounds     BoundedSnapshot `json:"bounds"`
	Incomplete bool            `json:"incomplete"`
}

type driftPathEvidence struct {
	runID  string
	planID uint64
	taskID uint64
}

func buildDriftSnapshot(
	workspace *WorkspaceContext,
	projection runtimeProjection,
	activity AgentActivitySnapshot,
	git GitSnapshot,
	linkedCommits []model.Commit,
	trackingStartedAt time.Time,
) DriftSnapshot {
	result := DriftSnapshot{State: SnapshotReady, Findings: []DriftFinding{}}
	if git.State != SnapshotReady && git.State != SnapshotStale {
		result.Incomplete = true
	} else if git.Snapshot.State == gitinfo.RepositoryReady {
		for _, path := range git.Snapshot.Status.ChangedPaths {
			result.Findings = append(result.Findings, DriftFinding{
				Kind: DriftCheckoutChangedPath, Severity: DriftSeverityInfo,
				Scope: DriftScopeProject, Path: path, RunIDs: []string{},
				PlanIDs: []uint64{}, TaskIDs: []uint64{}, EvidenceCount: 1,
			})
		}
		for _, path := range git.Snapshot.Status.UntrackedPaths {
			result.Findings = append(result.Findings, DriftFinding{
				Kind: DriftUntrackedFile, Severity: DriftSeverityWarning,
				Scope: DriftScopeProject, Path: path, RunIDs: []string{},
				PlanIDs: []uint64{}, TaskIDs: []uint64{}, EvidenceCount: 1,
			})
		}
		result.Incomplete = git.Snapshot.Status.ChangedPathBounds.More > 0 ||
			git.Snapshot.Status.UntrackedPathBounds.More > 0 ||
			git.Snapshot.RecentCommitsTruncated || git.Snapshot.UnpushedCommitsTruncated
		observed := append(
			append([]gitinfo.Commit{}, git.Snapshot.UnpushedCommits...),
			git.Snapshot.RecentCommits...,
		)
		linked := linkedObservedCommitSHAs(linkedCommits, observed)
		seen := map[string]bool{}
		for _, commit := range observed {
			sha := strings.ToLower(strings.TrimSpace(commit.SHA))
			if sha == "" || seen[sha] || linked[sha] {
				continue
			}
			if !trackingStartedAt.IsZero() {
				committedAt, err := time.Parse(time.RFC3339, commit.Date)
				if err != nil {
					result.Incomplete = true
					continue
				}
				if committedAt.Before(trackingStartedAt.Truncate(time.Second)) {
					continue
				}
			}
			seen[sha] = true
			result.Findings = append(result.Findings, DriftFinding{
				Kind: DriftUnlinkedCommit, Severity: DriftSeverityInfo,
				Scope: DriftScopeProject, SHA: sha, RunIDs: []string{},
				PlanIDs: []uint64{}, TaskIDs: []uint64{}, EvidenceCount: 1,
			})
		}
	}

	activityByRun := make(map[string]AgentActivity, len(activity.Items))
	for _, item := range activity.Items {
		activityByRun[item.RunID] = item
	}
	for _, run := range projection.agents {
		if run.Live && run.Association != nil && run.Intelligence != nil &&
			run.Intelligence.State == agentrun.IntelligencePotentiallyDrifting {
			result.Findings = append(result.Findings, DriftFinding{
				Kind: DriftTaskSignal, Severity: DriftSeverityWarning,
				Scope: DriftScopeAgent, RunIDs: []string{run.RunID},
				PlanIDs:       []uint64{run.Association.PlanID},
				TaskIDs:       []uint64{run.Association.TaskID},
				EvidenceCount: run.Intelligence.EvidenceCount,
			})
		}
	}
	registry, ok := workspace.agents.(agentIntelligenceRegistry)
	if !ok {
		result.Incomplete = true
	} else {
		byPath := map[string][]driftPathEvidence{}
		for _, projected := range projection.agents {
			item := activityByRun[projected.RunID]
			if !projected.Live || item.Ownership == nil || projected.Association == nil ||
				projected.Association.TaskID == 0 {
				continue
			}
			expected, exact := projection.exactAgentRuns[projected.RunID]
			run, events, total, _, err := registry.IntelligenceSnapshot(
				projected.RunID,
				agentIntelligenceEventLimit,
			)
			if err != nil || !exact || !exactAgentEvidenceSnapshot(expected, run) {
				result.Incomplete = true
				continue
			}
			if total > len(events) {
				result.Incomplete = true
			}
			seenPaths := map[string]bool{}
			for _, event := range events {
				if !eventRelevantToCurrentAssociation(run, projected.Association, event) {
					continue
				}
				for _, path := range event.Paths {
					if path == "" || seenPaths[path] {
						continue
					}
					seenPaths[path] = true
					byPath[path] = append(byPath[path], driftPathEvidence{
						runID: run.ID, planID: projected.Association.PlanID,
						taskID: projected.Association.TaskID,
					})
				}
			}
		}
		for path, evidence := range byPath {
			targets := map[[2]uint64]bool{}
			for _, item := range evidence {
				targets[[2]uint64{item.planID, item.taskID}] = true
			}
			if len(targets) < 2 {
				continue
			}
			sort.Slice(evidence, func(i, j int) bool {
				return evidence[i].runID < evidence[j].runID
			})
			finding := DriftFinding{
				Kind: DriftCrossTaskPath, Severity: DriftSeverityWarning,
				Scope: DriftScopeTaskComparison, Path: path,
				RunIDs: []string{}, PlanIDs: []uint64{}, TaskIDs: []uint64{},
				EvidenceCount: len(evidence),
			}
			for _, item := range evidence {
				finding.RunIDs = append(finding.RunIDs, item.runID)
				finding.PlanIDs = append(finding.PlanIDs, item.planID)
				finding.TaskIDs = append(finding.TaskIDs, item.taskID)
			}
			result.Findings = append(result.Findings, finding)
		}
	}
	result.Incomplete = result.Incomplete || projection.agentBounds.More > 0 ||
		projection.agentAnalysisIncomplete
	sort.Slice(result.Findings, func(i, j int) bool {
		left, right := result.Findings[i], result.Findings[j]
		if left.Severity != right.Severity {
			return left.Severity == DriftSeverityWarning
		}
		if left.Kind != right.Kind {
			return left.Kind < right.Kind
		}
		if left.Path != right.Path {
			return left.Path < right.Path
		}
		if left.SHA != right.SHA {
			return left.SHA < right.SHA
		}
		return strings.Join(left.RunIDs, "\x00") < strings.Join(right.RunIDs, "\x00")
	})
	total := len(result.Findings)
	if len(result.Findings) > driftFindingLimit {
		result.Findings = result.Findings[:driftFindingLimit]
		result.Incomplete = true
	}
	result.Bounds = snapshotBound(len(result.Findings), total)
	return result
}

func linkedObservedCommitSHAs(
	linkedCommits []model.Commit,
	observed []gitinfo.Commit,
) map[string]bool {
	observedSHAs := make(map[string]bool, len(observed))
	for _, commit := range observed {
		sha := strings.ToLower(strings.TrimSpace(commit.SHA))
		if sha != "" {
			observedSHAs[sha] = true
		}
	}
	linked := make(map[string]bool, len(linkedCommits))
	for _, commit := range linkedCommits {
		candidate := strings.ToLower(strings.TrimSpace(commit.SHA))
		if observedSHAs[candidate] {
			linked[candidate] = true
			continue
		}
		if len(candidate) < 7 || len(candidate) > 64 || !validHexCommit(candidate) {
			continue
		}
		match := ""
		ambiguous := false
		for sha := range observedSHAs {
			if !strings.HasPrefix(sha, candidate) {
				continue
			}
			if match != "" && match != sha {
				ambiguous = true
				break
			}
			match = sha
		}
		if match != "" && !ambiguous {
			linked[match] = true
		}
	}
	return linked
}

func validHexCommit(value string) bool {
	for _, character := range value {
		if !strings.ContainsRune("0123456789abcdef", character) {
			return false
		}
	}
	return true
}
