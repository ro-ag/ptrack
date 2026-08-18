use crate::test_support::{issue, meta, note, plan, snapshot, task};
use crate::{
    IssueStatus, MemoryKind, NoteTarget, PlanStatus, ProjectSnapshot, Severity, TaskStatus, context,
};

#[test]
fn context_markdown_is_byte_exact_with_the_go_report() {
    let digest = context(&snapshot());
    assert_eq!(
        digest.markdown(),
        "# ptrack context\n\
\n\
## Goal\n\
Ship the widget service\n\
\n\
## Summary\n\
Storage layer landed; wiring CLI\n\
\n\
## Active plan\n\
**#1 Build CLI**\n\
\n\
### Open tasks\n\
- [doing] #1 context command\n\
- [blocked] #3 publish release\n\
\n\
## Blocked (project-wide)\n\
- #3 publish release (plan 1)\n\
\n\
## Open issues\n\
- #1 [high] Release blocker (task 3)\n\
\n\
## Recent decisions\n\
- [handoff] (task #1) resume here\n\
- [decision] (plan #1) use dependency-free reports\n\
- (project) legacy decision\n\
\n\
## Inventory\n\
1 milestones (0 done) · 2 plans (1 done) · 4 tasks (1 done · 1 blocked · 3 open) · 2 issues (1 open) · 3 notes\n\
\n\
Drill deeper: `ptrack next` · `ptrack milestone list` · `ptrack plan show <id>` · `ptrack task show <id>` · `ptrack task list --status doing,blocked` · `ptrack issue list` · `ptrack note list` · `ptrack search <term>` · `ptrack board`\n"
    );
}

#[test]
fn context_moves_held_tasks_out_of_the_pick_up_list_into_their_own_bucket() {
    let mut snapshot = snapshot();
    snapshot.plans[0].hold_reason = Some("budget freeze".to_owned());
    snapshot.tasks[1].hold_reason = Some("waiting on review".to_owned());
    let digest = context(&snapshot);

    assert_eq!(
        digest
            .active_plan
            .as_ref()
            .expect("active plan")
            .open_tasks
            .iter()
            .map(|task| task.id)
            .collect::<Vec<_>>(),
        vec![3]
    );
    assert_eq!(
        digest
            .on_hold
            .iter()
            .map(|task| task.id)
            .collect::<Vec<_>>(),
        vec![1]
    );
    // A hold is orthogonal to status, so a held task keeps its blocked listing.
    assert_eq!(
        digest
            .blocked
            .iter()
            .map(|task| task.id)
            .collect::<Vec<_>>(),
        vec![3]
    );

    let markdown = digest.markdown();
    assert!(markdown.contains("**#1 Build CLI** [on hold: budget freeze]\n"));
    assert!(markdown.contains(
        "## On hold (project-wide)\n- #1 context command (plan 1) [on hold: waiting on review]\n"
    ));
    assert!(markdown.contains("2 plans (1 done · 1 on hold)"));
    assert!(markdown.contains("4 tasks (1 done · 1 blocked · 3 open · 1 on hold)"));
}

#[test]
fn context_bounds_project_wide_lists_and_uses_newest_notes() {
    let tasks = (1..=10)
        .map(|id| {
            task(
                id,
                1,
                &format!("blocked {id}"),
                TaskStatus::Blocked,
                i64::try_from(id).expect("small fixture id fits i64"),
            )
        })
        .collect();
    let issues = (1..=10)
        .map(|id| {
            issue(
                id,
                &format!("issue {id}"),
                "",
                IssueStatus::Open,
                Severity::Medium,
                0,
            )
        })
        .collect();
    let notes = (1..=7)
        .map(|id| {
            note(
                id,
                NoteTarget::Project,
                0,
                MemoryKind::Decision,
                &format!("note {id}"),
            )
        })
        .collect();
    let snapshot = ProjectSnapshot::new(
        meta(0),
        Vec::new(),
        vec![plan(1, "plan", PlanStatus::Active, 0, 0)],
        tasks,
        issues,
        notes,
        Vec::new(),
    );

    let digest = context(&snapshot);
    assert_eq!(digest.blocked.len(), 8);
    assert_eq!(digest.blocked_more, 2);
    assert_eq!(digest.blocked[0].id, 1);
    assert_eq!(digest.blocked[7].id, 8);
    assert_eq!(digest.open_issues.len(), 8);
    assert_eq!(digest.open_issues_more, 2);
    assert_eq!(
        digest
            .recent_notes
            .iter()
            .map(|note| note.id)
            .collect::<Vec<_>>(),
        vec![7, 6, 5, 4, 3]
    );
    assert!(
        digest
            .markdown()
            .contains("- … +2 more (use `ptrack task list --status blocked`)")
    );
    assert!(
        digest
            .markdown()
            .contains("- … +2 more (use `ptrack issue list`)")
    );
}

#[test]
fn context_silently_omits_a_missing_active_plan() {
    let snapshot = ProjectSnapshot::new(
        meta(99),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let digest = context(&snapshot);
    assert!(digest.active_plan.is_none());
    assert!(digest.markdown().contains("## Active plan\n_none_\n"));
}
