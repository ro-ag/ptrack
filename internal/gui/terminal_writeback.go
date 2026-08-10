package gui

import (
	"errors"
	"fmt"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/launchcontext"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
	"github.com/ro-ag/ptrack/internal/terminal"
)

const (
	TerminalWritebackMaxBytes = 8 * 1024
	terminalWritebackMaxRunes = 4_000
	terminalWritebackMaxLines = 128
)

var (
	ErrTerminalWritebackContent    = errors.New("write-back content is invalid")
	ErrTerminalWritebackCredential = errors.New("write-back content may contain a credential")
	ErrTerminalWritebackConfirm    = errors.New("summary replacement requires explicit confirmation")
)

// TerminalWritebackPreviewV2 is content supplied explicitly by the user plus
// an authoritative, content-free destination derived from the live session.
type TerminalWritebackPreviewV2 struct {
	Generation        uint64 `json:"generation"`
	SessionID         string `json:"sessionId"`
	Revision          uint64 `json:"revision"`
	Kind              string `json:"kind"`
	Content           string `json:"content"`
	ContentBytes      int    `json:"contentBytes"`
	AssociationTarget string `json:"associationTarget"`
	Destination       string `json:"destination"`
	ReplacesSummary   bool   `json:"replacesSummary"`
}

// TerminalWritebackResultV2 identifies a committed mutation without returning
// any terminal, agent, provider, capability, or credential-bearing data.
type TerminalWritebackResultV2 struct {
	Generation  uint64 `json:"generation"`
	SessionID   string `json:"sessionId"`
	Revision    uint64 `json:"revision"`
	RequestID   string `json:"requestId"`
	Kind        string `json:"kind"`
	Destination string `json:"destination"`
	NoteID      uint64 `json:"noteId,omitempty"`
	Replayed    bool   `json:"replayed"`
}

type validatedTerminalWriteback struct {
	association association.AssociationV1
	target      model.NoteTarget
	targetID    uint64
	planID      uint64
	label       string
}

// PreviewTerminalWritebackV2 validates only explicit form content and derives
// its destination from the exact current live association. It never reads the
// terminal stream, title, prompt, environment, or an AgentRun payload.
func (a *App) PreviewTerminalWritebackV2(
	generation uint64,
	sessionID string,
	expectedRevision uint64,
	kind string,
	content string,
) (TerminalWritebackPreviewV2, error) {
	workspace, manager, release, err := a.beginTerminalOperation(generation, false)
	if err != nil {
		return TerminalWritebackPreviewV2{}, err
	}
	defer release()
	s, err := store.Open(workspace.dbPath)
	if err != nil {
		return TerminalWritebackPreviewV2{}, err
	}
	defer s.Close()
	memoryKind, normalized, err := validateTerminalWritebackContent(kind, content)
	if err != nil {
		return TerminalWritebackPreviewV2{}, err
	}

	workspace.associationMu.Lock()
	validated, err := validateLiveTerminalWriteback(
		workspace, manager, s, sessionID, expectedRevision,
	)
	workspace.associationMu.Unlock()
	if err != nil {
		return TerminalWritebackPreviewV2{}, err
	}
	destination := validated.label
	if memoryKind == model.MemorySummary {
		destination = "Project rolling summary"
	}
	return TerminalWritebackPreviewV2{
		Generation:        workspace.Generation(),
		SessionID:         sessionID,
		Revision:          validated.association.Revision,
		Kind:              string(memoryKind),
		Content:           normalized,
		ContentBytes:      len(normalized),
		AssociationTarget: validated.label,
		Destination:       destination,
		ReplacesSummary:   memoryKind == model.MemorySummary,
	}, nil
}

// WriteTerminalMemoryV2 commits one explicitly confirmed write-back. The
// caller supplies no project, plan, task, or AgentRun identifier.
func (a *App) WriteTerminalMemoryV2(
	generation uint64,
	sessionID string,
	expectedRevision uint64,
	requestID string,
	kind string,
	content string,
	confirmSummary bool,
) (TerminalWritebackResultV2, error) {
	workspace, manager, release, err := a.beginTerminalOperation(generation, false)
	if err != nil {
		return TerminalWritebackResultV2{}, err
	}
	defer release()
	s, err := store.Open(workspace.dbPath)
	if err != nil {
		return TerminalWritebackResultV2{}, err
	}
	defer s.Close()
	memoryKind, normalized, err := validateTerminalWritebackContent(kind, content)
	if err != nil {
		return TerminalWritebackResultV2{}, err
	}
	if memoryKind == model.MemorySummary && !confirmSummary {
		return TerminalWritebackResultV2{}, ErrTerminalWritebackConfirm
	}

	workspace.associationMu.Lock()
	liveManager, ok := manager.(terminalLiveAssociationManager)
	if !ok {
		workspace.associationMu.Unlock()
		return TerminalWritebackResultV2{}, errors.New("terminal write-back is unavailable")
	}
	var validated validatedTerminalWriteback
	var writeResult store.MemoryWriteResult
	err = liveManager.WithLiveAssociation(
		sessionID,
		expectedRevision,
		func(current association.AssociationV1) error {
			validated, err = validateTerminalWritebackAssociation(
				workspace, s, sessionID, expectedRevision, &current,
			)
			if err != nil {
				return err
			}
			writeResult, err = s.WriteMemory(store.MemoryWriteRequest{
				RequestID:           requestID,
				Kind:                memoryKind,
				Body:                normalized,
				Target:              validated.target,
				TargetID:            validated.targetID,
				PlanID:              validated.planID,
				WorkspaceGeneration: workspace.Generation(),
				SessionID:           sessionID,
				AssociationRevision: validated.association.Revision,
			})
			return err
		},
	)
	workspace.associationMu.Unlock()
	if err != nil {
		return TerminalWritebackResultV2{}, err
	}
	destination := validated.label
	if memoryKind == model.MemorySummary {
		destination = "Project rolling summary"
	}
	result := TerminalWritebackResultV2{
		Generation: workspace.Generation(), SessionID: sessionID,
		Revision: validated.association.Revision, RequestID: requestID,
		Kind: string(memoryKind), Destination: destination,
		Replayed: writeResult.Replayed,
	}
	if writeResult.Note != nil {
		result.NoteID = writeResult.Note.ID
	}
	return result, nil
}

func validateLiveTerminalWriteback(
	workspace *WorkspaceContext,
	manager terminalManager,
	s *store.Store,
	sessionID string,
	expectedRevision uint64,
) (validatedTerminalWriteback, error) {
	infoManager, ok := manager.(terminalSessionInfoManager)
	if !ok {
		return validatedTerminalWriteback{}, errors.New("terminal write-back is unavailable")
	}
	if expectedRevision == 0 || strings.TrimSpace(sessionID) == "" {
		return validatedTerminalWriteback{}, association.ErrStaleAssociation
	}
	info, err := infoManager.SessionInfo(sessionID)
	if err != nil || info.State != terminal.SessionRunning || info.ID != sessionID {
		return validatedTerminalWriteback{}, errors.New("terminal session is not live")
	}
	return validateTerminalWritebackAssociation(
		workspace, s, sessionID, expectedRevision, info.Association,
	)
}

func validateTerminalWritebackAssociation(
	workspace *WorkspaceContext,
	s *store.Store,
	sessionID string,
	expectedRevision uint64,
	current *association.AssociationV1,
) (validatedTerminalWriteback, error) {
	host, err := workspaceAssociationHost(workspace, s)
	if err != nil {
		return validatedTerminalWriteback{}, err
	}
	if current == nil || current.Revision != expectedRevision ||
		currentRuntimeAssociation(host, sessionID, current) == nil {
		return validatedTerminalWriteback{}, association.ErrStaleAssociation
	}
	if current.Target.PlanID == 0 {
		if registry := workspace.agentRegistry(); registry != nil &&
			registry.HasLinkedTerminal(sessionID) {
			return validatedTerminalWriteback{}, errors.New("terminal session is detached")
		}
		return validatedTerminalWriteback{
			association: *current,
			target:      model.TargetProject,
			label:       "Project",
		}, nil
	}
	if current.Target.TaskID == 0 {
		return validatedTerminalWriteback{
			association: *current,
			target:      model.TargetPlan,
			targetID:    current.Target.PlanID,
			planID:      current.Target.PlanID,
			label:       fmt.Sprintf("Plan #%d", current.Target.PlanID),
		}, nil
	}
	return validatedTerminalWriteback{
		association: *current,
		target:      model.TargetTask,
		targetID:    current.Target.TaskID,
		planID:      current.Target.PlanID,
		label:       fmt.Sprintf("Task #%d", current.Target.TaskID),
	}, nil
}

func validateTerminalWritebackContent(
	kind string,
	content string,
) (model.MemoryKind, string, error) {
	memoryKind := model.MemoryKind(kind)
	switch memoryKind {
	case model.MemorySummary, model.MemoryDecision, model.MemoryBlocker, model.MemoryHandoff:
	default:
		return "", "", fmt.Errorf("%w: unsupported type", ErrTerminalWritebackContent)
	}
	if !utf8.ValidString(content) {
		return "", "", fmt.Errorf("%w: content must be valid UTF-8", ErrTerminalWritebackContent)
	}
	normalized := strings.TrimSpace(strings.ReplaceAll(
		strings.ReplaceAll(content, "\r\n", "\n"), "\r", "\n",
	))
	if normalized == "" {
		return "", "", fmt.Errorf("%w: content is required", ErrTerminalWritebackContent)
	}
	if len(normalized) > TerminalWritebackMaxBytes ||
		utf8.RuneCountInString(normalized) > terminalWritebackMaxRunes ||
		strings.Count(normalized, "\n")+1 > terminalWritebackMaxLines {
		return "", "", fmt.Errorf("%w: content exceeds the hard limit", ErrTerminalWritebackContent)
	}
	for _, value := range normalized {
		if unicode.IsControl(value) && value != '\n' && value != '\t' {
			return "", "", fmt.Errorf("%w: content contains unsupported characters", ErrTerminalWritebackContent)
		}
	}
	if launchcontext.ContainsPotentialCredential(normalized) {
		return "", "", ErrTerminalWritebackCredential
	}
	return memoryKind, normalized, nil
}
