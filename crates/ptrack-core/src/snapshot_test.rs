use crate::test_support::{meta, milestone, plan, snapshot, task};
use crate::{Counts, PlanStatus, ProjectSnapshot, TaskStatus};

#[test]
fn snapshot_normalizes_stable_display_and_insertion_order() {
    let snapshot = ProjectSnapshot::new(
        meta(0),
        vec![milestone(4, 1), milestone(2, 1), milestone(3, 0)],
        vec![
            plan(4, "four", PlanStatus::Active, 0, 1),
            plan(2, "two", PlanStatus::Active, 0, 1),
            plan(3, "three", PlanStatus::Active, 0, 0),
        ],
        vec![
            task(4, 1, "four", TaskStatus::Todo, 1),
            task(2, 1, "two", TaskStatus::Todo, 1),
            task(3, 1, "three", TaskStatus::Todo, 0),
        ],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );

    assert_eq!(
        snapshot
            .milestones
            .iter()
            .map(|value| value.id)
            .collect::<Vec<_>>(),
        vec![3, 2, 4]
    );
    assert_eq!(
        snapshot
            .plans
            .iter()
            .map(|value| value.id)
            .collect::<Vec<_>>(),
        vec![3, 2, 4]
    );
    assert_eq!(
        snapshot
            .tasks
            .iter()
            .map(|value| value.id)
            .collect::<Vec<_>>(),
        vec![3, 2, 4]
    );
}

#[test]
fn snapshot_indexes_filters_recent_notes_and_counts() {
    let snapshot = snapshot();

    assert_eq!(
        snapshot.plan(1).map(|plan| plan.title.as_str()),
        Some("Build CLI")
    );
    assert!(snapshot.plan(99).is_none());
    assert_eq!(
        snapshot.task(3).map(|task| task.title.as_str()),
        Some("publish release")
    );
    assert_eq!(snapshot.milestone(1).map(|value| value.id), Some(1));
    assert_eq!(snapshot.issue(1).map(|value| value.id), Some(1));
    assert_eq!(
        snapshot
            .tasks_for_plan(1)
            .map(|task| task.id)
            .collect::<Vec<_>>(),
        vec![2, 1, 3]
    );
    assert_eq!(
        snapshot
            .notes_for_plan(1)
            .map(|note| note.id)
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(
        snapshot
            .recent_notes(2)
            .into_iter()
            .map(|note| note.id)
            .collect::<Vec<_>>(),
        vec![3, 2]
    );
    assert_eq!(snapshot.recent_notes(0).len(), 3);

    assert_eq!(
        snapshot.counts(),
        Counts {
            milestones: 1,
            milestones_done: 0,
            plans: 2,
            plans_done: 1,
            tasks: 4,
            tasks_done: 1,
            tasks_blocked: 1,
            tasks_open: 3,
            issues: 2,
            issues_open: 1,
            commits: 1,
            notes: 3,
        }
    );
}
