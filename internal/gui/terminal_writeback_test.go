package gui

import (
	"errors"
	"strings"
	"testing"

	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
)

func TestTerminalWritebackDerivesTaskTargetAndReplaysWithoutDuplicate(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	broker := &fakeWorkspaceCapabilityBroker{token: "authority-token"}
	fixture.app.workspace.capabilities = broker
	launched, err := fixture.app.LaunchLinkedAgentV2(
		1, "agent-beta", "", 24, 80, fixture.taskPointer(),
	)
	if err != nil {
		t.Fatal(err)
	}
	preview, err := fixture.app.PreviewTerminalWritebackV2(
		1, launched.SessionID, 1, "decision", "  choose explicit write-back\r\n ",
	)
	if err != nil {
		t.Fatal(err)
	}
	if preview.AssociationTarget != "Task #1" || preview.Destination != "Task #1" ||
		preview.Content != "choose explicit write-back" || preview.ReplacesSummary {
		t.Fatalf("preview = %#v", preview)
	}
	result, err := fixture.app.WriteTerminalMemoryV2(
		1, launched.SessionID, 1, "writeback-stable-1", "decision",
		preview.Content, false,
	)
	if err != nil {
		t.Fatal(err)
	}
	replayed, err := fixture.app.WriteTerminalMemoryV2(
		1, launched.SessionID, 1, "writeback-stable-1", "decision",
		preview.Content, false,
	)
	if err != nil {
		t.Fatal(err)
	}
	if result.NoteID == 0 || replayed.NoteID != result.NoteID || !replayed.Replayed {
		t.Fatalf("result %#v replay %#v", result, replayed)
	}
	detail, err := fixture.app.GetTaskDetailV2(1, fixture.taskID)
	if err != nil {
		t.Fatal(err)
	}
	if detail.Notes[0].Kind != "decision" {
		t.Fatalf("typed task detail note = %#v", detail.Notes[0])
	}
	s := openWritebackStore(t, fixture)
	defer s.Close()
	notes, _ := s.NotesByTask(fixture.taskID)
	if len(notes) != 2 || notes[1].Kind != model.MemoryDecision ||
		notes[1].Body != preview.Content {
		t.Fatalf("task notes = %#v", notes)
	}
	if len(broker.issuedProfiles) != 1 || broker.boundSession != launched.SessionID ||
		len(broker.revokedTokens) != 0 || len(broker.revokedSessions) != 0 {
		t.Fatalf("write-back touched capabilities: %#v", broker)
	}
}

func TestTerminalWritebackSupportsPlanProjectAndExplicitSummary(t *testing.T) {
	t.Run("plan blocker", func(t *testing.T) {
		fixture := newLinkedLaunchFixture(t)
		launched, err := fixture.app.LaunchLinkedAgentV2(1, "agent-beta", "", 24, 80, fixture.taskPointer())
		if err != nil {
			t.Fatal(err)
		}
		mutation, err := fixture.app.MutateTerminalAssociationV2(1, launched.SessionID, 1, false, association.PointerV1{
			Version: association.VersionV1, PlanID: fixture.planID,
		})
		if err != nil {
			t.Fatal(err)
		}
		result, err := fixture.app.WriteTerminalMemoryV2(1, launched.SessionID, mutation.Revision, "plan-blocker", "blocker", "Awaiting product decision", false)
		if err != nil {
			t.Fatal(err)
		}
		if result.Destination != "Plan #1" {
			t.Fatalf("result = %#v", result)
		}
		s := openWritebackStore(t, fixture)
		defer s.Close()
		notes, _ := s.NotesByPlan(fixture.planID)
		if len(notes) != 1 || notes[0].Kind != model.MemoryBlocker {
			t.Fatalf("plan notes = %#v", notes)
		}
	})

	t.Run("project handoff", func(t *testing.T) {
		fixture := newLinkedLaunchFixture(t)
		if _, err := fixture.app.AssociateTerminalV2(1, "linked-session", association.PointerV1{Version: association.VersionV1}); err != nil {
			t.Fatal(err)
		}
		result, err := fixture.app.WriteTerminalMemoryV2(1, "linked-session", 1, "project-handoff", "handoff", "Resume with the project audit", false)
		if err != nil {
			t.Fatal(err)
		}
		if result.Destination != "Project" {
			t.Fatalf("result = %#v", result)
		}
		s := openWritebackStore(t, fixture)
		defer s.Close()
		notes, _ := s.ListNotes()
		last := notes[len(notes)-1]
		if last.Target != model.TargetProject || last.Kind != model.MemoryHandoff {
			t.Fatalf("project note = %#v", last)
		}
	})

	t.Run("summary requires confirmation", func(t *testing.T) {
		fixture := newLinkedLaunchFixture(t)
		launched, err := fixture.app.LaunchLinkedAgentV2(1, "agent-beta", "", 24, 80, fixture.taskPointer())
		if err != nil {
			t.Fatal(err)
		}
		_, err = fixture.app.WriteTerminalMemoryV2(1, launched.SessionID, 1, "summary-1", "summary", "New bounded handoff", false)
		if !errors.Is(err, ErrTerminalWritebackConfirm) {
			t.Fatalf("confirmation error = %v", err)
		}
		s := openWritebackStore(t, fixture)
		meta, _ := s.GetMeta()
		if meta.Summary != "" {
			t.Fatalf("unconfirmed summary mutated storage: %q", meta.Summary)
		}
		s.Close()
		preview, err := fixture.app.PreviewTerminalWritebackV2(1, launched.SessionID, 1, "summary", "New bounded handoff")
		if err != nil || !preview.ReplacesSummary || preview.Destination != "Project rolling summary" {
			t.Fatalf("summary preview = %#v err %v", preview, err)
		}
		if _, err := fixture.app.WriteTerminalMemoryV2(1, launched.SessionID, 1, "summary-1", "summary", "New bounded handoff", true); err != nil {
			t.Fatal(err)
		}
		s = openWritebackStore(t, fixture)
		defer s.Close()
		meta, _ = s.GetMeta()
		if meta.Summary != "New bounded handoff" {
			t.Fatalf("summary = %q", meta.Summary)
		}
	})
}

func TestTerminalWritebackRejectsStaleDetachedClosedAndMovedTargets(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	launched, err := fixture.app.LaunchLinkedAgentV2(1, "agent-beta", "", 24, 80, fixture.taskPointer())
	if err != nil {
		t.Fatal(err)
	}
	assertNoWrite := func(name string, call func() error) {
		t.Helper()
		s := openWritebackStore(t, fixture)
		before, _ := s.ListNotes()
		s.Close()
		if err := call(); err == nil {
			t.Fatalf("%s unexpectedly succeeded", name)
		}
		s = openWritebackStore(t, fixture)
		after, _ := s.ListNotes()
		s.Close()
		if len(after) != len(before) {
			t.Fatalf("%s mutated notes: %d -> %d", name, len(before), len(after))
		}
	}
	assertNoWrite("stale generation", func() error {
		_, err := fixture.app.WriteTerminalMemoryV2(2, launched.SessionID, 1, "stale-generation", "decision", "safe", false)
		return err
	})
	assertNoWrite("stale revision", func() error {
		_, err := fixture.app.WriteTerminalMemoryV2(1, launched.SessionID, 2, "stale-revision", "decision", "safe", false)
		return err
	})
	fixture.manager.mu.Lock()
	originalAssociation := *fixture.manager.association
	fixture.manager.association.ProjectRoot = fixture.root + "-other"
	fixture.manager.mu.Unlock()
	assertNoWrite("wrong project", func() error {
		_, err := fixture.app.WriteTerminalMemoryV2(1, launched.SessionID, 1, "wrong-project", "decision", "safe", false)
		return err
	})
	fixture.manager.mu.Lock()
	fixture.manager.association = &originalAssociation
	fixture.manager.mu.Unlock()
	if _, err := fixture.app.MutateTerminalAssociationV2(1, launched.SessionID, 1, true, association.PointerV1{}); err != nil {
		t.Fatal(err)
	}
	assertNoWrite("detached", func() error {
		_, err := fixture.app.WriteTerminalMemoryV2(1, launched.SessionID, 2, "detached", "decision", "safe", false)
		return err
	})
	if _, err := fixture.app.MutateTerminalAssociationV2(1, launched.SessionID, 2, false, fixture.taskPointer()); err != nil {
		t.Fatal(err)
	}
	s := openWritebackStore(t, fixture)
	other, _ := s.AddPlan("Other")
	if err := s.SetTaskPlan(fixture.taskID, other.ID); err != nil {
		t.Fatal(err)
	}
	s.Close()
	assertNoWrite("moved task", func() error {
		_, err := fixture.app.WriteTerminalMemoryV2(1, launched.SessionID, 3, "moved-task", "decision", "safe", false)
		return err
	})
	fixture.manager.mu.Lock()
	fixture.manager.createResult.State = "closed"
	fixture.manager.mu.Unlock()
	assertNoWrite("closed", func() error {
		_, err := fixture.app.WriteTerminalMemoryV2(1, launched.SessionID, 3, "closed", "decision", "safe", false)
		return err
	})
}

func TestTerminalWritebackRejectsCredentialsAndHardBoundsWithoutEcho(t *testing.T) {
	credential := "token=FORBIDDEN_WRITEBACK_CREDENTIAL_CANARY"
	_, _, err := validateTerminalWritebackContent("decision", credential)
	if !errors.Is(err, ErrTerminalWritebackCredential) {
		t.Fatalf("credential error = %v", err)
	}
	if strings.Contains(err.Error(), "FORBIDDEN") {
		t.Fatalf("credential error echoed content: %v", err)
	}
	laterURLCredential := "https://username@example.test then " +
		"https://later:FORBIDDEN_LATER_URL_PASSWORD@example.test"
	if _, _, err := validateTerminalWritebackContent(
		"decision", laterURLCredential,
	); !errors.Is(err, ErrTerminalWritebackCredential) {
		t.Fatalf("later URL credential error = %v", err)
	}
	for name, value := range map[string]string{
		"invalid utf8":   string([]byte{0xff}),
		"too many bytes": strings.Repeat("界", TerminalWritebackMaxBytes/3+1),
		"too many lines": strings.Repeat("line\n", terminalWritebackMaxLines+1),
		"control":        "safe\x00unsafe",
	} {
		t.Run(name, func(t *testing.T) {
			if _, _, err := validateTerminalWritebackContent("decision", value); !errors.Is(err, ErrTerminalWritebackContent) {
				t.Fatalf("error = %v", err)
			}
		})
	}
	valid := strings.Repeat("界", 100)
	_, normalized, err := validateTerminalWritebackContent("handoff", valid)
	if err != nil || normalized != valid {
		t.Fatalf("valid multibyte content rejected: %v", err)
	}
}

func TestTerminalExitAndAgentResultNeverWriteMemory(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	launched, err := fixture.app.LaunchLinkedAgentV2(1, "agent-beta", "", 24, 80, fixture.taskPointer())
	if err != nil {
		t.Fatal(err)
	}
	canary := "FORBIDDEN_RAW_AGENT_RESULT"
	if !fixture.registry.RecordTerminalExit(launched.SessionID, 7, canary) {
		t.Fatal("failed to record linked run exit")
	}
	s := openWritebackStore(t, fixture)
	defer s.Close()
	notes, _ := s.ListNotes()
	for _, note := range notes {
		if strings.Contains(note.Body, canary) {
			t.Fatalf("AgentRun result was persisted as memory: %#v", note)
		}
	}
}

func openWritebackStore(t *testing.T, fixture linkedLaunchFixture) *store.Store {
	t.Helper()
	s, err := store.Open(fixture.app.workspace.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	return s
}
