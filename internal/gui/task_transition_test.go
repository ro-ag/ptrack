package gui

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/agentrun"
	"github.com/ro-ag/ptrack/internal/association"
	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/store"
	"github.com/ro-ag/ptrack/internal/terminal"
)

func launchTaskLinkedAgent(t *testing.T, fixture linkedLaunchFixture) TerminalSessionV2 {
	t.Helper()
	result, err := fixture.app.LaunchLinkedAgentV2(
		1, "agent-beta", "", 24, 80, fixture.taskPointer(),
	)
	if err != nil {
		t.Fatalf("LaunchLinkedAgentV2: %v", err)
	}
	return result
}

func taskStatus(t *testing.T, fixture linkedLaunchFixture) model.TaskStatus {
	t.Helper()
	s, err := store.Open(fixture.app.workspace.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	task, err := s.GetTask(fixture.taskID)
	if err != nil {
		t.Fatal(err)
	}
	return task.Status
}

func requireTransitionChallenge(
	t *testing.T,
	fixture linkedLaunchFixture,
	status model.TaskStatus,
) TaskTransitionResultV3 {
	t.Helper()
	result, err := fixture.app.MoveTaskV3(1, fixture.taskID, string(status), "")
	if err != nil {
		t.Fatalf("MoveTaskV3 challenge: %v", err)
	}
	if result.Applied || !result.RequiresConfirmation || result.Confirmation == nil ||
		result.Confirmation.Token == "" {
		t.Fatalf("transition challenge = %#v", result)
	}
	return result
}

func TestTaskTransitionAppliesImmediatelyWithoutActiveTaskResources(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	result, err := fixture.app.MoveTaskV3(
		1, fixture.taskID, string(model.TaskDoing), "",
	)
	if err != nil {
		t.Fatal(err)
	}
	if !result.Applied || result.RequiresConfirmation || result.Confirmation != nil ||
		result.FromStatus != string(model.TaskTodo) || result.ToStatus != string(model.TaskDoing) {
		t.Fatalf("immediate transition = %#v", result)
	}
	if got := taskStatus(t, fixture); got != model.TaskDoing {
		t.Fatalf("task status = %q", got)
	}
}

func startPausedTaskLinkedAdmission(
	t *testing.T,
	fixture linkedLaunchFixture,
) (<-chan error, chan struct{}) {
	t.Helper()
	started := make(chan struct{})
	release := make(chan struct{})
	fixture.manager.createStarted = started
	fixture.manager.createRelease = release
	done := make(chan error, 1)
	go func() {
		_, err := fixture.app.LaunchLinkedAgentV2(
			1, "agent-beta", "", 24, 80, fixture.taskPointer(),
		)
		done <- err
	}()
	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("linked resource admission did not reach CreateWithEnv")
	}
	return done, release
}

func TestTaskTransitionRejectsWhilePreFenceResourceAdmissionIsPending(t *testing.T) {
	t.Run("zero-resource begin", func(t *testing.T) {
		fixture := newLinkedLaunchFixture(t)
		done, release := startPausedTaskLinkedAdmission(t, fixture)
		result, err := fixture.app.MoveTaskV3(
			1, fixture.taskID, string(model.TaskDoing), "",
		)
		if !errors.Is(err, ErrTaskTransitionAdmissionPending) || result.Applied {
			t.Fatalf("transition during admission = %#v err %v", result, err)
		}
		close(release)
		if err := <-done; err != nil {
			t.Fatalf("paused linked admission: %v", err)
		}
		if got := taskStatus(t, fixture); got != model.TaskTodo {
			t.Fatalf("pending admission transition mutated task to %q", got)
		}
	})

	t.Run("challenge confirmation", func(t *testing.T) {
		fixture := newLinkedLaunchFixture(t)
		launchTaskLinkedAgent(t, fixture)
		challenge := requireTransitionChallenge(t, fixture, model.TaskDone)
		done, release := startPausedTaskLinkedAdmission(t, fixture)
		for attempt := 0; attempt < 2; attempt++ {
			result, err := fixture.app.MoveTaskV3(
				1,
				fixture.taskID,
				string(model.TaskDone),
				challenge.Confirmation.Token,
			)
			if !errors.Is(err, ErrTaskTransitionAdmissionPending) || result.Applied {
				t.Fatalf("confirmation during admission = %#v err %v", result, err)
			}
		}
		close(release)
		// The fake returns the already-live session identity for this second
		// launch, so the admission may fail closed after it is released. Its
		// outcome is irrelevant to the transition fencing assertion.
		<-done
		if got := taskStatus(t, fixture); got != model.TaskTodo {
			t.Fatalf("pending admission confirmation mutated task to %q", got)
		}
	})
}

func TestTaskTransitionChallengeIsContentFreeSingleUseAndNonInterfering(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	broker := &fakeWorkspaceCapabilityBroker{token: "capability-secret-canary"}
	fixture.app.workspace.capabilities = broker
	launched := launchTaskLinkedAgent(t, fixture)
	beforeRun := fixture.registry.Snapshot(1)[0]
	beforeCreate := fixture.manager.lastCreate()
	beforeIssued := append([]string(nil), broker.issuedProfiles...)

	challenge := requireTransitionChallenge(t, fixture, model.TaskDone)
	if challenge.Confirmation.ActiveTerminals != 1 ||
		challenge.Confirmation.ActiveAgents != 1 {
		t.Fatalf("active counts = %#v", challenge.Confirmation)
	}
	if got := taskStatus(t, fixture); got != model.TaskTodo {
		t.Fatalf("first request mutated task to %q", got)
	}
	encoded, err := json.Marshal(challenge)
	if err != nil {
		t.Fatal(err)
	}
	for _, canary := range []string{
		launched.SessionID,
		beforeRun.ID,
		"capability-secret-canary",
		"ws://127.0.0.1/linked-session?token=opaque",
	} {
		if strings.Contains(string(encoded), canary) {
			t.Fatalf("challenge exposed runtime data %q: %s", canary, encoded)
		}
	}

	confirmed, err := fixture.app.MoveTaskV3(
		1, fixture.taskID, string(model.TaskDone), challenge.Confirmation.Token,
	)
	if err != nil {
		t.Fatal(err)
	}
	if !confirmed.Applied || confirmed.RequiresConfirmation {
		t.Fatalf("confirmed transition = %#v", confirmed)
	}
	if got := taskStatus(t, fixture); got != model.TaskDone {
		t.Fatalf("confirmed task status = %q", got)
	}
	if _, err := fixture.app.MoveTaskV3(
		1, fixture.taskID, string(model.TaskDone), challenge.Confirmation.Token,
	); !errors.Is(err, ErrTaskTransitionConfirmationInvalid) {
		t.Fatalf("reused challenge = %v", err)
	}

	afterRun := fixture.registry.Snapshot(1)[0]
	if !reflect.DeepEqual(beforeCreate, fixture.manager.lastCreate()) ||
		len(fixture.manager.creates) != 1 || len(fixture.manager.closes) != 0 ||
		afterRun.ID != beforeRun.ID || afterRun.TerminalID != beforeRun.TerminalID ||
		afterRun.PID != beforeRun.PID || afterRun.State != beforeRun.State ||
		afterRun.ProcessState != beforeRun.ProcessState ||
		!reflect.DeepEqual(broker.issuedProfiles, beforeIssued) ||
		len(broker.revokedTokens) != 0 || len(broker.revokedSessions) != 0 {
		t.Fatalf("transition changed runtime/capability identity: run %#v broker %#v", afterRun, broker)
	}
}

func TestLegacyMoveCannotBypassActiveResourceConfirmation(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	launchTaskLinkedAgent(t, fixture)
	if _, err := fixture.app.MoveTaskV2(
		1, fixture.taskID, string(model.TaskDone),
	); !errors.Is(err, ErrTaskTransitionConfirmationRequired) {
		t.Fatalf("MoveTaskV2 active resources = %v", err)
	}
	if got := taskStatus(t, fixture); got != model.TaskTodo {
		t.Fatalf("legacy move mutated task to %q", got)
	}
}

func TestTaskTransitionConfirmationInvalidatesOnBoundStateChanges(t *testing.T) {
	tests := []struct {
		name   string
		mutate func(*testing.T, linkedLaunchFixture, TerminalSessionV2)
		to     model.TaskStatus
	}{
		{
			name: "task status",
			mutate: func(t *testing.T, fixture linkedLaunchFixture, _ TerminalSessionV2) {
				s, err := store.Open(fixture.app.workspace.dbPath)
				if err != nil {
					t.Fatal(err)
				}
				defer s.Close()
				if err := s.SetTaskStatus(fixture.taskID, model.TaskBlocked); err != nil {
					t.Fatal(err)
				}
			},
			to: model.TaskDone,
		},
		{
			name: "task plan",
			mutate: func(t *testing.T, fixture linkedLaunchFixture, _ TerminalSessionV2) {
				s, err := store.Open(fixture.app.workspace.dbPath)
				if err != nil {
					t.Fatal(err)
				}
				defer s.Close()
				plan, err := s.AddPlan("new owner")
				if err != nil {
					t.Fatal(err)
				}
				if err := s.SetTaskPlan(fixture.taskID, plan.ID); err != nil {
					t.Fatal(err)
				}
			},
			to: model.TaskDone,
		},
		{
			name: "association revision",
			mutate: func(t *testing.T, fixture linkedLaunchFixture, launch TerminalSessionV2) {
				_, err := fixture.app.MutateTerminalAssociationV2(
					1, launch.SessionID, launch.AssociationRevision, false,
					association.PointerV1{Version: association.VersionV1, PlanID: fixture.planID},
				)
				if err != nil {
					t.Fatal(err)
				}
			},
			to: model.TaskDone,
		},
		{
			name: "terminal and run exit",
			mutate: func(t *testing.T, fixture linkedLaunchFixture, launch TerminalSessionV2) {
				fixture.manager.mu.Lock()
				fixture.manager.createResult.State = terminal.SessionExited
				fixture.manager.mu.Unlock()
				if !fixture.registry.RecordTerminalExit(launch.SessionID, 0, "result-canary") {
					t.Fatal("linked AgentRun was not marked exited")
				}
			},
			to: model.TaskDone,
		},
		{
			name: "terminal close",
			mutate: func(t *testing.T, fixture linkedLaunchFixture, launch TerminalSessionV2) {
				if err := fixture.manager.Close(launch.SessionID, false); err != nil {
					t.Fatal(err)
				}
				fixture.registry.RecordTerminalExit(launch.SessionID, 0, "closed")
			},
			to: model.TaskDone,
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			fixture := newLinkedLaunchFixture(t)
			launch := launchTaskLinkedAgent(t, fixture)
			challenge := requireTransitionChallenge(t, fixture, test.to)
			test.mutate(t, fixture, launch)
			beforeConfirm := taskStatus(t, fixture)
			if _, err := fixture.app.MoveTaskV3(
				1, fixture.taskID, string(test.to), challenge.Confirmation.Token,
			); err == nil {
				t.Fatal("stale challenge was accepted")
			}
			if got := taskStatus(t, fixture); got != beforeConfirm {
				t.Fatalf("rejected confirmation mutated status from %q to %q", beforeConfirm, got)
			}
		})
	}
}

func TestTaskTransitionConfirmationInvalidatesWhenTaskIsConverted(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	launchTaskLinkedAgent(t, fixture)
	challenge := requireTransitionChallenge(t, fixture, model.TaskDone)
	s, err := store.Open(fixture.app.workspace.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.ConvertTaskToPlan(fixture.taskID); err != nil {
		s.Close()
		t.Fatal(err)
	}
	s.Close()
	if _, err := fixture.app.MoveTaskV3(
		1, fixture.taskID, string(model.TaskDone), challenge.Confirmation.Token,
	); err == nil {
		t.Fatal("challenge survived task conversion")
	}
	s, err = store.Open(fixture.app.workspace.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	if _, err := s.GetTask(fixture.taskID); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("converted task unexpectedly restored: %v", err)
	}
}

func TestTaskTransitionConfirmationRejectsTaskStatusAndPlanABA(t *testing.T) {
	t.Run("status", func(t *testing.T) {
		fixture := newLinkedLaunchFixture(t)
		launchTaskLinkedAgent(t, fixture)
		challenge := requireTransitionChallenge(t, fixture, model.TaskDone)
		s, err := store.Open(fixture.app.workspace.dbPath)
		if err != nil {
			t.Fatal(err)
		}
		if err := s.SetTaskStatus(fixture.taskID, model.TaskDoing); err != nil {
			s.Close()
			t.Fatal(err)
		}
		if err := s.SetTaskStatus(fixture.taskID, model.TaskTodo); err != nil {
			s.Close()
			t.Fatal(err)
		}
		s.Close()
		if _, err := fixture.app.MoveTaskV3(
			1, fixture.taskID, string(model.TaskDone), challenge.Confirmation.Token,
		); !errors.Is(err, store.ErrTaskStatusChanged) {
			t.Fatalf("status ABA confirmation = %v", err)
		}
		if got := taskStatus(t, fixture); got != model.TaskTodo {
			t.Fatalf("status ABA mutated task to %q", got)
		}
	})
	t.Run("plan", func(t *testing.T) {
		fixture := newLinkedLaunchFixture(t)
		launchTaskLinkedAgent(t, fixture)
		challenge := requireTransitionChallenge(t, fixture, model.TaskDone)
		s, err := store.Open(fixture.app.workspace.dbPath)
		if err != nil {
			t.Fatal(err)
		}
		other, err := s.AddPlan("temporary owner")
		if err != nil {
			s.Close()
			t.Fatal(err)
		}
		if err := s.SetTaskPlan(fixture.taskID, other.ID); err != nil {
			s.Close()
			t.Fatal(err)
		}
		if err := s.SetTaskPlan(fixture.taskID, fixture.planID); err != nil {
			s.Close()
			t.Fatal(err)
		}
		s.Close()
		if _, err := fixture.app.MoveTaskV3(
			1, fixture.taskID, string(model.TaskDone), challenge.Confirmation.Token,
		); !errors.Is(err, store.ErrTaskStatusChanged) {
			t.Fatalf("plan ABA confirmation = %v", err)
		}
		if got := taskStatus(t, fixture); got != model.TaskTodo {
			t.Fatalf("plan ABA mutated task to %q", got)
		}
	})
}

type expandingExactTerminalManager struct {
	terminalManager
	exact terminalExactSnapshotManager
	mu    sync.Mutex
	calls int
	added terminal.SessionInfo
}

func (m *expandingExactTerminalManager) WithExactSessionSnapshot(
	maximum int,
	use func([]terminal.SessionInfo) error,
) error {
	m.mu.Lock()
	m.calls++
	expand := m.calls > 1
	m.mu.Unlock()
	return m.exact.WithExactSessionSnapshot(maximum, func(sessions []terminal.SessionInfo) error {
		if expand {
			sessions = append(sessions, m.added)
		}
		if len(sessions) > maximum {
			return terminal.ErrSnapshotLimit
		}
		return use(sessions)
	})
}

func TestTaskTransitionConfirmationInvalidatesWhenSessionStarts(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	launchTaskLinkedAgent(t, fixture)
	fixture.manager.mu.Lock()
	addedAssociation := *fixture.manager.association
	fixture.manager.mu.Unlock()
	addedAssociation.LiveID = "new-live-session"
	wrapper := &expandingExactTerminalManager{
		terminalManager: fixture.manager,
		exact:           fixture.manager,
		added: terminal.SessionInfo{
			ID: "new-live-session", State: terminal.SessionRunning,
			Association: &addedAssociation,
		},
	}
	fixture.app.workspace.terminals = wrapper
	challenge := requireTransitionChallenge(t, fixture, model.TaskDone)
	if challenge.Confirmation.ActiveTerminals != 1 {
		t.Fatalf("initial active terminals = %d", challenge.Confirmation.ActiveTerminals)
	}
	if _, err := fixture.app.MoveTaskV3(
		1, fixture.taskID, string(model.TaskDone), challenge.Confirmation.Token,
	); !errors.Is(err, ErrTaskTransitionConfirmationInvalid) {
		t.Fatalf("session-start confirmation = %v", err)
	}
	if got := taskStatus(t, fixture); got != model.TaskTodo {
		t.Fatalf("session-start invalidation mutated task to %q", got)
	}
}

func TestTaskTransitionChallengeRejectsWrongGenerationTaskActionAndExpiry(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	launchTaskLinkedAgent(t, fixture)

	wrongGeneration := requireTransitionChallenge(t, fixture, model.TaskDone)
	if _, err := fixture.app.MoveTaskV3(
		2, fixture.taskID, string(model.TaskDone), wrongGeneration.Confirmation.Token,
	); err == nil {
		t.Fatal("wrong generation challenge was accepted")
	}
	if got := taskStatus(t, fixture); got != model.TaskTodo {
		t.Fatalf("wrong generation mutated task to %q", got)
	}

	wrongTask := requireTransitionChallenge(t, fixture, model.TaskDone)
	if _, err := fixture.app.MoveTaskV3(
		1, fixture.taskID+999, string(model.TaskDone), wrongTask.Confirmation.Token,
	); !errors.Is(err, ErrTaskTransitionConfirmationInvalid) {
		t.Fatalf("wrong task challenge = %v", err)
	}

	wrongAction := requireTransitionChallenge(t, fixture, model.TaskDone)
	if _, err := fixture.app.MoveTaskV3(
		1, fixture.taskID, string(model.TaskDoing), wrongAction.Confirmation.Token,
	); !errors.Is(err, ErrTaskTransitionConfirmationInvalid) {
		t.Fatalf("wrong action challenge = %v", err)
	}

	now := time.Date(2026, 8, 10, 12, 0, 0, 0, time.UTC)
	fixture.app.workspace.taskTransitions = newTaskTransitionChallengeRegistry(
		func() time.Time { return now },
	)
	expired := requireTransitionChallenge(t, fixture, model.TaskDone)
	now = now.Add(taskTransitionConfirmationTTL + time.Nanosecond)
	if _, err := fixture.app.MoveTaskV3(
		1, fixture.taskID, string(model.TaskDone), expired.Confirmation.Token,
	); !errors.Is(err, ErrTaskTransitionConfirmationInvalid) {
		t.Fatalf("expired challenge = %v", err)
	}
	if got := taskStatus(t, fixture); got != model.TaskTodo {
		t.Fatalf("invalid challenges mutated task to %q", got)
	}
}

func TestTaskTransitionExitedAndPlanOnlyAssociationsAreNotBlockers(t *testing.T) {
	t.Run("exited", func(t *testing.T) {
		fixture := newLinkedLaunchFixture(t)
		launch := launchTaskLinkedAgent(t, fixture)
		fixture.manager.mu.Lock()
		fixture.manager.createResult.State = terminal.SessionExited
		fixture.manager.mu.Unlock()
		fixture.registry.RecordTerminalExit(launch.SessionID, 0, "historical")
		result, err := fixture.app.MoveTaskV3(
			1, fixture.taskID, string(model.TaskDone), "",
		)
		if err != nil || !result.Applied || result.RequiresConfirmation {
			t.Fatalf("exited resources transition = %#v err %v", result, err)
		}
	})
	t.Run("plan only", func(t *testing.T) {
		fixture := newLinkedLaunchFixture(t)
		launch := launchTaskLinkedAgent(t, fixture)
		if _, err := fixture.app.MutateTerminalAssociationV2(
			1, launch.SessionID, launch.AssociationRevision, false,
			association.PointerV1{Version: association.VersionV1, PlanID: fixture.planID},
		); err != nil {
			t.Fatal(err)
		}
		result, err := fixture.app.MoveTaskV3(
			1, fixture.taskID, string(model.TaskDone), "",
		)
		if err != nil || !result.Applied || result.RequiresConfirmation {
			t.Fatalf("plan-only resources transition = %#v err %v", result, err)
		}
	})
}

func TestTaskTransitionOverdueExternalLeaseInvalidatesChallenge(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	now := time.Date(2026, 8, 10, 12, 0, 0, 0, time.UTC)
	registry := agentrun.NewRegistry(agentrun.Config{
		ProjectRoot:   fixture.root,
		LeaseDuration: 30 * time.Second,
		SweepInterval: time.Hour,
		Now:           func() time.Time { return now },
	})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	fixture.app.workspace.agents = registry
	s, err := store.Open(fixture.app.workspace.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	host, err := association.NewHost(
		fixture.root, 1, storeAssociationCatalog{store: s},
	)
	if err != nil {
		s.Close()
		t.Fatal(err)
	}
	lease, err := registry.RegisterExternal(agentrun.Registration{
		Profile: "external", Provider: "test", CWD: fixture.root,
	})
	if err == nil {
		_, err = registry.Associate(lease.Run.ID, host, fixture.taskPointer())
	}
	s.Close()
	if err != nil {
		t.Fatal(err)
	}
	challenge := requireTransitionChallenge(t, fixture, model.TaskDone)
	if challenge.Confirmation.ActiveAgents != 1 ||
		challenge.Confirmation.ActiveTerminals != 0 {
		t.Fatalf("external lease counts = %#v", challenge.Confirmation)
	}
	now = now.Add(31 * time.Second)
	if _, err := fixture.app.MoveTaskV3(
		1, fixture.taskID, string(model.TaskDone), challenge.Confirmation.Token,
	); !errors.Is(err, ErrTaskTransitionConfirmationInvalid) {
		t.Fatalf("expired lease confirmation = %v", err)
	}
	run := registry.Snapshot(1)[0]
	if run.State != agentrun.StateStale || run.LeaseState != agentrun.LeaseExpired {
		t.Fatalf("exact snapshot did not expire overdue lease: %#v", run)
	}
	result, err := fixture.app.MoveTaskV3(
		1, fixture.taskID, string(model.TaskDone), "",
	)
	if err != nil || !result.Applied {
		t.Fatalf("stale external run remained blocker: %#v err %v", result, err)
	}
}

func TestTaskTransitionExternalLeaseLifecycleABAInvalidatesChallenge(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	now := time.Date(2026, 8, 10, 12, 0, 0, 0, time.UTC)
	registry := agentrun.NewRegistry(agentrun.Config{
		ProjectRoot: fixture.root, LeaseDuration: 30 * time.Second,
		SweepInterval: time.Hour, Now: func() time.Time { return now },
	})
	t.Cleanup(func() { _ = registry.Shutdown(context.Background()) })
	fixture.app.workspace.agents = registry
	s, err := store.Open(fixture.app.workspace.dbPath)
	if err != nil {
		t.Fatal(err)
	}
	host, err := association.NewHost(
		fixture.root, 1, storeAssociationCatalog{store: s},
	)
	if err != nil {
		s.Close()
		t.Fatal(err)
	}
	lease, err := registry.RegisterExternal(agentrun.Registration{
		Profile: "external", Provider: "test", CWD: fixture.root,
	})
	if err == nil {
		_, err = registry.Associate(lease.Run.ID, host, fixture.taskPointer())
	}
	s.Close()
	if err != nil {
		t.Fatal(err)
	}
	challenge := requireTransitionChallenge(t, fixture, model.TaskDone)
	now = now.Add(31 * time.Second)
	registry.SweepExpired()
	if err := registry.Heartbeat(lease.Run.ID, lease.LeaseToken); err != nil {
		t.Fatal(err)
	}
	if run := registry.Snapshot(1)[0]; run.State != agentrun.StateRunning || run.LeaseState != agentrun.LeaseActive {
		t.Fatalf("lease did not return to its original live state: %#v", run)
	}
	if _, err := fixture.app.MoveTaskV3(
		1, fixture.taskID, string(model.TaskDone), challenge.Confirmation.Token,
	); !errors.Is(err, ErrTaskTransitionConfirmationInvalid) {
		t.Fatalf("lease lifecycle ABA confirmation = %v", err)
	}
	if got := taskStatus(t, fixture); got != model.TaskTodo {
		t.Fatalf("lease lifecycle ABA mutated task to %q", got)
	}
}

func TestTaskTransitionTerminalResourceABAInvalidatesChallenge(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	launchTaskLinkedAgent(t, fixture)
	challenge := requireTransitionChallenge(t, fixture, model.TaskDone)
	fixture.app.workspace.recordTerminal(TerminalSession{
		SessionID: "transient-linked-session", State: terminal.SessionRunning,
	})
	fixture.app.workspace.removeTerminal("transient-linked-session")
	if _, err := fixture.app.MoveTaskV3(
		1, fixture.taskID, string(model.TaskDone), challenge.Confirmation.Token,
	); !errors.Is(err, ErrTaskTransitionConfirmationInvalid) {
		t.Fatalf("terminal resource ABA confirmation = %v", err)
	}
	if got := taskStatus(t, fixture); got != model.TaskTodo {
		t.Fatalf("terminal resource ABA mutated task to %q", got)
	}
}

type overflowingAgentRegistry struct {
	workspaceAgentRegistry
}

func (r *overflowingAgentRegistry) WithExactRuntimeSnapshot(
	int,
	func([]agentrun.Run) error,
) error {
	return agentrun.ErrSnapshotLimit
}

func TestTaskTransitionFailsClosedWhenExactResourceLimitIsExceeded(t *testing.T) {
	fixture := newLinkedLaunchFixture(t)
	fixture.app.workspace.agents = &overflowingAgentRegistry{
		workspaceAgentRegistry: fixture.registry,
	}
	result, err := fixture.app.MoveTaskV3(
		1, fixture.taskID, string(model.TaskDone), "",
	)
	if !errors.Is(err, agentrun.ErrSnapshotLimit) || result.Applied {
		t.Fatalf("overflow transition = %#v err %v", result, err)
	}
	if got := taskStatus(t, fixture); got != model.TaskTodo {
		t.Fatalf("overflow mutated task to %q", got)
	}
}

func TestTaskTransitionChallengeRegistryIsBounded(t *testing.T) {
	now := time.Date(2026, 8, 10, 12, 0, 0, 0, time.UTC)
	registry := newTaskTransitionChallengeRegistry(func() time.Time { return now })
	for index := 0; index < taskTransitionConfirmationLimit+10; index++ {
		_, _, err := registry.issue(taskTransitionChallenge{
			Generation: 1, TaskID: uint64(index + 1), PlanID: 1,
			FromStatus: model.TaskTodo, ToStatus: model.TaskDone,
			ResourceDigest: [32]byte{byte(index)},
		})
		if err != nil {
			t.Fatal(err)
		}
		now = now.Add(time.Nanosecond)
	}
	registry.mu.Lock()
	defer registry.mu.Unlock()
	if len(registry.records) != taskTransitionConfirmationLimit {
		t.Fatalf("challenge records = %d, want %d", len(registry.records), taskTransitionConfirmationLimit)
	}
	for token := range registry.records {
		decoded, err := base64.RawURLEncoding.DecodeString(token)
		if err != nil || len(decoded) != 32 {
			t.Fatalf("challenge token is not opaque: %q", token)
		}
	}
}
