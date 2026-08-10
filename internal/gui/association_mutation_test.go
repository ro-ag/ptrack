package gui

import (
	"context"
	"errors"
	"reflect"
	"sync"
	"testing"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/store"
)

func TestMutateTerminalAssociationRelinkDetachIsMonotonicAndLifecycleFree(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	broker := &fakeWorkspaceCapabilityBroker{token: "host-token"}
	fixture.app.workspace.capabilities = broker
	launched, err := fixture.app.LaunchLinkedAgentV2(
		1, "agent-beta", "", 24, 80, fixture.taskPointer(),
	)
	if err != nil {
		t.Fatal(err)
	}
	if launched.AssociationRevision != 1 {
		t.Fatalf("launch revision = %d", launched.AssociationRevision)
	}
	beforeCreate := fixture.manager.lastCreate()
	beforeRun := fixture.registry.Snapshot(1)[0]
	beforeIssued := append([]string{}, broker.issuedProfiles...)
	beforeBoundToken := broker.boundToken
	beforeBoundSession := broker.boundSession

	planPointer := association.PointerV1{
		Version: association.VersionV1,
		PlanID:  fixture.planID,
	}
	planResult, err := fixture.app.MutateTerminalAssociationV2(
		1, launched.SessionID, 1, false, planPointer,
	)
	if err != nil {
		t.Fatal(err)
	}
	if planResult.Revision != 2 || planResult.Detached ||
		planResult.Pointer == nil || *planResult.Pointer != planPointer {
		t.Fatalf("plan relink = %#v", planResult)
	}
	taskResult, err := fixture.app.MutateTerminalAssociationV2(
		1, launched.SessionID, 2, false, fixture.taskPointer(),
	)
	if err != nil {
		t.Fatal(err)
	}
	if taskResult.Revision != 3 || taskResult.Pointer == nil ||
		*taskResult.Pointer != fixture.taskPointer() {
		t.Fatalf("task relink = %#v", taskResult)
	}
	detached, err := fixture.app.MutateTerminalAssociationV2(
		1, launched.SessionID, 3, true, association.PointerV1{},
	)
	if err != nil {
		t.Fatal(err)
	}
	if detached.Revision != 4 || !detached.Detached || detached.Pointer != nil {
		t.Fatalf("detach = %#v", detached)
	}
	terminalAssociation := fixture.manager.association
	afterRun := fixture.registry.Snapshot(1)[0]
	if terminalAssociation == nil || afterRun.Association == nil ||
		terminalAssociation.Target != (association.TargetV1{}) ||
		afterRun.Association.Target != (association.TargetV1{}) ||
		terminalAssociation.Revision != 4 || afterRun.Association.Revision != 4 {
		t.Fatalf("detached pair = terminal %#v run %#v", terminalAssociation, afterRun)
	}
	if len(fixture.manager.creates) != 1 || len(fixture.manager.closes) != 0 ||
		!reflect.DeepEqual(beforeCreate, fixture.manager.lastCreate()) {
		t.Fatalf("mutation changed terminal lifecycle: creates %#v closes %#v",
			fixture.manager.creates, fixture.manager.closes)
	}
	if afterRun.ID != beforeRun.ID || afterRun.TerminalID != beforeRun.TerminalID ||
		afterRun.PID != beforeRun.PID || afterRun.Profile != beforeRun.Profile ||
		afterRun.Provider != beforeRun.Provider || afterRun.CWD != beforeRun.CWD ||
		afterRun.StartedAt != beforeRun.StartedAt {
		t.Fatalf("mutation changed runtime identity: before %#v after %#v", beforeRun, afterRun)
	}
	if !reflect.DeepEqual(broker.issuedProfiles, beforeIssued) ||
		broker.boundToken != beforeBoundToken || broker.boundSession != beforeBoundSession ||
		len(broker.revokedTokens) != 0 || len(broker.revokedSessions) != 0 {
		t.Fatalf("mutation changed capability identity: %#v", broker)
	}
	if !fixture.registry.IsLinkedLaunchRun(beforeRun.ID) ||
		!fixture.registry.HasLinkedTerminal(launched.SessionID) {
		t.Fatal("detach lost linked-launch provenance")
	}

	relinked, err := fixture.app.MutateTerminalAssociationV2(
		1, launched.SessionID, 4, false, fixture.taskPointer(),
	)
	if err != nil || relinked.Revision != 5 {
		t.Fatalf("relink after detach = %#v err %v", relinked, err)
	}
}

func TestMutateTerminalAssociationRejectsStaleInvalidAndMissingWithoutChanges(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	launched, err := fixture.app.LaunchLinkedAgentV2(
		1, "agent-beta", "", 24, 80, fixture.taskPointer(),
	)
	if err != nil {
		t.Fatal(err)
	}
	beforeTerminal := *fixture.manager.association
	beforeRun := fixture.registry.Snapshot(1)[0]
	s, err := store.Open(fixture.app.workspace.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	otherPlan, err := s.AddPlan("Other plan")
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}

	tests := []struct {
		name       string
		generation uint64
		sessionID  string
		revision   uint64
		detach     bool
		pointer    association.PointerV1
		want       error
	}{
		{
			name: "stale generation", generation: 2, sessionID: launched.SessionID,
			revision: 1, pointer: fixture.taskPointer(), want: errStaleWorkspaceGeneration,
		},
		{
			name: "stale revision", generation: 1, sessionID: launched.SessionID,
			revision: 0, pointer: fixture.taskPointer(), want: association.ErrStaleAssociation,
		},
		{
			name: "missing session", generation: 1, sessionID: "missing",
			revision: 1, pointer: fixture.taskPointer(), want: nil,
		},
		{
			name: "task ownership mismatch", generation: 1, sessionID: launched.SessionID,
			revision: 1, pointer: association.PointerV1{
				Version: association.VersionV1, PlanID: otherPlan.ID, TaskID: fixture.taskID,
			}, want: association.ErrInvalidTarget,
		},
		{
			name: "project-only relink", generation: 1, sessionID: launched.SessionID,
			revision: 1, pointer: association.PointerV1{Version: association.VersionV1},
			want: association.ErrInvalidTarget,
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			_, gotErr := fixture.app.MutateTerminalAssociationV2(
				test.generation,
				test.sessionID,
				test.revision,
				test.detach,
				test.pointer,
			)
			if gotErr == nil || (test.want != nil && !errors.Is(gotErr, test.want)) {
				t.Fatalf("error = %v, want %v", gotErr, test.want)
			}
		})
	}
	if fixture.manager.association == nil || *fixture.manager.association != beforeTerminal ||
		!reflect.DeepEqual(fixture.registry.Snapshot(1)[0], beforeRun) ||
		len(fixture.manager.creates) != 1 || len(fixture.manager.closes) != 0 {
		t.Fatal("rejected mutation changed live resources")
	}
}

func TestMutateTerminalAssociationSerializesConcurrentExpectedRevision(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	launched, err := fixture.app.LaunchLinkedAgentV2(
		1, "agent-beta", "", 24, 80, fixture.taskPointer(),
	)
	if err != nil {
		t.Fatal(err)
	}
	pointer := association.PointerV1{
		Version: association.VersionV1, PlanID: fixture.planID,
	}
	start := make(chan struct{})
	errorsSeen := make(chan error, 2)
	var wait sync.WaitGroup
	for range 2 {
		wait.Add(1)
		go func() {
			defer wait.Done()
			<-start
			_, callErr := fixture.app.MutateTerminalAssociationV2(
				1, launched.SessionID, 1, false, pointer,
			)
			errorsSeen <- callErr
		}()
	}
	close(start)
	wait.Wait()
	close(errorsSeen)
	succeeded, stale := 0, 0
	for callErr := range errorsSeen {
		if callErr == nil {
			succeeded++
		} else if errors.Is(callErr, association.ErrStaleAssociation) {
			stale++
		} else {
			t.Fatalf("unexpected concurrent error: %v", callErr)
		}
	}
	if succeeded != 1 || stale != 1 {
		t.Fatalf("concurrent results = success %d stale %d", succeeded, stale)
	}
	terminalAssociation := fixture.manager.association
	runAssociation := fixture.registry.Snapshot(1)[0].Association
	if terminalAssociation == nil || runAssociation == nil ||
		terminalAssociation.Revision != 2 || runAssociation.Revision != 2 ||
		terminalAssociation.Target != runAssociation.Target {
		t.Fatalf("concurrent pair = terminal %#v run %#v", terminalAssociation, runAssociation)
	}
}

type failingLinkedCommitRegistry struct {
	*agentrun.Registry
	err error
}

func (r *failingLinkedCommitRegistry) CommitLinkedAssociationChange(
	agentrun.LinkedAssociationChange,
) error {
	return r.err
}

func TestMutateTerminalAssociationRollsBackTerminalWhenRunCommitFails(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	launched, err := fixture.app.LaunchLinkedAgentV2(
		1, "agent-beta", "", 24, 80, fixture.taskPointer(),
	)
	if err != nil {
		t.Fatal(err)
	}
	beforeTerminal := *fixture.manager.association
	beforeRun := fixture.registry.Snapshot(1)[0]
	fixture.app.workspace.agents = &failingLinkedCommitRegistry{
		Registry: fixture.registry,
		err:      errors.New("injected run commit failure"),
	}
	_, err = fixture.app.MutateTerminalAssociationV2(
		1,
		launched.SessionID,
		1,
		false,
		association.PointerV1{Version: association.VersionV1, PlanID: fixture.planID},
	)
	if err == nil {
		t.Fatal("paired mutation succeeded despite injected run failure")
	}
	if fixture.manager.association == nil || *fixture.manager.association != beforeTerminal ||
		!reflect.DeepEqual(fixture.registry.Snapshot(1)[0], beforeRun) {
		t.Fatalf("failed pair became visible: terminal %#v run %#v",
			fixture.manager.association, fixture.registry.Snapshot(1)[0])
	}
}

func TestMutateTerminalAssociationRejectsCorrespondenceMismatchAndLegacyBypass(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	launched, err := fixture.app.LaunchLinkedAgentV2(
		1, "agent-beta", "", 24, 80, fixture.taskPointer(),
	)
	if err != nil {
		t.Fatal(err)
	}
	run := fixture.registry.Snapshot(1)[0]
	if _, err := fixture.app.AssociateTerminalV2(
		1,
		launched.SessionID,
		association.PointerV1{Version: association.VersionV1, PlanID: fixture.planID},
	); err == nil {
		t.Fatal("legacy terminal association endpoint changed a linked pair")
	}
	if _, err := fixture.app.AssociateAgentRunV2(
		1,
		run.ID,
		association.PointerV1{Version: association.VersionV1, PlanID: fixture.planID},
	); err == nil {
		t.Fatal("legacy AgentRun association endpoint changed a linked pair")
	}

	beforeTerminal := *fixture.manager.association
	beforeRun := fixture.registry.Snapshot(1)[0]
	fixture.app.workspace.agents = &mismatchedLinkedPrepareRegistry{
		Registry: fixture.registry,
	}
	_, err = fixture.app.MutateTerminalAssociationV2(
		1,
		launched.SessionID,
		1,
		false,
		association.PointerV1{Version: association.VersionV1, PlanID: fixture.planID},
	)
	if !errors.Is(err, agentrun.ErrAssociationMismatch) {
		t.Fatalf("correspondence mismatch = %v", err)
	}
	if fixture.manager.association == nil || *fixture.manager.association != beforeTerminal ||
		!reflect.DeepEqual(fixture.registry.Snapshot(1)[0], beforeRun) {
		t.Fatal("correspondence mismatch partially changed the pair")
	}
}

type mismatchedLinkedPrepareRegistry struct {
	*agentrun.Registry
}

func (r *mismatchedLinkedPrepareRegistry) PrepareLinkedTerminalAssociationChange(
	string,
	*association.AssociationV1,
	association.AssociationV1,
	*association.Host,
	association.PointerV1,
) (agentrun.LinkedAssociationChange, bool, error) {
	return agentrun.LinkedAssociationChange{}, false, agentrun.ErrAssociationMismatch
}

func TestMutateTerminalAssociationPublishesOneGenerationScopedRuntimeEvent(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	launched, err := fixture.app.LaunchLinkedAgentV2(
		1, "agent-beta", "", 24, 80, fixture.taskPointer(),
	)
	if err != nil {
		t.Fatal(err)
	}
	var mu sync.Mutex
	events := []emittedTerminalEvent{}
	fixture.app.lifecycleMu.Lock()
	fixture.app.wailsContext = context.Background()
	fixture.app.emitTerminal = func(ctx context.Context, name string, payload any) {
		mu.Lock()
		defer mu.Unlock()
		events = append(events, emittedTerminalEvent{ctx: ctx, name: name, payload: payload})
	}
	fixture.app.lifecycleMu.Unlock()
	if _, err := fixture.app.MutateTerminalAssociationV2(
		1,
		launched.SessionID,
		1,
		false,
		association.PointerV1{Version: association.VersionV1, PlanID: fixture.planID},
	); err != nil {
		t.Fatal(err)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(events) != 1 || events[0].name != workspaceRuntimeChangedEvent ||
		events[0].payload != uint64(1) {
		t.Fatalf("runtime events = %#v", events)
	}
}
