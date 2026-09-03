use std::cell::Cell;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::rc::Rc;

use ptrack_app::{
    ActivityState, ActorIdentity, AgentHandoffInbox, AgentRunObservationV1, AgentRunsV2,
    AgentRuntimeSummary, AppError, AppResult, ApplicationPort, BoundedSnapshot,
    CapabilityCancellation, CapabilityMcpOutcome, GuideAction, HookAction, HookResult, InitRequest,
    InitResult, LeaseState, Mutation, MutationResult, PlanLifecycleOutcome, PlanLifecycleRequest,
    ProcessOutput, ProcessState, RegistrationKind, RunState,
};
use ptrack_core::{
    Meta, Plan, PlanStatus, ProjectRef, ProjectSnapshot, Task, TaskStatus, Timestamp,
};

use crate::model::{Effect, Success};
use crate::runtime::{TerminalMode, apply_effect};
use crate::{Model, RuntimeContext, Tab};

struct FakeApplication {
    snapshot: ProjectSnapshot,
    mutation_fails: bool,
    snapshot_fails: bool,
    agent_runs: Option<AgentRunsV2>,
    agent_inbox: Option<AgentHandoffInbox>,
    agent_detail: Option<AgentRunObservationV1>,
}

impl ApplicationPort for FakeApplication {
    fn initialize(&mut self, _request: InitRequest) -> AppResult<InitResult> {
        unreachable!()
    }

    fn snapshot(&mut self) -> AppResult<ProjectSnapshot> {
        if self.snapshot_fails {
            Err(AppError::Message("reload failed".to_owned()))
        } else {
            Ok(self.snapshot.clone())
        }
    }

    fn mutate(&mut self, _mutation: Mutation) -> AppResult<MutationResult> {
        if self.mutation_fails {
            Err(AppError::Message("mutation failed".to_owned()))
        } else {
            Ok(MutationResult::None)
        }
    }

    fn plan_lifecycle(
        &mut self,
        _request: PlanLifecycleRequest,
    ) -> AppResult<PlanLifecycleOutcome> {
        Err(AppError::NotImplemented("test plan lifecycle"))
    }

    fn projects(&mut self) -> AppResult<Vec<ProjectRef>> {
        unreachable!()
    }

    fn identity(&mut self) -> AppResult<Option<ActorIdentity>> {
        unreachable!()
    }

    fn set_identity(&mut self, _name: &str) -> AppResult<ActorIdentity> {
        unreachable!()
    }

    fn backup(&mut self) -> AppResult<PathBuf> {
        unreachable!()
    }

    fn guide(&mut self, _action: GuideAction) -> AppResult<(String, Vec<PathBuf>)> {
        unreachable!()
    }

    fn hook(&mut self, _action: HookAction) -> AppResult<HookResult> {
        unreachable!()
    }

    fn git_show(&mut self, _reference: &str, _stat: bool) -> AppResult<ProcessOutput> {
        unreachable!()
    }

    fn capability_call(&mut self, _tool: &str, _arguments: &str) -> AppResult<Vec<u8>> {
        unreachable!()
    }

    fn capability_mcp(
        &mut self,
        _input: Box<dyn Read + Send>,
        _output: &mut dyn Write,
        _cancellation: &CapabilityCancellation,
    ) -> AppResult<CapabilityMcpOutcome> {
        unreachable!()
    }

    fn agent_runs(&mut self) -> AppResult<AgentRunsV2> {
        self.agent_runs
            .clone()
            .ok_or_else(|| AppError::Message("no active agent coordination host".to_owned()))
    }

    fn agent_run(&mut self, _run_id: &str) -> AppResult<AgentRunObservationV1> {
        self.agent_detail
            .clone()
            .ok_or_else(|| AppError::Message("AgentRun not found".to_owned()))
    }

    fn agent_inbox(&mut self) -> AppResult<AgentHandoffInbox> {
        self.agent_inbox
            .clone()
            .ok_or_else(|| AppError::Message("no active agent coordination host".to_owned()))
    }
}

fn snapshot(status: TaskStatus) -> ProjectSnapshot {
    ProjectSnapshot::new(
        Meta {
            goal: String::new(),
            summary: String::new(),
            active_plan: 1,
            created_at: Timestamp::Zero,
            updated_at: Timestamp::Zero,
            format_version: 4,
            last_write_version: "test".to_owned(),
            active_plans: Vec::new(),
            actors: Vec::new(),
        },
        vec![],
        vec![Plan {
            id: 1,
            title: "Plan".to_owned(),
            status: PlanStatus::Active,
            milestone_id: 0,
            order: 1,
            created_at: Timestamp::Zero,
            updated_at: Timestamp::Zero,
            hold_reason: None,
            actor: None,
            claim_conflict: false,
            claim_epoch: 0,
            claim_owner: None,
            ulid: None,
            deps: Vec::new(),
        }],
        vec![Task {
            id: 2,
            plan_id: 1,
            title: "Card".to_owned(),
            status,
            order: 1,
            created_at: Timestamp::Zero,
            updated_at: Timestamp::Zero,
            hold_reason: None,
            actor: None,
            ulid: None,
            deps: Vec::new(),
        }],
        vec![],
        vec![],
        vec![],
    )
}

fn model() -> Model {
    let mut model = Model::new(
        snapshot(TaskStatus::Todo),
        RuntimeContext {
            project_root: PathBuf::from("/project"),
            database: PathBuf::from("/project/.ptrack/ptrack.redb"),
            global_home: PathBuf::from("/home"),
        },
    );
    model.welcome = false;
    model.tab = Tab::Board;
    model
}

fn move_effect() -> Effect {
    Effect::Mutate {
        mutation: Mutation::SetTaskStatus {
            id: 2,
            status: TaskStatus::Doing,
        },
        success: Success::MovedCard {
            message: "moved #2 → doing".to_owned(),
            column: 1,
        },
    }
}

#[test]
fn partial_terminal_setup_failure_restores_every_owned_mode() {
    let state = Rc::new(Cell::new((false, false, false)));
    let setup_state = Rc::clone(&state);
    let cleanup_state = Rc::clone(&state);

    let result = TerminalMode::enter_with(
        || Ok(()),
        move || {
            // Simulate EnterAlternateScreen succeeding before a later command
            // in the combined setup write fails.
            setup_state.set((true, true, false));
            Err(std::io::Error::other("partial setup failure"))
        },
        Box::new(move |raw, alternate| {
            assert!(raw);
            assert!(alternate);
            cleanup_state.set((false, false, true));
        }),
    );

    assert!(result.is_err());
    assert_eq!(state.get(), (false, false, true));
}

#[test]
fn board_column_commits_only_after_mutation_and_snapshot_both_succeed() {
    let mut value = model();
    let mut mutation_failure = FakeApplication {
        snapshot: snapshot(TaskStatus::Doing),
        mutation_fails: true,
        snapshot_fails: false,
        agent_runs: None,
        agent_inbox: None,
        agent_detail: None,
    };
    assert!(!apply_effect(
        &mut mutation_failure,
        &mut value,
        move_effect()
    ));
    assert_eq!(value.board_col, 0);

    let mut value = model();
    let mut reload_failure = FakeApplication {
        snapshot: snapshot(TaskStatus::Doing),
        mutation_fails: false,
        snapshot_fails: true,
        agent_runs: None,
        agent_inbox: None,
        agent_detail: None,
    };
    assert!(!apply_effect(
        &mut reload_failure,
        &mut value,
        move_effect()
    ));
    assert_eq!(value.board_col, 0);

    let mut value = model();
    let mut success = FakeApplication {
        snapshot: snapshot(TaskStatus::Doing),
        mutation_fails: false,
        snapshot_fails: false,
        agent_runs: None,
        agent_inbox: None,
        agent_detail: None,
    };
    assert!(!apply_effect(&mut success, &mut value, move_effect()));
    assert_eq!(value.board_col, 1);
    assert_eq!(value.board_task().map(|task| task.id), Some(2));
}

#[test]
fn reload_refreshes_agent_data_and_preserves_or_clamps_selected_identity() {
    let mut value = model();
    value.tab = Tab::Agents;
    value.replace_agent_state(
        Some(agent_runs(&["run-one", "run-two"])),
        Some(empty_inbox()),
        String::new(),
    );
    value.agent_cursor = 1;
    let mut application = FakeApplication {
        snapshot: snapshot(TaskStatus::Todo),
        mutation_fails: false,
        snapshot_fails: false,
        agent_runs: Some(agent_runs(&["run-two"])),
        agent_inbox: Some(empty_inbox()),
        agent_detail: None,
    };
    assert!(!apply_effect(
        &mut application,
        &mut value,
        Effect::Reload {
            success: "reloaded".to_owned(),
            reopen_detail: false,
        }
    ));
    assert_eq!(value.agent_cursor, 0);
    assert_eq!(value.selected_agent_run_id(), Some("run-two"));

    application.agent_runs = Some(agent_runs(&[]));
    assert!(!apply_effect(
        &mut application,
        &mut value,
        Effect::Reload {
            success: "reloaded".to_owned(),
            reopen_detail: false,
        }
    ));
    assert_eq!(value.agent_cursor, 0);
    assert_eq!(value.selected_agent_run_id(), None);
}

fn agent_runs(ids: &[&str]) -> AgentRunsV2 {
    AgentRunsV2 {
        generation: 7,
        runs: ids.iter().map(|id| agent_row(id)).collect(),
        bounds: BoundedSnapshot::new(ids.len(), ids.len()),
    }
}

fn agent_row(id: &str) -> AgentRuntimeSummary {
    AgentRuntimeSummary {
        run_id: id.to_owned(),
        registration_kind: RegistrationKind::External,
        terminal_id: String::new(),
        terminal_backed: false,
        terminal_present: false,
        corresponding_terminal: false,
        state: RunState::Running,
        process_state: ProcessState::Unknown,
        lease_state: LeaseState::Active,
        live: true,
        activity_state: ActivityState::Running,
        association: None,
        intelligence: None,
    }
}

fn empty_inbox() -> AgentHandoffInbox {
    AgentHandoffInbox {
        items: Vec::new(),
        bounds: BoundedSnapshot::new(0, 0),
        incomplete: false,
    }
}
