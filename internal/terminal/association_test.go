package terminal

import (
	"errors"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/association"
)

type terminalAssociationCatalog struct{}

func (terminalAssociationCatalog) ValidatePlan(planID uint64) error {
	if planID != 2 {
		return errors.New("not found")
	}
	return nil
}

func (terminalAssociationCatalog) TaskPlan(taskID uint64) (uint64, error) {
	if taskID != 9 {
		return 0, errors.New("not found")
	}
	return 2, nil
}

func TestManagerAssociatesSessionWithHostOwnedMonotonicContext(t *testing.T) {
	manager := newManagerWithSession(t)
	host, err := association.NewHost(manager.projectRoot, 5, terminalAssociationCatalog{})
	if err != nil {
		t.Fatal(err)
	}
	session := manager.SessionSnapshot(1)[0]
	first, err := manager.Associate(session.ID, host, association.PointerV1{
		Version: association.VersionV1, PlanID: 2, TaskID: 9,
	})
	if err != nil {
		t.Fatal(err)
	}
	second, err := manager.Associate(session.ID, host, association.PointerV1{
		Version: association.VersionV1, PlanID: 2,
	})
	if err != nil {
		t.Fatal(err)
	}
	if first.LiveID != session.ID || first.Generation != 5 || first.Revision != 1 ||
		second.Revision != 2 || second.Target.TaskID != 0 {
		t.Fatalf("associations = first %#v second %#v", first, second)
	}
	snapshot := manager.SessionSnapshot(1)
	if snapshot[0].Association == nil || snapshot[0].Association.Revision != 2 {
		t.Fatalf("session snapshot = %#v", snapshot[0])
	}
}

func TestManagerAssociationRejectsUnknownSessionAndInvalidPointer(t *testing.T) {
	manager := newManagerWithSession(t)
	host, err := association.NewHost(manager.projectRoot, 1, terminalAssociationCatalog{})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := manager.Associate("missing", host, association.PointerV1{
		Version: association.VersionV1,
	}); !errors.Is(err, ErrSessionNotFound) {
		t.Fatalf("missing session = %v", err)
	}
	session := manager.SessionSnapshot(1)[0]
	if _, err := manager.Associate(session.ID, host, association.PointerV1{
		Version: 2,
	}); !errors.Is(err, association.ErrUnsupportedVersion) {
		t.Fatalf("unsupported pointer = %v", err)
	}
}

func TestManagerAssociationRejectsExitedSession(t *testing.T) {
	manager := newManagerWithSession(t)
	host, err := association.NewHost(manager.projectRoot, 1, terminalAssociationCatalog{})
	if err != nil {
		t.Fatal(err)
	}
	sessionID := manager.SessionSnapshot(1)[0].ID
	session, err := manager.Get(sessionID)
	if err != nil {
		t.Fatal(err)
	}
	session.mu.Lock()
	session.state = SessionExited
	session.mu.Unlock()
	if _, err := manager.Associate(sessionID, host, association.PointerV1{
		Version: association.VersionV1, PlanID: 2, TaskID: 9,
	}); !errors.Is(err, ErrSessionNotFound) {
		t.Fatalf("exited session association = %v", err)
	}
	if current := manager.SessionSnapshot(1)[0].Association; current != nil {
		t.Fatalf("exited session published association: %#v", current)
	}
}

func TestManagerAssociationChangeIsRevisionFencedAndRollbackSafe(t *testing.T) {
	manager := newManagerWithSession(t)
	host, err := association.NewHost(manager.projectRoot, 5, terminalAssociationCatalog{})
	if err != nil {
		t.Fatal(err)
	}
	sessionID := manager.SessionSnapshot(1)[0].ID
	first, err := manager.Associate(sessionID, host, association.PointerV1{
		Version: association.VersionV1, PlanID: 2, TaskID: 9,
	})
	if err != nil {
		t.Fatal(err)
	}
	change, err := manager.PrepareAssociationChange(
		sessionID,
		host,
		association.PointerV1{Version: association.VersionV1, PlanID: 2},
		first.Revision,
	)
	if err != nil {
		t.Fatal(err)
	}
	if current := manager.SessionSnapshot(1)[0].Association; current == nil ||
		current.Revision != first.Revision || current.Target.TaskID != 9 {
		t.Fatalf("prepare changed session = %#v", current)
	}
	if err := manager.CommitAssociationChange(change); err != nil {
		t.Fatal(err)
	}
	if current := manager.SessionSnapshot(1)[0].Association; current == nil ||
		current.Revision != 2 || current.Target.TaskID != 0 {
		t.Fatalf("commit = %#v", current)
	}
	if err := manager.CommitAssociationChange(change); !errors.Is(err, association.ErrStaleAssociation) {
		t.Fatalf("replayed commit = %v", err)
	}
	if err := manager.RollbackAssociationChange(change); err != nil {
		t.Fatal(err)
	}
	if current := manager.SessionSnapshot(1)[0].Association; current == nil ||
		*current != first {
		t.Fatalf("rollback = %#v", current)
	}
	if _, err := manager.PrepareAssociationChange(
		sessionID,
		host,
		association.PointerV1{Version: association.VersionV1},
		2,
	); !errors.Is(err, association.ErrStaleAssociation) {
		t.Fatalf("stale prepare = %v", err)
	}
}

func TestManagerWithLiveAssociationFencesProcessExit(t *testing.T) {
	manager := newManagerWithSession(t)
	host, err := association.NewHost(manager.projectRoot, 5, terminalAssociationCatalog{})
	if err != nil {
		t.Fatal(err)
	}
	sessionID := manager.SessionSnapshot(1)[0].ID
	bound, err := manager.Associate(sessionID, host, association.PointerV1{
		Version: association.VersionV1, PlanID: 2, TaskID: 9,
	})
	if err != nil {
		t.Fatal(err)
	}
	session, err := manager.Get(sessionID)
	if err != nil {
		t.Fatal(err)
	}
	entered := make(chan struct{})
	release := make(chan struct{})
	used := make(chan error, 1)
	go func() {
		used <- manager.WithLiveAssociation(sessionID, bound.Revision, func(current association.AssociationV1) error {
			close(entered)
			<-release
			if current != bound {
				t.Errorf("association = %#v want %#v", current, bound)
			}
			return nil
		})
	}()
	<-entered
	exited := make(chan struct{})
	go func() {
		session.mu.Lock()
		session.state = SessionExited
		session.mu.Unlock()
		close(exited)
	}()
	select {
	case <-exited:
		t.Fatal("process exit crossed the live-association fence")
	case <-time.After(10 * time.Millisecond):
	}
	close(release)
	if err := <-used; err != nil {
		t.Fatal(err)
	}
	<-exited
	if err := manager.WithLiveAssociation(sessionID, bound.Revision, func(association.AssociationV1) error {
		return nil
	}); !errors.Is(err, association.ErrStaleAssociation) {
		t.Fatalf("exited session association = %v", err)
	}
}

func newManagerWithSession(t *testing.T) *Manager {
	t.Helper()
	process := newManagerFakeProcess()
	manager := newManagerForTest(t, t.TempDir(), newManagerFakeFactory(
		managerStartOutcome{process: process},
	))
	cleanupManager(t, manager, process)
	if _, err := manager.Create("agent", "", 24, 80); err != nil {
		t.Fatalf("Create: %v", err)
	}
	return manager
}
