package model

import (
	"bytes"
	"encoding/gob"
	"reflect"
	"testing"
	"time"
)

func gobRoundTrip[T any](t *testing.T, in T) T {
	t.Helper()
	var buf bytes.Buffer
	if err := gob.NewEncoder(&buf).Encode(in); err != nil {
		t.Fatalf("encode: %v", err)
	}
	var out T
	if err := gob.NewDecoder(&buf).Decode(&out); err != nil {
		t.Fatalf("decode: %v", err)
	}
	return out
}

func TestGobRoundTrip(t *testing.T) {
	now := time.Date(2026, 7, 12, 10, 30, 0, 0, time.UTC)

	meta := Meta{Goal: "ship it", Summary: "wip", ActivePlan: 3, CreatedAt: now, UpdatedAt: now}
	if got := gobRoundTrip(t, meta); !reflect.DeepEqual(got, meta) {
		t.Errorf("meta mismatch: got %+v want %+v", got, meta)
	}

	plan := Plan{ID: 1, Title: "storage", Status: PlanActive, Order: 0, CreatedAt: now, UpdatedAt: now}
	if got := gobRoundTrip(t, plan); !reflect.DeepEqual(got, plan) {
		t.Errorf("plan mismatch: got %+v want %+v", got, plan)
	}

	task := Task{ID: 2, PlanID: 1, Title: "buckets", Status: TaskDoing, Order: 1, CreatedAt: now, UpdatedAt: now}
	if got := gobRoundTrip(t, task); !reflect.DeepEqual(got, task) {
		t.Errorf("task mismatch: got %+v want %+v", got, task)
	}

	note := Note{ID: 5, Target: TargetTask, TargetID: 2, Body: "use NextSequence", CreatedAt: now}
	if got := gobRoundTrip(t, note); !reflect.DeepEqual(got, note) {
		t.Errorf("note mismatch: got %+v want %+v", got, note)
	}

	ref := ProjectRef{Name: "ptrack", Path: "/tmp/ptrack", LastSeen: now}
	if got := gobRoundTrip(t, ref); !reflect.DeepEqual(got, ref) {
		t.Errorf("ref mismatch: got %+v want %+v", got, ref)
	}
}

func TestTaskStatusOpen(t *testing.T) {
	cases := map[TaskStatus]bool{
		TaskTodo:    true,
		TaskDoing:   true,
		TaskBlocked: true,
		TaskDone:    false,
	}
	for st, want := range cases {
		if got := st.Open(); got != want {
			t.Errorf("%s.Open() = %v want %v", st, got, want)
		}
	}
}
