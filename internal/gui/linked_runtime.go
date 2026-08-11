package gui

import (
	"sort"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/store"
	"github.com/ro-ag/ptrack/internal/terminal"
)

const (
	linkedRuntimeEntryLimit     = 64
	linkedRuntimeCandidateLimit = 1_024
)

// RuntimeAssociation is the authority-free part of a current, host-validated
// AssociationV1 that is useful to presentation code. Project roots, live
// identities, generations, and capability data never cross this boundary.
type RuntimeAssociation struct {
	PlanID   uint64 `json:"planId,omitempty"`
	TaskID   uint64 `json:"taskId,omitempty"`
	Revision uint64 `json:"revision"`
}

// TerminalRuntimeSummary is deliberately content-free. It contains no CWD,
// process output, title, prompt, environment, token, or capability metadata.
type TerminalRuntimeSummary struct {
	SessionID   string                `json:"sessionId"`
	ProfileKind terminal.ProfileKind  `json:"profileKind"`
	State       terminal.SessionState `json:"state"`
	Live        bool                  `json:"live"`
	Association *RuntimeAssociation   `json:"association,omitempty"`
}

// AgentRuntimeSummary excludes raw prompts and AgentRun Exit.Result. A
// terminal-backed run is explicitly distinguished from an external run, and
// CorrespondingTerminal is true only when both current associations match.
type AgentRuntimeSummary struct {
	RunID                 string                    `json:"runId"`
	RegistrationKind      agentrun.RegistrationKind `json:"registrationKind"`
	TerminalID            string                    `json:"terminalId,omitempty"`
	TerminalBacked        bool                      `json:"terminalBacked"`
	TerminalPresent       bool                      `json:"terminalPresent"`
	CorrespondingTerminal bool                      `json:"correspondingTerminal"`
	State                 agentrun.State            `json:"state"`
	ProcessState          agentrun.ProcessState     `json:"processState"`
	LeaseState            agentrun.LeaseState       `json:"leaseState"`
	Live                  bool                      `json:"live"`
	ActivityState         agentrun.ActivityState    `json:"activityState"`
	Association           *RuntimeAssociation       `json:"association,omitempty"`
	Intelligence          *AgentIntelligenceSummary `json:"intelligence,omitempty"`
}

// AgentIntelligenceSummary is deliberately content-free: only the derived
// state and evidence counts/timestamp cross the Wails boundary.
type AgentIntelligenceSummary struct {
	State         agentrun.IntelligenceState      `json:"state"`
	Confidence    agentrun.IntelligenceConfidence `json:"confidence"`
	EvidenceCount int                             `json:"evidenceCount"`
	EventCount    int                             `json:"eventCount"`
	LastEventAt   string                          `json:"lastEventAt,omitempty"`
}

type TaskLinkedRuntimeSummary struct {
	Terminals          int  `json:"terminals"`
	LiveTerminals      int  `json:"liveTerminals"`
	Agents             int  `json:"agents"`
	LiveAgents         int  `json:"liveAgents"`
	TerminalBackedRuns int  `json:"terminalBackedRuns"`
	ExternalRuns       int  `json:"externalRuns"`
	Truncated          bool `json:"truncated"`
}

type TaskLinkedRuntimeDetail struct {
	Summary          TaskLinkedRuntimeSummary `json:"summary"`
	Terminals        []TerminalRuntimeSummary `json:"terminals"`
	Agents           []AgentRuntimeSummary    `json:"agents"`
	TerminalRowsMore int                      `json:"terminalRowsMore"`
	AgentRowsMore    int                      `json:"agentRowsMore"`
}

type runtimeProjection struct {
	terminals               []TerminalRuntimeSummary
	agents                  []AgentRuntimeSummary
	terminalCandidates      []TerminalRuntimeSummary
	agentCandidates         []AgentRuntimeSummary
	terminalBounds          BoundedSnapshot
	agentBounds             BoundedSnapshot
	sourcesTruncated        bool
	exactAgentRuns          map[string]agentrun.Run
	agentAnalysisIncomplete bool
}

func workspaceRuntimeProjection(
	s *store.Store,
	workspace *WorkspaceContext,
) (runtimeProjection, error) {
	workspace.associationMu.Lock()
	defer workspace.associationMu.Unlock()
	host, err := workspaceAssociationHost(workspace, s)
	if err != nil {
		return runtimeProjection{}, err
	}
	sessions := []terminal.SessionInfo{}
	terminalTotal := 0
	if manager, ok := workspace.terminals.(interface {
		RuntimeSessionSnapshotBounded(int) ([]terminal.SessionInfo, int)
	}); ok {
		sessions, terminalTotal = manager.RuntimeSessionSnapshotBounded(
			linkedRuntimeCandidateLimit,
		)
	} else if manager, ok := workspace.terminals.(interface {
		SessionSnapshot(int) []terminal.SessionInfo
	}); ok {
		sessions = manager.SessionSnapshot(linkedRuntimeCandidateLimit)
		terminalTotal = len(sessions)
	}
	runs := []agentrun.Run{}
	agentTotal := 0
	exactAgentRuns := make(map[string]agentrun.Run)
	agentAnalysisIncomplete := false
	if registry := workspace.agentRegistry(); registry != nil {
		for _, session := range sessions {
			registry.RecordTerminalActivityAt(session.ID, session.LastActivityAt)
		}
		runs, agentTotal = registry.RuntimeSnapshotBounded(linkedRuntimeCandidateLimit)
		exactRuns := []agentrun.Run{}
		exactErr := registry.WithExactRuntimeSnapshot(
			linkedRuntimeCandidateLimit,
			func(snapshot []agentrun.Run) error {
				exactRuns = append(exactRuns, snapshot...)
				return nil
			},
		)
		agentAnalysisIncomplete = exactErr != nil
		if exactErr == nil {
			runs = exactRuns
			agentTotal = max(agentTotal, len(exactRuns))
			for _, run := range exactRuns {
				exactAgentRuns[run.ID] = run
			}
		}
	}
	projection := buildRuntimeProjection(host, sessions, runs)
	projection.exactAgentRuns = exactAgentRuns
	projection.agentAnalysisIncomplete = agentAnalysisIncomplete
	if intelligenceRegistry, ok := workspace.agents.(agentIntelligenceRegistry); ok {
		intelligenceByRun := make(map[string]AgentIntelligenceSummary, len(runs))
		activityByRun := make(map[string]agentrun.ActivityState, len(runs))
		for _, run := range runs {
			expected, exact := exactAgentRuns[run.ID]
			observed, events, total, _, intelligenceErr :=
				intelligenceRegistry.IntelligenceSnapshot(run.ID, 0)
			if intelligenceErr != nil || !exact ||
				!exactAgentEvidenceSnapshot(expected, observed) {
				projection.agentAnalysisIncomplete = true
				continue
			}
			association := runtimeAssociationForRun(projection.agentCandidates, run.ID)
			intelligence := agentrun.DeriveRunIntelligence(
				expected,
				currentAssociationEvents(expected, association, events),
			)
			if total > len(events) {
				projection.agentAnalysisIncomplete = true
			}
			projected := projectAgentIntelligence(intelligence)
			intelligenceByRun[run.ID] = AgentIntelligenceSummary{
				State:         intelligence.State,
				Confidence:    intelligence.Confidence,
				EvidenceCount: len(intelligence.Evidence),
				EventCount:    intelligence.EventCount,
				LastEventAt:   projected.LastEventAt,
			}
			activityByRun[run.ID] = agentrun.DeriveActivityState(expected, intelligence)
		}
		applyAgentIntelligence(&projection, intelligenceByRun, activityByRun)
	}
	projection.terminalBounds = snapshotBound(len(projection.terminals), terminalTotal)
	projection.agentBounds = snapshotBound(len(projection.agents), agentTotal)
	projection.sourcesTruncated = terminalTotal > len(sessions) || agentTotal > len(runs)
	projection.agentAnalysisIncomplete = projection.agentAnalysisIncomplete ||
		agentTotal > len(runs)
	return projection, nil
}

func runtimeAssociationForRun(
	runs []AgentRuntimeSummary,
	runID string,
) *RuntimeAssociation {
	for _, run := range runs {
		if run.RunID == runID {
			return run.Association
		}
	}
	return nil
}

// exactAgentEvidenceSnapshot fences structured evidence to the exact run
// lifecycle and host association used to build the visible runtime row.
// Heartbeat timestamps may advance without invalidating evidence, but an exit,
// stale/revive transition, or reassociation must fail closed.
func exactAgentEvidenceSnapshot(expected, observed agentrun.Run) bool {
	if expected.ID == "" || expected.ID != observed.ID ||
		expected.LifecycleRevision == 0 ||
		expected.LifecycleRevision != observed.LifecycleRevision ||
		expected.ProjectRoot != observed.ProjectRoot ||
		expected.TerminalID != observed.TerminalID || expected.Kind != observed.Kind {
		return false
	}
	if expected.Association == nil || observed.Association == nil {
		return expected.Association == nil && observed.Association == nil
	}
	return *expected.Association == *observed.Association
}

func applyAgentIntelligence(
	projection *runtimeProjection,
	intelligenceByRun map[string]AgentIntelligenceSummary,
	activityByRun map[string]agentrun.ActivityState,
) {
	if projection == nil {
		return
	}
	apply := func(runs []AgentRuntimeSummary) {
		for index := range runs {
			intelligence, exists := intelligenceByRun[runs[index].RunID]
			if exists {
				copy := intelligence
				runs[index].Intelligence = &copy
			}
			if activity, activityExists := activityByRun[runs[index].RunID]; activityExists {
				runs[index].ActivityState = activity
			}
		}
	}
	apply(projection.agents)
	apply(projection.agentCandidates)
}

func buildRuntimeProjection(
	host *association.Host,
	sessions []terminal.SessionInfo,
	runs []agentrun.Run,
) runtimeProjection {
	terminals := make([]TerminalRuntimeSummary, 0, len(sessions))
	for _, session := range sessions {
		terminals = append(terminals, TerminalRuntimeSummary{
			SessionID:   session.ID,
			ProfileKind: session.ProfileKind,
			State:       session.State,
			Live:        terminalStateIsLive(session.State),
			Association: currentRuntimeAssociation(host, session.ID, session.Association),
		})
	}
	sort.Slice(terminals, func(i, j int) bool {
		return runtimeAssociationLess(
			terminals[i].Association,
			terminals[j].Association,
			terminals[i].SessionID,
			terminals[j].SessionID,
		)
	})

	terminalByID := make(map[string]TerminalRuntimeSummary, len(terminals))
	for _, session := range terminals {
		terminalByID[session.SessionID] = session
	}
	agents := make([]AgentRuntimeSummary, 0, len(runs))
	for _, run := range runs {
		associationSummary := currentRuntimeAssociation(host, run.ID, run.Association)
		terminalBacked := run.Kind == agentrun.RegistrationLaunched && run.TerminalID != ""
		corresponding := false
		terminalPresent := false
		if terminalBacked {
			if session, exists := terminalByID[run.TerminalID]; exists {
				terminalPresent = true
				if associationSummary != nil {
					corresponding = runtimeAssociationsEqual(
						associationSummary,
						session.Association,
					)
				}
			}
		}
		agents = append(agents, AgentRuntimeSummary{
			RunID:                 run.ID,
			RegistrationKind:      run.Kind,
			TerminalID:            run.TerminalID,
			TerminalBacked:        terminalBacked,
			TerminalPresent:       terminalPresent,
			CorrespondingTerminal: corresponding,
			State:                 run.State,
			ProcessState:          run.ProcessState,
			LeaseState:            run.LeaseState,
			Live:                  agentRunIsLive(run),
			ActivityState:         agentrun.DeriveActivityState(run, agentrun.RunIntelligence{}),
			Association:           associationSummary,
		})
	}
	sort.Slice(agents, func(i, j int) bool {
		return runtimeAssociationLess(
			agents[i].Association,
			agents[j].Association,
			agents[i].RunID,
			agents[j].RunID,
		)
	})

	projection := runtimeProjection{
		terminalBounds: snapshotBound(min(len(terminals), linkedRuntimeEntryLimit), len(terminals)),
		agentBounds:    snapshotBound(min(len(agents), linkedRuntimeEntryLimit), len(agents)),
	}
	projection.terminalCandidates = append(
		[]TerminalRuntimeSummary{},
		terminals...,
	)
	projection.agentCandidates = append(
		[]AgentRuntimeSummary{},
		agents...,
	)
	projection.terminals = append(
		[]TerminalRuntimeSummary{},
		terminals[:min(len(terminals), linkedRuntimeEntryLimit)]...,
	)
	projection.agents = append(
		[]AgentRuntimeSummary{},
		agents[:min(len(agents), linkedRuntimeEntryLimit)]...,
	)
	return projection
}

func currentRuntimeAssociation(
	host *association.Host,
	liveID string,
	current *association.AssociationV1,
) *RuntimeAssociation {
	if host == nil || current == nil || current.Version != association.VersionV1 ||
		current.ProjectRoot != host.ProjectRoot() ||
		current.Generation != host.Generation() ||
		current.LiveID != liveID || current.Revision == 0 ||
		(current.Target.TaskID != 0 && current.Target.PlanID == 0) {
		return nil
	}
	validated, err := host.Validate(association.PointerV1{
		Version: association.VersionV1,
		PlanID:  current.Target.PlanID,
		TaskID:  current.Target.TaskID,
	})
	if err != nil || validated != current.Target {
		return nil
	}
	return &RuntimeAssociation{
		PlanID:   validated.PlanID,
		TaskID:   validated.TaskID,
		Revision: current.Revision,
	}
}

func terminalStateIsLive(state terminal.SessionState) bool {
	return state == terminal.SessionStarting || state == terminal.SessionRunning ||
		state == terminal.SessionClosing
}

func agentRunIsLive(run agentrun.Run) bool {
	if run.State != agentrun.StateRunning || run.ProcessState == agentrun.ProcessExited {
		return false
	}
	if run.Kind == agentrun.RegistrationExternal {
		return run.LeaseState == agentrun.LeaseActive
	}
	return run.Kind == agentrun.RegistrationLaunched &&
		run.ProcessState == agentrun.ProcessRunning
}

func runtimeAssociationsEqual(left, right *RuntimeAssociation) bool {
	return left != nil && right != nil && *left == *right
}

func runtimeAssociationLess(
	left, right *RuntimeAssociation,
	leftID, rightID string,
) bool {
	if left == nil && right != nil {
		return false
	}
	if left != nil && right == nil {
		return true
	}
	if left != nil && right != nil {
		if left.PlanID != right.PlanID {
			return left.PlanID < right.PlanID
		}
		if left.TaskID != right.TaskID {
			return left.TaskID < right.TaskID
		}
		if left.Revision != right.Revision {
			return left.Revision < right.Revision
		}
	}
	return leftID < rightID
}

func taskLinkedRuntime(
	projection runtimeProjection,
	taskID uint64,
) TaskLinkedRuntimeDetail {
	detail := TaskLinkedRuntimeDetail{
		Terminals: []TerminalRuntimeSummary{},
		Agents:    []AgentRuntimeSummary{},
	}
	detail.Summary.Truncated = projection.sourcesTruncated
	for _, session := range projection.terminalCandidates {
		if session.Association == nil || session.Association.TaskID != taskID {
			continue
		}
		if len(detail.Terminals) < linkedRuntimeEntryLimit {
			detail.Terminals = append(detail.Terminals, session)
		} else {
			detail.TerminalRowsMore++
		}
		detail.Summary.Terminals++
		if session.Live {
			detail.Summary.LiveTerminals++
		}
	}
	for _, run := range projection.agentCandidates {
		if run.Association == nil || run.Association.TaskID != taskID {
			continue
		}
		if len(detail.Agents) < linkedRuntimeEntryLimit {
			detail.Agents = append(detail.Agents, run)
		} else {
			detail.AgentRowsMore++
		}
		detail.Summary.Agents++
		if run.Live {
			detail.Summary.LiveAgents++
		}
		if run.TerminalBacked {
			detail.Summary.TerminalBackedRuns++
		} else if run.RegistrationKind == agentrun.RegistrationExternal {
			detail.Summary.ExternalRuns++
		}
	}
	return detail
}

func applyLinkedRuntimeToBoard(board *Board, projection runtimeProjection) {
	if board == nil {
		return
	}
	for columnIndex := range board.Columns {
		for taskIndex := range board.Columns[columnIndex].Tasks {
			task := &board.Columns[columnIndex].Tasks[taskIndex]
			detail := taskLinkedRuntime(projection, task.ID)
			if detail.Summary.Terminals != 0 || detail.Summary.Agents != 0 ||
				detail.Summary.Truncated {
				summary := detail.Summary
				task.LinkedRuntime = &summary
			}
		}
	}
}
