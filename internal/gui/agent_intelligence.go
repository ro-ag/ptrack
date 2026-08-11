package gui

import (
	"errors"
	"fmt"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

const (
	agentIntelligenceEventLimit = 128
	agentSuggestionLimit        = 16
	agentFileSuggestionLimit    = 8
	agentMemorySuggestionLimit  = 4
	agentIssueSuggestionLimit   = 4
	agentSuggestionTextBytes    = 320
)

type AgentSuggestionKind string

const (
	AgentSuggestionContext  AgentSuggestionKind = "context"
	AgentSuggestionFile     AgentSuggestionKind = "file"
	AgentSuggestionDecision AgentSuggestionKind = "decision"
	AgentSuggestionIssue    AgentSuggestionKind = "issue"
)

// AgentSuggestion is a bounded read-only pointer to existing project context
// or observed file metadata. It carries no mutation or capability authority.
type AgentSuggestion struct {
	Kind             AgentSuggestionKind `json:"kind"`
	TargetID         uint64              `json:"targetId,omitempty"`
	Label            string              `json:"label"`
	Path             string              `json:"path,omitempty"`
	Reason           string              `json:"reason"`
	EvidenceEventIDs []string            `json:"evidenceEventIds"`
}

type AgentIntelligenceV2 struct {
	Generation   uint64                  `json:"generation"`
	RunID        string                  `json:"runId"`
	Association  *RuntimeAssociation     `json:"association,omitempty"`
	Intelligence AgentIntelligenceDetail `json:"intelligence"`
	EventBounds  BoundedSnapshot         `json:"eventBounds"`
	Suggestions  []AgentSuggestion       `json:"suggestions"`
	Bounds       BoundedSnapshot         `json:"bounds"`
}

type AgentIntelligenceEvidence struct {
	EventID    string              `json:"eventId,omitempty"`
	Kind       agentrun.EventKind  `json:"kind,omitempty"`
	Phase      agentrun.EventPhase `json:"phase,omitempty"`
	ObservedAt string              `json:"observedAt,omitempty"`
	Reason     string              `json:"reason"`
}

type AgentIntelligenceDetail struct {
	State       agentrun.IntelligenceState      `json:"state"`
	Confidence  agentrun.IntelligenceConfidence `json:"confidence"`
	Evidence    []AgentIntelligenceEvidence     `json:"evidence"`
	EventCount  int                             `json:"eventCount"`
	LastEventAt string                          `json:"lastEventAt,omitempty"`
}

type agentIntelligenceRegistry interface {
	IntelligenceSnapshot(
		string,
		int,
	) (agentrun.Run, []agentrun.Event, int, agentrun.RunIntelligence, error)
}

// GetAgentIntelligenceV2 returns a generation-fenced, non-mutating view. It
// never writes notes, summaries, issues, tasks, or run state.
func (a *App) GetAgentIntelligenceV2(
	generation uint64,
	runID string,
) (AgentIntelligenceV2, error) {
	s, workspace, release, err := a.openWorkspace(generation)
	if err != nil {
		return AgentIntelligenceV2{}, err
	}
	defer release()
	defer s.Close()
	registry, ok := workspace.agents.(agentIntelligenceRegistry)
	if !ok {
		return AgentIntelligenceV2{}, errors.New("AgentRun intelligence is unavailable")
	}
	return buildAgentIntelligenceV2(s, workspace, registry, runID)
}

func buildAgentIntelligenceV2(
	s *store.Store,
	workspace *WorkspaceContext,
	registry agentIntelligenceRegistry,
	runID string,
) (AgentIntelligenceV2, error) {
	host, err := workspaceAssociationHost(workspace, s)
	if err != nil {
		return AgentIntelligenceV2{}, err
	}
	workspace.associationMu.Lock()
	run, events, eventTotal, intelligence, err := registry.IntelligenceSnapshot(
		runID,
		agentIntelligenceEventLimit,
	)
	if err != nil {
		workspace.associationMu.Unlock()
		return AgentIntelligenceV2{}, err
	}
	association := currentRuntimeAssociation(host, run.ID, run.Association)
	workspace.associationMu.Unlock()
	suggestions, total, err := buildAgentSuggestions(s, run, association, events)
	if err != nil {
		return AgentIntelligenceV2{}, err
	}
	return AgentIntelligenceV2{
		Generation:   workspace.Generation(),
		RunID:        run.ID,
		Association:  association,
		Intelligence: projectAgentIntelligence(intelligence),
		EventBounds:  snapshotBound(len(events), eventTotal),
		Suggestions:  suggestions,
		Bounds:       snapshotBound(len(suggestions), total),
	}, nil
}

func projectAgentIntelligence(
	intelligence agentrun.RunIntelligence,
) AgentIntelligenceDetail {
	projected := AgentIntelligenceDetail{
		State: intelligence.State, Confidence: intelligence.Confidence,
		Evidence: []AgentIntelligenceEvidence{}, EventCount: intelligence.EventCount,
	}
	if !intelligence.LastEventAt.IsZero() {
		projected.LastEventAt = intelligence.LastEventAt.UTC().Format(time.RFC3339Nano)
	}
	for _, evidence := range intelligence.Evidence {
		item := AgentIntelligenceEvidence{
			EventID: evidence.EventID, Kind: evidence.Kind, Phase: evidence.Phase,
			Reason: evidence.Reason,
		}
		if !evidence.ObservedAt.IsZero() {
			item.ObservedAt = evidence.ObservedAt.UTC().Format(time.RFC3339Nano)
		}
		projected.Evidence = append(projected.Evidence, item)
	}
	return projected
}

type intelligenceStore interface {
	GetPlan(uint64) (model.Plan, error)
	GetTask(uint64) (model.Task, error)
	RecentNotes(int) ([]model.Note, error)
	ListIssues() ([]model.Issue, error)
}

func buildAgentSuggestions(
	s intelligenceStore,
	run agentrun.Run,
	association *RuntimeAssociation,
	events []agentrun.Event,
) ([]AgentSuggestion, int, error) {
	candidates := []AgentSuggestion{}
	if association != nil && association.TaskID != 0 {
		task, err := s.GetTask(association.TaskID)
		if err != nil {
			return nil, 0, err
		}
		candidates = append(candidates, AgentSuggestion{
			Kind: AgentSuggestionContext, TargetID: task.ID,
			Label:  boundedSuggestionText(fmt.Sprintf("Task #%d · %s", task.ID, task.Title)),
			Reason: "current host-validated task association", EvidenceEventIDs: []string{},
		})
	}
	if association != nil && association.PlanID != 0 {
		plan, err := s.GetPlan(association.PlanID)
		if err != nil {
			return nil, 0, err
		}
		candidates = append(candidates, AgentSuggestion{
			Kind: AgentSuggestionContext, TargetID: plan.ID,
			Label:  boundedSuggestionText(fmt.Sprintf("Plan #%d · %s", plan.ID, plan.Title)),
			Reason: "current host-validated plan association", EvidenceEventIDs: []string{},
		})
	}

	fileCount := 0
	seenPaths := map[string]bool{}
	for index := len(events) - 1; index >= 0 && fileCount < agentFileSuggestionLimit; index-- {
		event := events[index]
		if !eventRelevantToCurrentAssociation(run, association, event) {
			continue
		}
		for _, path := range event.Paths {
			if seenPaths[path] {
				continue
			}
			seenPaths[path] = true
			candidates = append(candidates, AgentSuggestion{
				Kind: AgentSuggestionFile, Label: boundedSuggestionText(path), Path: path,
				Reason:           "observed structured file evidence",
				EvidenceEventIDs: []string{event.ID},
			})
			fileCount++
			if fileCount == agentFileSuggestionLimit {
				break
			}
		}
	}

	notes, err := s.RecentNotes(50)
	if err != nil {
		return nil, 0, err
	}
	decisionCount := 0
	for _, note := range notes {
		if note.Kind != model.MemoryDecision ||
			!memoryRelevantToAssociation(note.Target, note.TargetID, association) {
			continue
		}
		candidates = append(candidates, AgentSuggestion{
			Kind: AgentSuggestionDecision, TargetID: note.ID,
			Label:  boundedSuggestionText(note.Body),
			Reason: "relevant durable decision", EvidenceEventIDs: []string{},
		})
		decisionCount++
		if decisionCount == agentMemorySuggestionLimit {
			break
		}
	}

	issues, err := s.ListIssues()
	if err != nil {
		return nil, 0, err
	}
	issueCount := 0
	for _, issue := range issues {
		if issue.Status != model.IssueOpen {
			continue
		}
		relevant, relevanceErr := issueRelevantToAssociation(s, issue, association)
		if relevanceErr != nil {
			return nil, 0, relevanceErr
		}
		if !relevant {
			continue
		}
		candidates = append(candidates, AgentSuggestion{
			Kind: AgentSuggestionIssue, TargetID: issue.ID,
			Label:  boundedSuggestionText(issue.Title),
			Reason: "relevant open issue", EvidenceEventIDs: []string{},
		})
		issueCount++
		if issueCount == agentIssueSuggestionLimit {
			break
		}
	}

	unique := make([]AgentSuggestion, 0, min(len(candidates), agentSuggestionLimit))
	seen := map[string]bool{}
	for _, suggestion := range candidates {
		key := fmt.Sprintf("%s:%d:%s:%s", suggestion.Kind, suggestion.TargetID, suggestion.Path, suggestion.Label)
		if seen[key] {
			continue
		}
		seen[key] = true
		unique = append(unique, suggestion)
	}
	total := len(unique)
	if len(unique) > agentSuggestionLimit {
		unique = unique[:agentSuggestionLimit]
	}
	return unique, total, nil
}

func issueRelevantToAssociation(
	s intelligenceStore,
	issue model.Issue,
	association *RuntimeAssociation,
) (bool, error) {
	if association == nil {
		return issue.TaskID == 0, nil
	}
	if association.TaskID != 0 {
		return issue.TaskID == association.TaskID, nil
	}
	if association.PlanID == 0 || issue.TaskID == 0 {
		return false, nil
	}
	task, err := s.GetTask(issue.TaskID)
	if err != nil {
		return false, err
	}
	return task.PlanID == association.PlanID, nil
}

func eventRelevantToCurrentAssociation(
	run agentrun.Run,
	association *RuntimeAssociation,
	event agentrun.Event,
) bool {
	if event.RunID != run.ID || event.Correlation.ProjectRoot != run.ProjectRoot ||
		event.Correlation.TerminalID != run.TerminalID {
		return false
	}
	if association == nil {
		return event.Correlation.PlanID == 0 && event.Correlation.TaskID == 0
	}
	if run.Association == nil {
		return false
	}
	return event.Correlation.PlanID == association.PlanID &&
		event.Correlation.TaskID == association.TaskID &&
		event.Correlation.Generation == run.Association.Generation &&
		event.Correlation.AssociationRevision == association.Revision
}

func memoryRelevantToAssociation(
	target model.NoteTarget,
	targetID uint64,
	association *RuntimeAssociation,
) bool {
	switch target {
	case model.TargetProject:
		return targetID == 0
	case model.TargetPlan:
		return association != nil && association.PlanID == targetID
	case model.TargetTask:
		return association != nil && association.TaskID == targetID
	default:
		return false
	}
}

func boundedSuggestionText(value string) string {
	value = strings.Join(strings.Fields(value), " ")
	if len(value) <= agentSuggestionTextBytes {
		return value
	}
	value = value[:agentSuggestionTextBytes]
	for !utf8.ValidString(value) {
		value = value[:len(value)-1]
	}
	return strings.TrimSpace(value) + "…"
}
