package store

import (
	"errors"
	"fmt"
	"sync"
	"testing"

	"github.com/ro-ag/ptrack/internal/model"
	bolt "go.etcd.io/bbolt"
)

func TestWriteMemoryKindsScopesAndSummaryReplacement(t *testing.T) {
	s := openTemp(t)
	plan, _ := s.AddPlan("plan")
	task, _ := s.AddTask(plan.ID, "task")
	if err := s.SetSummary("old summary"); err != nil {
		t.Fatal(err)
	}

	tests := []MemoryWriteRequest{
		memoryRequest("decision-1", model.MemoryDecision, "choose bounded writes", model.TargetProject, 0, 0),
		memoryRequest("blocker-1", model.MemoryBlocker, "waiting on review", model.TargetPlan, plan.ID, plan.ID),
		memoryRequest("handoff-1", model.MemoryHandoff, "resume at validation", model.TargetTask, task.ID, plan.ID),
	}
	for _, request := range tests {
		result, err := s.WriteMemory(request)
		if err != nil {
			t.Fatalf("WriteMemory(%s): %v", request.Kind, err)
		}
		if result.Note == nil || result.Note.Kind != request.Kind ||
			result.Note.Target != request.Target || result.Note.TargetID != request.TargetID {
			t.Fatalf("WriteMemory(%s) = %#v", request.Kind, result)
		}
	}
	summaryRequest := memoryRequest("summary-1", model.MemorySummary, "new summary", model.TargetTask, task.ID, plan.ID)
	result, err := s.WriteMemory(summaryRequest)
	if err != nil {
		t.Fatal(err)
	}
	if result.Summary != "new summary" || result.Note != nil {
		t.Fatalf("summary result = %#v", result)
	}
	meta, _ := s.GetMeta()
	if meta.Summary != "new summary" {
		t.Fatalf("summary = %q", meta.Summary)
	}
	taskAfter, _ := s.GetTask(task.ID)
	if taskAfter.Status != model.TaskTodo {
		t.Fatalf("write-back changed task status to %q", taskAfter.Status)
	}
}

func TestWriteMemoryIdempotencyCollisionAndTargetValidationAreAtomic(t *testing.T) {
	s := openTemp(t)
	plan, _ := s.AddPlan("plan")
	other, _ := s.AddPlan("other")
	task, _ := s.AddTask(plan.ID, "task")
	request := memoryRequest("stable-request", model.MemoryDecision, "one note", model.TargetTask, task.ID, plan.ID)
	first, err := s.WriteMemory(request)
	if err != nil {
		t.Fatal(err)
	}
	replayed, err := s.WriteMemory(request)
	if err != nil {
		t.Fatal(err)
	}
	if !replayed.Replayed || replayed.Note == nil || replayed.Note.ID != first.Note.ID {
		t.Fatalf("replay = %#v first = %#v", replayed, first)
	}
	request.Body = "different note"
	if _, err := s.WriteMemory(request); !errors.Is(err, ErrMemoryWritebackReplay) {
		t.Fatalf("collision error = %v", err)
	}

	if err := s.SetTaskPlan(task.ID, other.ID); err != nil {
		t.Fatal(err)
	}
	before, _ := s.ListNotes()
	stale := memoryRequest("stale-task", model.MemoryBlocker, "must not persist", model.TargetTask, task.ID, plan.ID)
	_, err = s.WriteMemory(stale)
	if !errors.Is(err, ErrInvalidMemoryWriteback) {
		t.Fatalf("stale task error = %v", err)
	}
	after, _ := s.ListNotes()
	if len(after) != len(before) {
		t.Fatalf("failed target validation wrote a note: %d -> %d", len(before), len(after))
	}
}

func TestWriteMemorySerializesWithConcurrentTaskConversion(t *testing.T) {
	s := openTemp(t)
	plan, _ := s.AddPlan("plan")
	task, _ := s.AddTask(plan.ID, "task")
	request := memoryRequest(
		"concurrent-conversion", model.MemoryHandoff, "resume after conversion",
		model.TargetTask, task.ID, plan.ID,
	)
	start := make(chan struct{})
	var writeResult MemoryWriteResult
	var writeErr error
	var converted model.Plan
	var convertErr error
	var wait sync.WaitGroup
	wait.Add(2)
	go func() {
		defer wait.Done()
		<-start
		writeResult, writeErr = s.WriteMemory(request)
	}()
	go func() {
		defer wait.Done()
		<-start
		converted, convertErr = s.ConvertTaskToPlan(task.ID)
	}()
	close(start)
	wait.Wait()
	if convertErr != nil {
		t.Fatal(convertErr)
	}
	notes, err := s.ListNotes()
	if err != nil {
		t.Fatal(err)
	}
	if writeErr != nil {
		if len(notes) != 0 {
			t.Fatalf("failed concurrent write left notes: %#v", notes)
		}
		return
	}
	if writeResult.Note == nil || len(notes) != 1 ||
		notes[0].Target != model.TargetPlan || notes[0].TargetID != converted.ID ||
		notes[0].Kind != model.MemoryHandoff {
		t.Fatalf("concurrent converted memory = result %#v notes %#v plan %#v", writeResult, notes, converted)
	}
}

func TestWriteMemoryReplayReceiptsAreBounded(t *testing.T) {
	s := openTemp(t)
	for index := 0; index < MemoryWritebackReplayLimit+5; index++ {
		request := memoryRequest(fmt.Sprintf("request-%03d", index), model.MemoryDecision, fmt.Sprintf("decision %d", index), model.TargetProject, 0, 0)
		_, err := s.WriteMemory(request)
		if err != nil {
			t.Fatal(err)
		}
	}
	err := s.db.View(func(tx *bolt.Tx) error {
		if got := tx.Bucket(bucketMemoryWritebacks).Stats().KeyN; got != MemoryWritebackReplayLimit {
			t.Fatalf("replay receipts = %d want %d", got, MemoryWritebackReplayLimit)
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
}

func memoryRequest(
	requestID string,
	kind model.MemoryKind,
	body string,
	target model.NoteTarget,
	targetID uint64,
	planID uint64,
) MemoryWriteRequest {
	return MemoryWriteRequest{
		RequestID: requestID, Kind: kind, Body: body,
		Target: target, TargetID: targetID, PlanID: planID,
		WorkspaceGeneration: 1, SessionID: "session-1", AssociationRevision: 1,
	}
}

func TestLegacyNotesDecodeWithoutTypedKind(t *testing.T) {
	s := openTemp(t)
	note, err := s.AddNote(model.TargetProject, 0, "legacy")
	if err != nil {
		t.Fatal(err)
	}
	notes, err := s.ListNotes()
	if err != nil {
		t.Fatal(err)
	}
	if len(notes) != 1 || notes[0].ID != note.ID || notes[0].Kind != "" {
		t.Fatalf("legacy notes = %#v", notes)
	}
}
