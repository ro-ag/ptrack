use std::path::PathBuf;

use ptrack_app::{
    ActivityState, AgentHandoffInbox, AgentRunsV2, AgentRuntimeSummary, BoundedSnapshot,
    LeaseState, Mutation, ProcessState, RegistrationKind, RunState,
};
use ptrack_core::{
    Issue, IssueStatus, MemoryKind, Meta, Note, NoteTarget, Plan, PlanStatus, ProjectSnapshot,
    Severity, Task, TaskStatus, Timestamp,
};

use crate::model::{DetailTarget, PaneFocus, Success};
use crate::{Effect, Key, Model, RuntimeContext, Tab, update};

fn model() -> Model {
    Model::new(
        ProjectSnapshot::new(
            Meta {
                goal: "ship".to_owned(),
                summary: String::new(),
                active_plan: 0,
                created_at: Timestamp::Zero,
                updated_at: Timestamp::Zero,
                format_version: 4,
                last_write_version: "test".to_owned(),
                active_plans: Vec::new(),
                actors: Vec::new(),
                stack: None,
                scratchpad: None,
                summary_updated_at: None,
            },
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        ),
        RuntimeContext {
            project_root: PathBuf::from("/project"),
            database: PathBuf::from("/project/.ptrack/ptrack.redb"),
            global_home: PathBuf::from("/home"),
        },
    )
}

#[test]
fn modal_precedence_and_input_ctrl_c_match_source_behavior() {
    let mut value = model();
    update(&mut value, &Key::Enter);
    update(&mut value, &Key::Char('g'));
    assert_eq!(update(&mut value, &Key::Ctrl('c')), None);
    assert!(value.input.is_some());
    update(&mut value, &Key::Escape);
    assert_eq!(update(&mut value, &Key::Ctrl('c')), Some(Effect::Quit));
}

#[test]
fn six_tabs_and_menu_layering_are_stable() {
    let mut value = model();
    update(&mut value, &Key::Char('5'));
    assert_eq!(value.tab, Tab::Maintenance);
    update(&mut value, &Key::Char('?'));
    assert!(value.menu);
    update(&mut value, &Key::Char('2'));
    assert_eq!(value.tab, Tab::Board);
    assert!(!value.menu);

    update(&mut value, &Key::Char('6'));
    assert_eq!(value.tab, Tab::Agents);
    update(&mut value, &Key::BackTab);
    assert_eq!(value.tab, Tab::Maintenance);
    update(&mut value, &Key::Tab);
    assert_eq!(value.tab, Tab::Agents);
}

#[test]
fn agent_pane_row_and_detail_navigation_are_explicit() {
    let mut value = model();
    value.welcome = false;
    value.tab = Tab::Agents;
    value.replace_agent_state(
        Some(AgentRunsV2 {
            generation: 7,
            runs: vec![agent_row("run-one"), agent_row("run-two")],
            bounds: BoundedSnapshot::new(2, 2),
        }),
        Some(AgentHandoffInbox {
            items: Vec::new(),
            bounds: BoundedSnapshot::new(0, 0),
            incomplete: false,
        }),
        String::new(),
    );
    update(&mut value, &Key::Down);
    assert_eq!(value.agent_cursor, 1);
    assert_eq!(
        update(&mut value, &Key::Enter),
        Some(Effect::LoadAgentDetail("run-two".to_owned()))
    );
    update(&mut value, &Key::Right);
    assert_eq!(value.agent_pane, crate::model::AgentPane::Handoffs);
    update(&mut value, &Key::Left);
    assert_eq!(value.agent_pane, crate::model::AgentPane::Runs);
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

#[test]
fn overview_task_note_falls_back_to_the_current_plan() {
    let mut value = model();
    value.snapshot.plans.push(Plan {
        id: 8,
        title: "Current".to_owned(),
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
    });
    value.welcome = false;
    value.focus = PaneFocus::Tasks;

    update(&mut value, &Key::Char('n'));
    update(&mut value, &Key::Paste("durable note".to_owned()));
    let effect = update(&mut value, &Key::Enter);
    assert!(matches!(
        effect,
        Some(Effect::Mutate {
            mutation: Mutation::AddNote {
                target: NoteTarget::Plan,
                target_id: 8,
                ref body,
            },
            ..
        }) if body == "durable note"
    ));
}

#[test]
fn detail_scroll_is_bounded_to_the_last_full_viewport() {
    let mut value = model();
    value.snapshot.plans.push(Plan {
        id: 1,
        title: "A very long plan title that wraps on a narrow terminal".to_owned(),
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
    });
    value.welcome = false;
    value.detail = Some(DetailTarget::Plan(1));
    value.resize(24, 12);
    value.detail_offset = usize::MAX;

    update(&mut value, &Key::Down);
    let maximum = crate::render::detail_scroll_max(&value);
    assert_eq!(value.detail_offset, maximum);
    update(&mut value, &Key::PageDown);
    assert_eq!(value.detail_offset, maximum);
}

#[test]
fn board_column_change_is_deferred_until_the_mutation_and_reload_succeed() {
    let mut value = model();
    value.snapshot.plans.push(Plan {
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
    });
    value.snapshot.tasks.push(Task {
        id: 2,
        plan_id: 1,
        title: "Card".to_owned(),
        status: TaskStatus::Todo,
        order: 1,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
        actor: None,
        ulid: None,
        deps: Vec::new(),
    });
    value.welcome = false;
    value.tab = Tab::Board;

    let effect = update(&mut value, &Key::Char('L'));
    assert_eq!(value.board_col, 0);
    assert!(matches!(
        effect,
        Some(Effect::Mutate {
            mutation: Mutation::SetTaskStatus {
                id: 2,
                status: TaskStatus::Doing,
            },
            success: crate::model::Success::MovedCard { column: 1, .. },
        })
    ));
}

#[test]
fn done_moves_from_the_board_and_overview_go_through_the_recorded_close() {
    let mut value = model();
    value.snapshot.plans.push(Plan {
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
    });
    value.snapshot.tasks.push(Task {
        id: 2,
        plan_id: 1,
        title: "Card".to_owned(),
        status: TaskStatus::Blocked,
        order: 1,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
        actor: None,
        ulid: None,
        deps: Vec::new(),
    });
    value.welcome = false;
    value.tab = Tab::Board;
    value.board_col = 2;
    assert!(matches!(
        update(&mut value, &Key::Char('L')),
        Some(Effect::Close {
            target: crate::model::UiClose::Task(2),
            success: crate::model::Success::MovedCard { column: 3, .. },
        })
    ));

    value.tab = Tab::Overview;
    value.focus = PaneFocus::Tasks;
    assert!(matches!(
        update(&mut value, &Key::Char('d')),
        Some(Effect::Close {
            target: crate::model::UiClose::Task(2),
            ..
        })
    ));
    assert!(matches!(
        update(&mut value, &Key::Char('x')),
        Some(Effect::Close {
            target: crate::model::UiClose::Plan(1),
            ..
        })
    ));
}

fn plan(id: u64, hold_reason: Option<&str>) -> Plan {
    Plan {
        id,
        title: format!("Plan {id}"),
        status: PlanStatus::Active,
        milestone_id: 0,
        order: 1,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: hold_reason.map(str::to_owned),
        actor: None,
        claim_conflict: false,
        claim_epoch: 0,
        claim_owner: None,
        ulid: None,
        deps: Vec::new(),
    }
}

fn task(id: u64, plan_id: u64, title: &str, hold_reason: Option<&str>) -> Task {
    Task {
        id,
        plan_id,
        title: title.to_owned(),
        status: TaskStatus::Todo,
        order: 1,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: hold_reason.map(str::to_owned),
        actor: None,
        ulid: None,
        deps: Vec::new(),
    }
}

fn issue(id: u64, title: &str, severity: Severity) -> Issue {
    Issue {
        id,
        title: title.to_owned(),
        body: String::new(),
        status: IssueStatus::Open,
        severity,
        task_id: 0,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        actor: None,
        ulid: None,
    }
}

fn type_and_enter(value: &mut Model, text: &str) -> Option<Effect> {
    update(value, &Key::Paste(text.to_owned()));
    update(value, &Key::Enter)
}

#[test]
fn hold_asks_for_a_reason_then_holds_the_selected_card() {
    let mut value = model();
    value.snapshot.plans.push(plan(1, None));
    value.snapshot.tasks.push(task(2, 1, "Card", None));
    value.welcome = false;
    value.tab = Tab::Board;

    assert_eq!(update(&mut value, &Key::Char('w')), None);
    assert!(value.input.is_some());
    assert_eq!(
        type_and_enter(&mut value, "waiting on upstream"),
        Some(Effect::Mutate {
            mutation: Mutation::SetTaskHold {
                id: 2,
                reason: Some("waiting on upstream".to_owned()),
            },
            success: Success::Message("task #2 on hold".to_owned()),
        })
    );
}

#[test]
fn hold_on_a_held_task_resumes_it_without_a_prompt() {
    let mut value = model();
    value.snapshot.plans.push(plan(1, None));
    value
        .snapshot
        .tasks
        .push(task(2, 1, "Card", Some("blocked upstream")));
    value.welcome = false;
    value.focus = PaneFocus::Tasks;

    assert_eq!(
        update(&mut value, &Key::Char('w')),
        Some(Effect::Mutate {
            mutation: Mutation::SetTaskHold {
                id: 2,
                reason: None
            },
            success: Success::Message("task #2 resumed".to_owned()),
        })
    );
    assert!(value.input.is_none());
}

#[test]
fn hold_in_the_plans_pane_targets_the_plan() {
    let mut value = model();
    value.snapshot.plans.push(plan(1, None));
    value.welcome = false;

    update(&mut value, &Key::Char('w'));
    assert_eq!(
        type_and_enter(&mut value, "next quarter"),
        Some(Effect::Mutate {
            mutation: Mutation::SetPlanHold {
                id: 1,
                reason: Some("next quarter".to_owned()),
            },
            success: Success::Message("plan #1 on hold".to_owned()),
        })
    );

    value.snapshot.plans[0].hold_reason = Some("next quarter".to_owned());
    assert_eq!(
        update(&mut value, &Key::Char('w')),
        Some(Effect::Mutate {
            mutation: Mutation::SetPlanHold {
                id: 1,
                reason: None
            },
            success: Success::Message("plan #1 resumed".to_owned()),
        })
    );
}

#[test]
fn an_empty_hold_reason_cancels() {
    let mut value = model();
    value.snapshot.plans.push(plan(1, None));
    value.welcome = false;

    update(&mut value, &Key::Char('w'));
    assert_eq!(update(&mut value, &Key::Enter), None);
    assert_eq!(value.status, "cancelled");
}

#[test]
fn severity_cycles_low_medium_high_critical() {
    let mut value = model();
    value
        .snapshot
        .issues
        .push(issue(3, "Crash", Severity::High));
    value.welcome = false;
    value.tab = Tab::Issues;

    assert_eq!(
        update(&mut value, &Key::Char('v')),
        Some(Effect::Mutate {
            mutation: Mutation::SetIssueSeverity {
                id: 3,
                severity: Severity::Critical,
            },
            success: Success::Message("issue #3 severity critical".to_owned()),
        })
    );
    value.snapshot.issues[0].severity = Severity::Critical;
    assert!(matches!(
        update(&mut value, &Key::Char('v')),
        Some(Effect::Mutate {
            mutation: Mutation::SetIssueSeverity {
                severity: Severity::Low,
                ..
            },
            ..
        })
    ));
}

#[test]
fn schedule_defaults_to_the_active_plan_and_uses_the_issue_title() {
    let mut value = model();
    value.snapshot.meta.active_plan = 4;
    value.snapshot.plans.push(plan(4, None));
    value
        .snapshot
        .issues
        .push(issue(3, "Crash on save", Severity::High));
    value.welcome = false;
    value.tab = Tab::Issues;

    update(&mut value, &Key::Char('S'));
    let input = value.input.as_ref().expect("schedule prompt");
    assert_eq!(input.editor.value(), "4");
    assert_eq!(
        update(&mut value, &Key::Enter),
        Some(Effect::Mutate {
            mutation: Mutation::ScheduleIssue {
                id: 3,
                plan_id: 4,
                task_title: "Crash on save".to_owned(),
            },
            success: Success::Message("issue #3 scheduled into plan #4".to_owned()),
        })
    );
}

#[test]
fn schedule_rejects_a_non_numeric_plan() {
    let mut value = model();
    value.snapshot.issues.push(issue(3, "Crash", Severity::Low));
    value.welcome = false;
    value.tab = Tab::Issues;

    update(&mut value, &Key::Char('S'));
    assert_eq!(type_and_enter(&mut value, "abc"), None);
    assert_eq!(value.status, "enter a valid target plan ID");
}

#[test]
fn search_lists_matches_and_opens_the_selected_one() {
    let mut value = model();
    value.snapshot.plans.push(plan(1, None));
    value.snapshot.tasks.push(task(2, 1, "Write schema", None));
    value.snapshot.tasks.push(task(5, 1, "Review schema", None));
    value
        .snapshot
        .issues
        .push(issue(3, "Unrelated", Severity::Low));
    value.welcome = false;
    value.tab = Tab::Issues;

    update(&mut value, &Key::Char('/'));
    assert_eq!(type_and_enter(&mut value, "SCHEMA"), None);
    let search = value.search.as_ref().expect("search results");
    assert_eq!(search.hits.len(), 2);

    update(&mut value, &Key::Down);
    update(&mut value, &Key::Enter);
    assert!(value.search.is_none());
    assert_eq!(value.detail, Some(DetailTarget::Task(5)));
}

#[test]
fn search_with_no_matches_reports_it_and_escape_closes_results() {
    let mut value = model();
    value.snapshot.plans.push(plan(1, None));
    value.welcome = false;

    update(&mut value, &Key::Char('/'));
    type_and_enter(&mut value, "nothing here");
    assert!(value.search.is_none());
    assert_eq!(value.status, "no matches for “nothing here”");

    update(&mut value, &Key::Char('/'));
    type_and_enter(&mut value, "plan");
    assert!(value.search.is_some());
    update(&mut value, &Key::Escape);
    assert!(value.search.is_none());
}

#[test]
fn search_hits_on_notes_open_the_note_target() {
    let mut value = model();
    value.snapshot.plans.push(plan(1, None));
    value.snapshot.tasks.push(task(2, 1, "Card", None));
    value.snapshot.notes.push(Note {
        id: 9,
        target: NoteTarget::Task,
        target_id: 2,
        body: "Chose redb for storage".to_owned(),
        kind: MemoryKind::Decision,
        created_at: Timestamp::Zero,
        actor: None,
        ulid: None,
    });
    value.welcome = false;

    update(&mut value, &Key::Char('/'));
    type_and_enter(&mut value, "redb");
    update(&mut value, &Key::Enter);
    assert_eq!(value.detail, Some(DetailTarget::Task(2)));
}
