use crate::test_support::{issue, meta, plan, snapshot, task};
use crate::{
    IssueStatus, PlanStatus, ProjectSnapshot, ReportError, Severity, TaskStatus, Timestamp,
    board_for, next, show_issue, show_milestone, show_plan, show_task,
};

#[test]
fn next_matches_active_plan_priority_and_messages() {
    let view = next(&snapshot()).expect("active plan exists");
    assert_eq!(view.task.as_ref().map(|task| task.id), Some(1));
    assert_eq!(
        view.markdown(),
        "next: [doing] #1 context command (plan: Build CLI)\n"
    );

    let no_active = ProjectSnapshot::new(
        meta(0),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    assert_eq!(
        next(&no_active)
            .expect("no active plan is not an error")
            .markdown(),
        "no active plan (set one with 'ptrack plan use <id>')\n"
    );

    let no_action = ProjectSnapshot::new(
        meta(1),
        Vec::new(),
        vec![plan(1, "Done work", PlanStatus::Active, 0, 0)],
        vec![
            task(1, 1, "done", TaskStatus::Done, 0),
            task(2, 1, "blocked", TaskStatus::Blocked, 1),
        ],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    assert_eq!(
        next(&no_action).expect("active plan exists").markdown(),
        "no actionable task in the active plan\n"
    );
}

#[test]
fn next_skips_held_tasks_and_stops_at_a_held_active_plan() {
    // Task #1 is the doing pick; holding it leaves nothing actionable.
    let mut held = snapshot();
    held.tasks[1].hold_reason = Some("waiting on review".to_owned());
    assert_eq!(
        next(&held).expect("active plan exists").markdown(),
        "no actionable task in the active plan\n"
    );

    held.tasks.push(task(5, 1, "fallback", TaskStatus::Todo, 5));
    let view = next(&held).expect("active plan exists");
    assert_eq!(view.task.as_ref().map(|task| task.id), Some(5));

    let mut held_plan = snapshot();
    held_plan.plans[0].hold_reason = Some("budget freeze".to_owned());
    let view = next(&held_plan).expect("active plan exists");
    assert!(view.task.is_none());
    assert_eq!(view.markdown(), "active plan on hold: budget freeze\n");
    // The reason is a field, not something a consumer parses out of the prose.
    assert_eq!(view.plan_hold_reason.as_deref(), Some("budget freeze"));
    assert!(
        next(&snapshot())
            .expect("active plan exists")
            .plan_hold_reason
            .is_none()
    );
}

#[test]
fn holds_render_as_one_marker_in_plan_and_task_views() {
    let mut held = snapshot();
    held.plans[0].hold_reason = Some("budget freeze".to_owned());
    held.tasks[1].hold_reason = Some("waiting on review".to_owned());

    assert_eq!(
        show_plan(&held, 1).expect("plan exists").markdown(),
        "# Plan #1 Build CLI [active] [on hold: budget freeze]\n\
\n\
## Tasks\n\
- [done] #2 init command\n\
- [doing] #1 context command [on hold: waiting on review]\n\
- [blocked] #3 publish release\n\
\n\
## Notes\n\
- [decision] (plan #1) use dependency-free reports\n"
    );
    assert_eq!(
        show_task(&held, 1).expect("task exists").markdown(),
        "# Task #1 context command [doing] [on hold: waiting on review]\n\
\n\
Plan: #1 Build CLI [on hold: budget freeze]\n\
\n\
## Notes\n\
- [handoff] (task #1) resume here\n"
    );
    assert_eq!(
        board_for(&held, 1).expect("plan exists").markdown(),
        "# Board — #1 Build CLI\n\
\n\
## Todo (0)\n\
_none_\n\
\n\
## Doing (1)\n\
- #1 context command [on hold: waiting on review]\n\
\n\
## Blocked (1)\n\
- #3 publish release\n\
\n\
## Done (1)\n\
- #2 init command\n\
\n"
    );
}

#[test]
fn show_plan_and_task_markdown_are_byte_exact() {
    let snapshot = snapshot();
    assert_eq!(
        show_plan(&snapshot, 1).expect("plan exists").markdown(),
        "# Plan #1 Build CLI [active]\n\
\n\
## Tasks\n\
- [done] #2 init command\n\
- [doing] #1 context command\n\
- [blocked] #3 publish release\n\
\n\
## Notes\n\
- [decision] (plan #1) use dependency-free reports\n"
    );
    assert_eq!(
        show_task(&snapshot, 1).expect("task exists").markdown(),
        "# Task #1 context command [doing]\n\
\n\
Plan: #1 Build CLI\n\
\n\
## Notes\n\
- [handoff] (task #1) resume here\n"
    );
}

#[test]
fn milestone_issue_and_board_markdown_are_byte_exact() {
    let snapshot = snapshot();
    assert_eq!(
        show_milestone(&snapshot, 1)
            .expect("milestone exists")
            .markdown(),
        "# Milestone #1 Ship beta [open] (due 1969-12-31)\n\
\n\
Tasks: 1 done · 2 open\n\
\n\
## Plans\n\
- #1 Build CLI [active]\n"
    );
    assert_eq!(
        show_issue(&snapshot, 1).expect("issue exists").markdown(),
        "# Issue #1 Release blocker\n\
\n\
Status: open · Severity: high\n\
Task: #3 publish release\n\
\n\
waiting on registry\n"
    );
    assert_eq!(
        board_for(&snapshot, 1).expect("plan exists").markdown(),
        "# Board — #1 Build CLI\n\
\n\
## Todo (0)\n\
_none_\n\
\n\
## Doing (1)\n\
- #1 context command\n\
\n\
## Blocked (1)\n\
- #3 publish release\n\
\n\
## Done (1)\n\
- #2 init command\n\
\n"
    );
}

#[test]
fn milestone_due_date_pads_negative_years_after_the_sign() {
    let mut snapshot = snapshot();
    snapshot.milestones[0].due = Timestamp::Fixed {
        seconds: -62_198_755_200,
        nanoseconds: 0,
        offset_seconds: 0,
    };

    let view = show_milestone(&snapshot, 1).expect("milestone exists");
    assert_eq!(view.due, "-0001-01-01");
    assert!(view.markdown().contains("(due -0001-01-01)"));
}

#[test]
fn reference_resolution_is_tolerant_but_requested_roots_are_required() {
    let snapshot = ProjectSnapshot::new(
        meta(99),
        Vec::new(),
        Vec::new(),
        vec![task(1, 99, "orphan task", TaskStatus::Todo, 0)],
        vec![issue(
            1,
            "orphan issue",
            "",
            IssueStatus::Open,
            Severity::Medium,
            99,
        )],
        Vec::new(),
        Vec::new(),
    );

    assert_eq!(
        next(&snapshot),
        Err(ReportError::NotFound {
            entity: "plan",
            id: 99,
        })
    );
    assert!(show_task(&snapshot, 1).expect("task exists").plan.is_none());
    assert!(
        show_issue(&snapshot, 1)
            .expect("issue exists")
            .task
            .is_none()
    );
    assert_eq!(
        show_plan(&snapshot, 7),
        Err(ReportError::NotFound {
            entity: "plan",
            id: 7,
        })
    );
    assert_eq!(
        show_task(&snapshot, 7),
        Err(ReportError::NotFound {
            entity: "task",
            id: 7,
        })
    );
    assert_eq!(
        show_milestone(&snapshot, 7),
        Err(ReportError::NotFound {
            entity: "milestone",
            id: 7,
        })
    );
    assert_eq!(
        show_issue(&snapshot, 7),
        Err(ReportError::NotFound {
            entity: "issue",
            id: 7,
        })
    );
    assert_eq!(
        board_for(&snapshot, 7),
        Err(ReportError::NotFound {
            entity: "plan",
            id: 7,
        })
    );
}
