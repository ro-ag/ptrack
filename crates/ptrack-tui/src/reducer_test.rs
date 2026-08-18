use std::path::PathBuf;

use ptrack_app::Mutation;
use ptrack_core::{
    Meta, NoteTarget, Plan, PlanStatus, ProjectSnapshot, Task, TaskStatus, Timestamp,
};

use crate::model::{DetailTarget, PaneFocus};
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
fn five_tabs_and_menu_layering_are_stable() {
    let mut value = model();
    update(&mut value, &Key::Char('5'));
    assert_eq!(value.tab, Tab::Maintenance);
    update(&mut value, &Key::Char('?'));
    assert!(value.menu);
    update(&mut value, &Key::Char('2'));
    assert_eq!(value.tab, Tab::Board);
    assert!(!value.menu);
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
