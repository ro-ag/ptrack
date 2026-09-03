use crate::test_support::{issue, meta, plan, snapshot, task};
use crate::{
    DepSkip, IssueStatus, PlanStatus, ProjectSnapshot, ReportError, Severity, TaskStatus,
    Timestamp, board_for, next, show_issue, show_milestone, show_plan, show_task,
};

#[test]
fn next_matches_active_plan_priority_and_messages() {
    let view = next(&snapshot()).expect("active plan exists");
    assert_eq!(view.task.as_ref().map(|task| task.id), Some(1));
    assert_eq!(
        view.markdown(),
        "Goal: Ship the widget service\nnext: [doing] #1 context command (plan: Build CLI)\n"
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
        "Goal: Ship the widget service\nno active plan (set one with 'ptrack plan use <id>')\n"
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
        "Goal: Ship the widget service\nno actionable task in the active plan\n"
    );
}

#[test]
fn next_only_selects_tasks_and_includes_their_open_issue_context() {
    let mut data = snapshot();
    data.issues = vec![
        issue(
            10,
            "triage only",
            "",
            IssueStatus::Open,
            Severity::Critical,
            0,
        ),
        issue(11, "scheduled", "", IssueStatus::Open, Severity::High, 1),
        issue(12, "closed", "", IssueStatus::Closed, Severity::High, 1),
    ];
    let view = next(&data).unwrap();
    assert_eq!(view.task.unwrap().id, 1);
    assert_eq!(
        view.issues.iter().map(|issue| issue.id).collect::<Vec<_>>(),
        [11]
    );
    data.tasks.clear();
    assert!(next(&data).unwrap().task.is_none());
}

#[test]
fn next_skips_held_tasks_and_stops_at_a_held_active_plan() {
    // Task #1 is the doing pick; holding it leaves nothing actionable.
    let mut held = snapshot();
    held.tasks[1].hold_reason = Some("waiting on review".to_owned());
    assert_eq!(
        next(&held).expect("active plan exists").markdown(),
        "Goal: Ship the widget service\nno actionable task in the active plan\n"
    );

    held.tasks.push(task(5, 1, "fallback", TaskStatus::Todo, 5));
    let view = next(&held).expect("active plan exists");
    assert_eq!(view.task.as_ref().map(|task| task.id), Some(5));

    let mut held_plan = snapshot();
    held_plan.plans[0].hold_reason = Some("budget freeze".to_owned());
    let view = next(&held_plan).expect("active plan exists");
    assert!(view.task.is_none());
    assert_eq!(
        view.markdown(),
        "Goal: Ship the widget service\nactive plan on hold: budget freeze\n"
    );
    // The reason is a field, not something a consumer parses out of the prose.
    assert_eq!(view.plan_hold_reason.as_deref(), Some("budget freeze"));
    assert!(
        next(&snapshot())
            .expect("active plan exists")
            .plan_hold_reason
            .is_none()
    );
}

/// One active plan with the given tasks; keeps dep tests free of fixture noise.
fn plan_snapshot(tasks: Vec<crate::Task>) -> ProjectSnapshot {
    ProjectSnapshot::new(
        meta(1),
        Vec::new(),
        vec![plan(1, "Build CLI", PlanStatus::Active, 0, 1)],
        tasks,
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
}

#[test]
fn next_skips_a_dep_blocked_task_and_names_its_blockers() {
    let mut first = task(1, 1, "write docs", TaskStatus::Todo, 1);
    first.deps = vec![3];
    let snapshot = plan_snapshot(vec![
        first,
        task(2, 1, "ship crate", TaskStatus::Todo, 2),
        task(3, 1, "cut release", TaskStatus::Blocked, 3),
    ]);
    let view = next(&snapshot).expect("active plan exists");
    assert_eq!(view.task.as_ref().map(|task| task.id), Some(2));
    assert_eq!(
        view.skipped,
        vec![DepSkip {
            task_id: 1,
            waiting_on: vec![3],
        }]
    );
    assert_eq!(
        view.markdown(),
        "Goal: Ship the widget service\nnext: [todo] #2 ship crate (plan: Build CLI)\nskipped: #1 (waiting on #3)\n"
    );
}

#[test]
fn next_reports_nothing_actionable_when_every_candidate_waits_on_deps() {
    let mut only = task(1, 1, "write docs", TaskStatus::Todo, 1);
    only.deps = vec![2, 3];
    let snapshot = plan_snapshot(vec![
        only,
        task(2, 1, "ship crate", TaskStatus::Blocked, 2),
        task(3, 1, "cut release", TaskStatus::Blocked, 3),
    ]);
    let view = next(&snapshot).expect("active plan exists");
    assert!(view.task.is_none());
    assert_eq!(
        view.markdown(),
        "Goal: Ship the widget service\nno actionable task in the active plan\nskipped: #1 (waiting on #2, #3)\n"
    );
}

#[test]
fn a_task_becomes_actionable_once_its_dep_target_is_done() {
    let mut first = task(1, 1, "write docs", TaskStatus::Todo, 1);
    first.deps = vec![3];
    let snapshot = plan_snapshot(vec![first, task(3, 1, "cut release", TaskStatus::Done, 3)]);
    let view = next(&snapshot).expect("active plan exists");
    assert_eq!(view.task.as_ref().map(|task| task.id), Some(1));
    assert!(view.skipped.is_empty());
    assert_eq!(
        view.markdown(),
        "Goal: Ship the widget service\nnext: [todo] #1 write docs (plan: Build CLI)\n"
    );
}

#[test]
fn open_plan_deps_block_every_task_of_the_active_plan() {
    let mut active = plan(1, "Build CLI", PlanStatus::Active, 0, 1);
    active.deps = vec![2];
    let mut snapshot = ProjectSnapshot::new(
        meta(1),
        Vec::new(),
        vec![active, plan(2, "Foundations", PlanStatus::Active, 0, 2)],
        vec![task(1, 1, "ready todo", TaskStatus::Todo, 1)],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let view = next(&snapshot).expect("active plan exists");
    assert!(view.task.is_none());
    assert_eq!(view.plan_waiting_on, vec![2]);
    assert_eq!(
        view.markdown(),
        "Goal: Ship the widget service\nactive plan waiting on #2\n"
    );

    // Finishing the dep plan unblocks the active plan's tasks.
    snapshot.plans[1].status = PlanStatus::Done;
    let view = next(&snapshot).expect("active plan exists");
    assert_eq!(view.task.as_ref().map(|task| task.id), Some(1));
    assert!(view.plan_waiting_on.is_empty());
}

#[test]
fn a_held_or_blocked_dep_target_counts_open_without_status_propagation() {
    let mut dependent = task(1, 1, "write docs", TaskStatus::Todo, 1);
    dependent.deps = vec![2, 3];
    let mut held_target = task(2, 1, "ship crate", TaskStatus::Todo, 2);
    held_target.hold_reason = Some("waiting on review".to_owned());
    let snapshot = plan_snapshot(vec![
        dependent,
        held_target,
        task(3, 1, "cut release", TaskStatus::Blocked, 3),
    ]);
    let view = next(&snapshot).expect("active plan exists");
    assert!(view.task.is_none());
    assert_eq!(
        view.skipped,
        vec![DepSkip {
            task_id: 1,
            waiting_on: vec![2, 3],
        }]
    );
    // Openness is computed in the view only: the dependent's stored status is
    // untouched by its held and blocked targets.
    let stored = snapshot.task(1).expect("task exists");
    assert_eq!(stored.status, TaskStatus::Todo);
    assert!(stored.hold_reason.is_none());
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
        "Goal: Ship the widget service\n# Task #1 context command [doing] [on hold: waiting on review]\n\
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
    assert_eq!(
        show_milestone(&held, 1)
            .expect("milestone exists")
            .markdown(),
        "# Milestone #1 Ship beta [open] (due 1969-12-31)\n\
\n\
Tasks: 1 done · 2 open\n\
\n\
## Plans\n\
- #1 Build CLI [active] [on hold: budget freeze]\n"
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
        "Goal: Ship the widget service\n# Task #1 context command [doing]\n\
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
fn claim_owner_resolves_to_actor_name_in_plan_view_and_markdown() {
    let mut claimed = snapshot();
    claimed.plans[0].claim_owner = Some("01hzvyekq3s7m8w9x0abcdefgh".to_owned());
    claimed
        .meta
        .actors
        .push(("01hzvyekq3s7m8w9x0abcdefgh".to_owned(), "Alice".to_owned()));

    let view = show_plan(&claimed, 1).expect("plan exists");
    assert_eq!(
        view.plan.claimed_by.as_deref(),
        Some("01hzvyekq3s7m8w9x0abcdefgh")
    );
    assert_eq!(view.plan.claimed_by_name.as_deref(), Some("Alice"));
    assert!(view.markdown().contains("[claimed: Alice]"));

    // Every text surface that renders a `PlanRef` carries the marker, not
    // just the plan's own show view.
    assert!(
        show_task(&claimed, 1)
            .expect("task exists")
            .markdown()
            .contains("Plan: #1 Build CLI [claimed: Alice]\n")
    );
    assert!(
        show_milestone(&claimed, 1)
            .expect("milestone exists")
            .markdown()
            .contains("[claimed: Alice]\n")
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

#[test]
fn checkpoint_reports_the_whole_picture_with_milestone_progress() {
    let view = crate::checkpoint(&snapshot(), Some(1));
    assert_eq!(view.open_plans, vec![(1, "Build CLI".to_owned())]);
    assert_eq!((view.open_issues, view.high_issues), (1, 1));
    assert_eq!(
        view.markdown(),
        "Goal: Ship the widget service\n\
         Rolling summary: Storage layer landed; wiring CLI\n\
         Remaining open plans: #1 Build CLI\n\
         Open issues: 1 (1 high)\n\
         Milestone: Ship beta — 0/1 plans done\n\
         \n\
         CHECKPOINT — before continuing, re-evaluate:\n\
         - Does the remaining roadmap still reach the goal? Missing plans? Obsolete ones?\n\
         - What did this plan change that the next plans must know?\n\
         - Update: ptrack summary set \"...\" | ptrack plan add \"...\" | ptrack issue add \"...\"\n"
    );
}

#[test]
fn checkpoint_names_the_missing_goal_and_summary_and_skips_the_milestone() {
    let mut bare = snapshot();
    bare.meta.goal = String::new();
    bare.meta.summary = String::new();
    let view = crate::checkpoint(&bare, None);
    let markdown = view.markdown();
    assert!(markdown.starts_with(
        "Goal: (not set — set one with 'ptrack goal set \"...\"')\nRolling summary: (not set)\n"
    ));
    assert!(!markdown.contains("Milestone:"));
}
