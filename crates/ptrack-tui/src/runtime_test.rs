use std::cell::Cell;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::rc::Rc;

use ptrack_app::{
    AppError, AppResult, ApplicationPort, CapabilityCancellation, CapabilityMcpOutcome,
    GuideAction, HookAction, HookResult, InitRequest, InitResult, Mutation, MutationResult,
    ProcessOutput,
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

    fn projects(&mut self) -> AppResult<Vec<ProjectRef>> {
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
    };
    assert!(!apply_effect(&mut success, &mut value, move_effect()));
    assert_eq!(value.board_col, 1);
    assert_eq!(value.board_task().map(|task| task.id), Some(2));
}
