package association

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"testing"
)

type testCatalog struct {
	plans map[uint64]bool
	tasks map[uint64]uint64
}

func (c testCatalog) ValidatePlan(planID uint64) error {
	if !c.plans[planID] {
		return errors.New("not found")
	}
	return nil
}

func (c testCatalog) TaskPlan(taskID uint64) (uint64, error) {
	planID, ok := c.tasks[taskID]
	if !ok {
		return 0, errors.New("not found")
	}
	return planID, nil
}

func TestHostValidatesTargetsAndMintsMonotonicAssociations(t *testing.T) {
	root := t.TempDir()
	alias := filepath.Join(t.TempDir(), "project")
	if err := os.Symlink(root, alias); err != nil {
		t.Fatal(err)
	}
	host, err := NewHost(alias, 7, testCatalog{
		plans: map[uint64]bool{2: true},
		tasks: map[uint64]uint64{9: 2},
	})
	if err != nil {
		t.Fatal(err)
	}
	canonicalRoot, err := filepath.EvalSymlinks(root)
	if err != nil {
		t.Fatal(err)
	}
	first, err := host.Bind("opaque-live-id", PointerV1{
		Version: VersionV1, PlanID: 2, TaskID: 9,
	}, nil)
	if err != nil {
		t.Fatal(err)
	}
	if first.ProjectRoot != canonicalRoot || first.Generation != 7 ||
		first.LiveID != "opaque-live-id" || first.Revision != 1 ||
		first.Target.PlanID != 2 || first.Target.TaskID != 9 {
		t.Fatalf("first association = %#v", first)
	}
	second, err := host.Bind("opaque-live-id", PointerV1{
		Version: VersionV1, PlanID: 2,
	}, &first)
	if err != nil {
		t.Fatal(err)
	}
	if second.Revision != 2 || second.Target.PlanID != 2 || second.Target.TaskID != 0 {
		t.Fatalf("second association = %#v", second)
	}
}

func TestHostRejectsUnsupportedStaleAndMismatchedAssociations(t *testing.T) {
	root := t.TempDir()
	host, err := NewHost(root, 3, testCatalog{
		plans: map[uint64]bool{1: true, 2: true},
		tasks: map[uint64]uint64{8: 2},
	})
	if err != nil {
		t.Fatal(err)
	}
	tests := []struct {
		name    string
		pointer PointerV1
		want    error
	}{
		{"unsupported version", PointerV1{Version: 2}, ErrUnsupportedVersion},
		{"task without plan", PointerV1{Version: 1, TaskID: 8}, ErrInvalidTarget},
		{"missing plan", PointerV1{Version: 1, PlanID: 99}, ErrInvalidTarget},
		{"missing task", PointerV1{Version: 1, PlanID: 2, TaskID: 99}, ErrInvalidTarget},
		{"mismatched task", PointerV1{Version: 1, PlanID: 1, TaskID: 8}, ErrInvalidTarget},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if _, err := host.Bind("live", test.pointer, nil); !errors.Is(err, test.want) {
				t.Fatalf("Bind = %v, want %v", err, test.want)
			}
		})
	}

	prior, err := host.Bind("live", PointerV1{Version: 1}, nil)
	if err != nil {
		t.Fatal(err)
	}
	prior.Generation = 2
	if _, err := host.Bind("live", PointerV1{Version: 1}, &prior); !errors.Is(err, ErrStaleAssociation) {
		t.Fatalf("stale Bind = %v, want ErrStaleAssociation", err)
	}
	if _, err := NewHost(root, 0, nil); !errors.Is(err, ErrStaleAssociation) {
		t.Fatalf("zero-generation NewHost = %v, want ErrStaleAssociation", err)
	}
}

func TestAssociationJSONContainsOnlyLiveContextMetadata(t *testing.T) {
	encoded, err := json.Marshal(AssociationV1{
		Version:     VersionV1,
		ProjectRoot: "/project",
		Generation:  3,
		LiveID:      "opaque-live-id",
		Target:      TargetV1{PlanID: 2, TaskID: 9},
		Revision:    4,
	})
	if err != nil {
		t.Fatal(err)
	}
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(encoded, &fields); err != nil {
		t.Fatal(err)
	}
	for _, field := range []string{
		"version", "projectRoot", "generation", "liveId", "target", "revision",
	} {
		if _, ok := fields[field]; !ok {
			t.Fatalf("association JSON missing %q: %s", field, encoded)
		}
		delete(fields, field)
	}
	if len(fields) != 0 {
		t.Fatalf("association JSON exposes unexpected fields: %v", fields)
	}
	var target map[string]json.RawMessage
	if err := json.Unmarshal(json.RawMessage(`{"planId":2,"taskId":9}`), &target); err != nil {
		t.Fatal(err)
	}
	var actualTarget map[string]json.RawMessage
	associationJSON := struct {
		Target json.RawMessage `json:"target"`
	}{}
	if err := json.Unmarshal(encoded, &associationJSON); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(associationJSON.Target, &actualTarget); err != nil {
		t.Fatal(err)
	}
	if len(actualTarget) != len(target) || actualTarget["planId"] == nil || actualTarget["taskId"] == nil {
		t.Fatalf("association target JSON = %s", associationJSON.Target)
	}
}
