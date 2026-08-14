use ptrack_app::{Mutation, MutationResult};
use ptrack_core::{IssueStatus, MilestoneStatus, NoteTarget, PlanStatus, TaskStatus};

use crate::input::Key;
use crate::model::{BOARD_STATUSES, Effect, InputPurpose, Model, PaneFocus, Success, Tab};

const MENU_LEN: usize = 12;

macro_rules! mutate {
    ($mutation:expr, $success:expr $(,)?) => {
        Some(Effect::Mutate {
            mutation: $mutation,
            success: $success,
        })
    };
}

/// Applies one key event and returns at most one explicit application effect.
/// Modal precedence deliberately matches the Go implementation. In particular,
/// active text input consumes Ctrl+C; it does not quit despite an older matrix
/// row's over-broad wording.
pub fn update(model: &mut Model, key: &Key) -> Option<Effect> {
    if model.welcome {
        return update_welcome(model, key);
    }
    if model.input.is_some() {
        return update_input(model, key);
    }
    if model.menu {
        return update_menu(model, key);
    }
    if model.detail.is_some() {
        return update_detail(model, key);
    }
    update_normal(model, key)
}

fn update_welcome(model: &mut Model, key: &Key) -> Option<Effect> {
    model.welcome = false;
    match key {
        Key::Char('q') | Key::Ctrl('c') => Some(Effect::Quit),
        Key::Enter | Key::Char(' ' | '1') => {
            model.tab = Tab::Overview;
            None
        }
        Key::Char(value @ '2'..='5') => {
            model.tab = Tab::from_index(value.to_digit(10).unwrap_or(1) as usize - 1);
            None
        }
        Key::Char('?') | Key::F(1) => {
            model.menu = true;
            model.menu_cursor = 0;
            None
        }
        _ => {
            model.welcome = true;
            None
        }
    }
}

fn update_input(model: &mut Model, key: &Key) -> Option<Effect> {
    match key {
        Key::Enter => return commit_input(model),
        Key::Escape => {
            if model.input.as_ref().is_some_and(|input| {
                matches!(
                    input.purpose,
                    InputPurpose::MoveTask | InputPurpose::ConvertTask
                )
            }) {
                model.pending_task_id = 0;
            }
            model.input = None;
            "cancelled".clone_into(&mut model.status);
        }
        // Bubble Tea's textinput owns this key before global dispatch. Preserve
        // the observed source behavior: it is consumed and does not quit.
        Key::Ctrl('c') => {}
        _ => model
            .input
            .as_mut()
            .expect("input exists")
            .editor
            .apply(key),
    }
    None
}

fn commit_input(model: &mut Model) -> Option<Effect> {
    let input = model.input.take().expect("input exists");
    let value = input.editor.value().trim().to_owned();
    match input.purpose {
        InputPurpose::EditGoal => mutate!(
            Mutation::SetGoal(value),
            Success::Message("goal updated".to_owned()),
        ),
        InputPurpose::EditSummary => mutate!(
            Mutation::SetSummary(value),
            Success::Message("summary updated".to_owned()),
        ),
        InputPurpose::AddPlan => add_named(
            model,
            &value,
            Mutation::AddPlan {
                title: value.clone(),
                milestone_id: 0,
            },
            "plan",
        ),
        InputPurpose::AddMilestone => add_named(
            model,
            &value,
            Mutation::AddMilestone {
                title: value.clone(),
                due: ptrack_core::Timestamp::Zero,
            },
            "milestone",
        ),
        InputPurpose::AddIssue => add_named(
            model,
            &value,
            Mutation::AddIssue {
                title: value.clone(),
                body: String::new(),
                severity: None,
                task_id: 0,
            },
            "issue",
        ),
        InputPurpose::AddTask => {
            let Some(plan_id) = model.current_plan().map(|plan| plan.id) else {
                "no plan selected".clone_into(&mut model.status);
                return None;
            };
            add_named(
                model,
                &value,
                Mutation::AddTask {
                    plan_id,
                    title: value.clone(),
                },
                "task",
            )
        }
        InputPurpose::AddNote => add_note(model, &value),
        InputPurpose::Rename => rename(model, &value),
        InputPurpose::MoveTask => move_task(model, &value),
        InputPurpose::ConvertTask => convert_task(model, &value),
    }
}

fn add_named(
    model: &mut Model,
    value: &str,
    mutation: Mutation,
    kind: &'static str,
) -> Option<Effect> {
    if value.is_empty() {
        "cancelled".clone_into(&mut model.status);
        None
    } else {
        mutate!(mutation, Success::Added(kind))
    }
}

fn add_note(model: &mut Model, value: &str) -> Option<Effect> {
    if value.is_empty() {
        "cancelled".clone_into(&mut model.status);
        return None;
    }
    let (target, target_id) = if model.tab == Tab::Issues && model.current_issue().is_some() {
        (NoteTarget::Project, 0)
    } else if model.tab == Tab::Board {
        model
            .board_task()
            .map_or((NoteTarget::Project, 0), |task| (NoteTarget::Task, task.id))
    } else if model.tab == Tab::Overview && model.focus == PaneFocus::Tasks {
        model.current_task().map_or_else(
            || {
                model
                    .current_plan()
                    .map_or((NoteTarget::Project, 0), |plan| (NoteTarget::Plan, plan.id))
            },
            |task| (NoteTarget::Task, task.id),
        )
    } else {
        model
            .current_plan()
            .map_or((NoteTarget::Project, 0), |plan| (NoteTarget::Plan, plan.id))
    };
    mutate!(
        Mutation::AddNote {
            target,
            target_id,
            body: value.to_owned(),
        },
        Success::Message("note added".to_owned()),
    )
}

fn rename(model: &mut Model, value: &str) -> Option<Effect> {
    let Some((kind, id, _)) = model.rename_target() else {
        "nothing to rename".clone_into(&mut model.status);
        return None;
    };
    if value.is_empty() {
        "nothing to rename".clone_into(&mut model.status);
        return None;
    }
    let mutation = match kind {
        "plan" => Mutation::SetPlanTitle {
            id,
            title: value.to_owned(),
        },
        "task" => Mutation::SetTaskTitle {
            id,
            title: value.to_owned(),
        },
        "milestone" => Mutation::SetMilestoneTitle {
            id,
            title: value.to_owned(),
        },
        "issue" => Mutation::SetIssueTitle {
            id,
            title: value.to_owned(),
        },
        _ => return None,
    };
    mutate!(mutation, Success::Message("renamed".to_owned()))
}

fn move_task(model: &mut Model, value: &str) -> Option<Effect> {
    let task_id = std::mem::take(&mut model.pending_task_id);
    let Ok(plan_id) = value.parse::<u64>() else {
        "enter a valid target plan ID".clone_into(&mut model.status);
        return None;
    };
    if plan_id == 0 {
        "enter a valid target plan ID".clone_into(&mut model.status);
        return None;
    }
    mutate!(
        Mutation::SetTaskPlan {
            id: task_id,
            plan_id,
        },
        Success::Message(format!("task moved to plan #{plan_id}")),
    )
}

fn convert_task(model: &mut Model, value: &str) -> Option<Effect> {
    if !value.eq_ignore_ascii_case("yes") {
        model.pending_task_id = 0;
        "cancelled".clone_into(&mut model.status);
        return None;
    }
    let task_id = std::mem::take(&mut model.pending_task_id);
    mutate!(
        Mutation::ConvertTaskToPlan(task_id),
        Success::ConvertedTask(task_id),
    )
}

fn update_menu(model: &mut Model, key: &Key) -> Option<Effect> {
    match key {
        Key::Char('q') | Key::Ctrl('c') => return Some(Effect::Quit),
        Key::Char('?') | Key::F(1) | Key::Escape => {
            model.menu = false;
            return None;
        }
        Key::Up | Key::Char('k') => {
            model.menu_cursor = model.menu_cursor.saturating_sub(1);
            return None;
        }
        Key::Down | Key::Char('j') => {
            model.menu_cursor = (model.menu_cursor + 1).min(MENU_LEN - 1);
            return None;
        }
        Key::Enter => return menu_action(model, model.menu_cursor),
        _ => {}
    }
    let direct = match key {
        Key::Char('1') => Some(0),
        Key::Char('2') => Some(1),
        Key::Char('3') => Some(2),
        Key::Char('4') => Some(3),
        Key::Char('5') => Some(4),
        Key::Char('g') => Some(5),
        Key::Char('m') => Some(6),
        Key::Char('e') => Some(7),
        Key::Char('M') => Some(8),
        Key::Char('P') => Some(9),
        Key::Char('r') => Some(10),
        Key::Char('B') => Some(11),
        _ => None,
    };
    direct.and_then(|action| menu_action(model, action))
}

fn menu_action(model: &mut Model, action: usize) -> Option<Effect> {
    model.menu = false;
    match action {
        0..=4 => {
            model.detail = None;
            model.tab = Tab::from_index(action);
            None
        }
        5 => {
            let initial = model.snapshot.meta.goal.clone();
            model.start_input(InputPurpose::EditGoal, "Goal:", &initial);
            None
        }
        6 => {
            let initial = model.snapshot.meta.summary.clone();
            model.start_input(InputPurpose::EditSummary, "Summary:", &initial);
            None
        }
        7 => start_rename(model, "nothing to edit"),
        8 => {
            model.detail = None;
            start_move(model)
        }
        9 => {
            model.detail = None;
            start_convert(model)
        }
        10 => Some(Effect::Reload {
            success: "project reloaded".to_owned(),
            reopen_detail: model.detail.is_some(),
        }),
        11 => Some(Effect::Backup),
        _ => None,
    }
}

fn update_detail(model: &mut Model, key: &Key) -> Option<Effect> {
    let maximum = crate::render::detail_scroll_max(model);
    model.detail_offset = model.detail_offset.min(maximum);
    match key {
        Key::Char('q') | Key::Ctrl('c') => Some(Effect::Quit),
        Key::Char('?') | Key::F(1) => {
            model.menu = true;
            model.menu_cursor = 0;
            None
        }
        Key::Escape | Key::Enter | Key::Backspace => {
            model.detail = None;
            None
        }
        Key::Up | Key::Char('k') => {
            model.detail_offset = model.detail_offset.saturating_sub(1);
            None
        }
        Key::Down | Key::Char('j') => {
            model.detail_offset = (model.detail_offset + 1).min(maximum);
            None
        }
        Key::PageUp => {
            model.detail_offset = model.detail_offset.saturating_sub(10);
            None
        }
        Key::PageDown | Key::Char(' ') => {
            model.detail_offset = model.detail_offset.saturating_add(10).min(maximum);
            None
        }
        Key::Char('r') => Some(Effect::Reload {
            success: String::new(),
            reopen_detail: true,
        }),
        Key::Char('e') => start_rename(model, "nothing to edit"),
        Key::Char('M') => {
            model.detail = None;
            start_move(model)
        }
        Key::Char('P') => {
            model.detail = None;
            start_convert(model)
        }
        _ => None,
    }
}

fn update_normal(model: &mut Model, key: &Key) -> Option<Effect> {
    match key {
        Key::Char('q') | Key::Ctrl('c') => return Some(Effect::Quit),
        Key::Char('?') | Key::F(1) => {
            model.menu = true;
            model.menu_cursor = 0;
            return None;
        }
        Key::Tab => {
            model.tab = Tab::from_index(model.tab.index() + 1);
            return None;
        }
        Key::BackTab => {
            model.tab = Tab::from_index(model.tab.index() + 4);
            return None;
        }
        Key::Char(value @ '1'..='5') => {
            model.tab = Tab::from_index(value.to_digit(10).unwrap_or(1) as usize - 1);
            return None;
        }
        Key::Char('g') => {
            let initial = model.snapshot.meta.goal.clone();
            model.start_input(InputPurpose::EditGoal, "Goal:", &initial);
            return None;
        }
        Key::Char('m') => {
            let initial = model.snapshot.meta.summary.clone();
            model.start_input(InputPurpose::EditSummary, "Summary:", &initial);
            return None;
        }
        Key::Char('e') => return start_rename(model, "nothing to rename"),
        Key::Enter => {
            model.detail = model.selected_detail();
            model.detail_offset = 0;
            if model.detail.is_none() {
                "nothing to open".clone_into(&mut model.status);
            }
            return None;
        }
        Key::Char('r') => {
            return Some(Effect::Reload {
                success: "reloaded".to_owned(),
                reopen_detail: false,
            });
        }
        Key::Char('B') => return Some(Effect::Backup),
        _ => {}
    }
    match model.tab {
        Tab::Overview => update_overview(model, key),
        Tab::Board => update_board(model, key),
        Tab::Milestones => update_milestones(model, key),
        Tab::Issues => update_issues(model, key),
        Tab::Maintenance => None,
    }
}

fn update_overview(model: &mut Model, key: &Key) -> Option<Effect> {
    match key {
        Key::Left | Key::Right | Key::Char('h' | 'l') => {
            model.focus = if model.focus == PaneFocus::Plans {
                PaneFocus::Tasks
            } else {
                PaneFocus::Plans
            };
        }
        Key::Up | Key::Char('k') => move_overview(model, false),
        Key::Down | Key::Char('j') => move_overview(model, true),
        Key::Char('a') if model.focus == PaneFocus::Plans => {
            model.start_input(InputPurpose::AddPlan, "New plan:", "");
        }
        Key::Char('a') => {
            if model.current_plan().is_none() {
                "add a plan first".clone_into(&mut model.status);
            } else {
                model.start_input(InputPurpose::AddTask, "New task:", "");
            }
        }
        Key::Char('n') => model.start_input(InputPurpose::AddNote, "Note:", ""),
        Key::Char('u') => {
            if let Some(id) = model.current_plan().map(|plan| plan.id) {
                return mutate!(
                    Mutation::SetActivePlan(id),
                    Success::Message("active plan set".to_owned()),
                );
            }
        }
        Key::Char('x') => {
            if let Some(id) = model.current_plan().map(|plan| plan.id) {
                return mutate!(
                    Mutation::SetPlanStatus {
                        id,
                        status: PlanStatus::Done,
                    },
                    Success::Message("plan done".to_owned()),
                );
            }
        }
        Key::Char('s') => return set_task(model, TaskStatus::Doing, "task started"),
        Key::Char('d') => return set_task(model, TaskStatus::Done, "task done"),
        Key::Char('b') => return set_task(model, TaskStatus::Blocked, "task blocked"),
        Key::Char('M') => return start_move(model),
        Key::Char('P') => return start_convert(model),
        _ => {}
    }
    None
}

fn move_overview(model: &mut Model, down: bool) {
    if model.focus == PaneFocus::Plans {
        model.plan_cursor = if down {
            (model.plan_cursor + 1).min(model.snapshot.plans.len().saturating_sub(1))
        } else {
            model.plan_cursor.saturating_sub(1)
        };
        model.task_cursor = 0;
    } else if down {
        model.task_cursor =
            (model.task_cursor + 1).min(model.current_tasks().count().saturating_sub(1));
    } else {
        model.task_cursor = model.task_cursor.saturating_sub(1);
    }
}

fn set_task(model: &mut Model, status: TaskStatus, message: &str) -> Option<Effect> {
    let Some(id) = model.current_task().map(|task| task.id) else {
        "no task selected".clone_into(&mut model.status);
        return None;
    };
    mutate!(
        Mutation::SetTaskStatus { id, status },
        Success::Message(message.to_owned()),
    )
}

fn update_board(model: &mut Model, key: &Key) -> Option<Effect> {
    match key {
        Key::Left | Key::Char('h') => {
            model.board_col = model.board_col.saturating_sub(1);
            model.board_row = model
                .board_row
                .min(model.board_tasks(model.board_col).count().saturating_sub(1));
        }
        Key::Right | Key::Char('l') => {
            model.board_col = (model.board_col + 1).min(BOARD_STATUSES.len() - 1);
            model.board_row = model
                .board_row
                .min(model.board_tasks(model.board_col).count().saturating_sub(1));
        }
        Key::Up | Key::Char('k') => model.board_row = model.board_row.saturating_sub(1),
        Key::Down | Key::Char('j') => {
            model.board_row = (model.board_row + 1)
                .min(model.board_tasks(model.board_col).count().saturating_sub(1));
        }
        Key::Char('H' | '<') => return move_card(model, false),
        Key::Char('L' | '>') => return move_card(model, true),
        Key::Char('a') => {
            if model.current_plan().is_none() {
                "add a plan first".clone_into(&mut model.status);
            } else {
                model.start_input(InputPurpose::AddTask, "New task:", "");
            }
        }
        Key::Char('n') => {
            if model.board_task().is_none() {
                "no card selected".clone_into(&mut model.status);
            } else {
                model.start_input(InputPurpose::AddNote, "Note:", "");
            }
        }
        Key::Char('M') => return start_move(model),
        Key::Char('P') => return start_convert(model),
        _ => {}
    }
    None
}

fn move_card(model: &mut Model, right: bool) -> Option<Effect> {
    let Some(id) = model.board_task().map(|task| task.id) else {
        "no card selected".clone_into(&mut model.status);
        return None;
    };
    let column = if right {
        model.board_col.checked_add(1)
    } else {
        model.board_col.checked_sub(1)
    };
    let column = column.filter(|column| *column < BOARD_STATUSES.len())?;
    let status = BOARD_STATUSES[column];
    mutate!(
        Mutation::SetTaskStatus { id, status },
        Success::MovedCard {
            message: format!("moved #{id} → {status}"),
            column,
        },
    )
}

fn update_milestones(model: &mut Model, key: &Key) -> Option<Effect> {
    match key {
        Key::Up | Key::Char('k') => {
            model.milestone_cursor = model.milestone_cursor.saturating_sub(1);
        }
        Key::Down | Key::Char('j') => {
            model.milestone_cursor =
                (model.milestone_cursor + 1).min(model.snapshot.milestones.len().saturating_sub(1));
        }
        Key::Char('a') => model.start_input(InputPurpose::AddMilestone, "New milestone:", ""),
        Key::Char('x' | 'o') => {
            if let Some(id) = model.current_milestone().map(|milestone| milestone.id) {
                let (status, message) = if matches!(key, Key::Char('x')) {
                    (MilestoneStatus::Done, "milestone done")
                } else {
                    (MilestoneStatus::Open, "milestone reopened")
                };
                return mutate!(
                    Mutation::SetMilestoneStatus { id, status },
                    Success::Message(message.to_owned()),
                );
            }
        }
        _ => {}
    }
    None
}

fn update_issues(model: &mut Model, key: &Key) -> Option<Effect> {
    match key {
        Key::Up | Key::Char('k') => model.issue_cursor = model.issue_cursor.saturating_sub(1),
        Key::Down | Key::Char('j') => {
            model.issue_cursor =
                (model.issue_cursor + 1).min(model.snapshot.issues.len().saturating_sub(1));
        }
        Key::Char('a') => model.start_input(InputPurpose::AddIssue, "New issue:", ""),
        Key::Char('c' | 'o') => {
            if let Some(id) = model.current_issue().map(|issue| issue.id) {
                let (status, message) = if matches!(key, Key::Char('c')) {
                    (IssueStatus::Closed, "issue closed")
                } else {
                    (IssueStatus::Open, "issue reopened")
                };
                return mutate!(
                    Mutation::SetIssueStatus { id, status },
                    Success::Message(message.to_owned()),
                );
            }
        }
        _ => {}
    }
    None
}

fn start_rename(model: &mut Model, empty_message: &str) -> Option<Effect> {
    let Some((_, _, title)) = model.rename_target() else {
        empty_message.clone_into(&mut model.status);
        return None;
    };
    let initial = title.to_owned();
    model.start_input(InputPurpose::Rename, "Rename:", &initial);
    None
}

fn start_move(model: &mut Model) -> Option<Effect> {
    let Some(id) = model.selected_task().map(|task| task.id) else {
        "no task selected".clone_into(&mut model.status);
        return None;
    };
    model.pending_task_id = id;
    model.start_input(InputPurpose::MoveTask, "Move to plan ID:", "");
    None
}

fn start_convert(model: &mut Model) -> Option<Effect> {
    let Some(id) = model.selected_task().map(|task| task.id) else {
        "no task selected".clone_into(&mut model.status);
        return None;
    };
    model.pending_task_id = id;
    model.start_input(
        InputPurpose::ConvertTask,
        format!("Convert task #{id} to a plan? Type yes:"),
        "",
    );
    None
}

pub(crate) fn success_message(success: &Success, result: &MutationResult) -> Option<String> {
    match success {
        Success::Message(message) | Success::MovedCard { message, .. } => Some(message.clone()),
        Success::Added(kind) => match result {
            MutationResult::Plan(value) if *kind == "plan" => {
                Some(format!("added plan #{}", value.id))
            }
            MutationResult::Task(value) if *kind == "task" => {
                Some(format!("added task #{}", value.id))
            }
            MutationResult::Milestone(value) if *kind == "milestone" => {
                Some(format!("added milestone #{}", value.id))
            }
            MutationResult::Issue(value) if *kind == "issue" => {
                Some(format!("added issue #{}", value.id))
            }
            _ => None,
        },
        Success::ConvertedTask(task_id) => match result {
            MutationResult::Plan(value) => {
                Some(format!("converted task #{task_id} to plan #{}", value.id))
            }
            _ => None,
        },
    }
}
