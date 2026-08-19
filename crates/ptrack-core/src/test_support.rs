use crate::{
    Commit, Issue, IssueStatus, MemoryKind, Meta, Milestone, MilestoneStatus, Note, NoteTarget,
    Plan, PlanStatus, ProjectSnapshot, Severity, Task, TaskStatus, Timestamp,
};

pub(crate) fn meta(active_plan: u64) -> Meta {
    Meta {
        goal: "Ship the widget service".to_owned(),
        summary: "Storage layer landed; wiring CLI".to_owned(),
        active_plan,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        format_version: 5,
        last_write_version: "v0.21.0".to_owned(),
        active_plans: Vec::new(),
        actors: Vec::new(),
    }
}

pub(crate) fn milestone(id: u64, order: i64) -> Milestone {
    Milestone {
        id,
        title: "Ship beta".to_owned(),
        status: MilestoneStatus::Open,
        due: Timestamp::Fixed {
            seconds: 0,
            nanoseconds: 0,
            offset_seconds: -3_600,
        },
        order,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        actor: None,
        ulid: None,
    }
}

pub(crate) fn plan(
    id: u64,
    title: &str,
    status: PlanStatus,
    milestone_id: u64,
    order: i64,
) -> Plan {
    Plan {
        id,
        title: title.to_owned(),
        status,
        milestone_id,
        order,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
        actor: None,
        claim_conflict: false,
        claim_epoch: 0,
        claim_owner: None,
        ulid: None,
        deps: Vec::new(),
    }
}

pub(crate) fn task(id: u64, plan_id: u64, title: &str, status: TaskStatus, order: i64) -> Task {
    Task {
        id,
        plan_id,
        title: title.to_owned(),
        status,
        order,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
        actor: None,
        ulid: None,
        deps: Vec::new(),
    }
}

pub(crate) fn issue(
    id: u64,
    title: &str,
    body: &str,
    status: IssueStatus,
    severity: Severity,
    task_id: u64,
) -> Issue {
    Issue {
        id,
        title: title.to_owned(),
        body: body.to_owned(),
        status,
        severity,
        task_id,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        actor: None,
        ulid: None,
    }
}

pub(crate) fn note(
    id: u64,
    target: NoteTarget,
    target_id: u64,
    kind: MemoryKind,
    body: &str,
) -> Note {
    Note {
        id,
        target,
        target_id,
        kind,
        body: body.to_owned(),
        created_at: Timestamp::Zero,
        actor: None,
        ulid: None,
    }
}

pub(crate) fn commit(id: u64) -> Commit {
    Commit {
        id,
        sha: format!("{id:040x}"),
        subject: "Implement report".to_owned(),
        plan_id: 1,
        task_id: 1,
        created_at: Timestamp::Zero,
        actor: None,
        ulid: None,
    }
}

pub(crate) fn snapshot() -> ProjectSnapshot {
    ProjectSnapshot::new(
        meta(1),
        vec![milestone(1, 1)],
        vec![
            plan(2, "Later work", PlanStatus::Done, 0, 2),
            plan(1, "Build CLI", PlanStatus::Active, 1, 1),
        ],
        vec![
            task(4, 2, "unrelated todo", TaskStatus::Todo, 4),
            task(3, 1, "publish release", TaskStatus::Blocked, 3),
            task(1, 1, "context command", TaskStatus::Doing, 2),
            task(2, 1, "init command", TaskStatus::Done, 1),
        ],
        vec![
            issue(
                2,
                "Resolved typo",
                "fixed",
                IssueStatus::Closed,
                Severity::Low,
                0,
            ),
            issue(
                1,
                "Release blocker",
                "waiting on registry",
                IssueStatus::Open,
                Severity::High,
                3,
            ),
        ],
        vec![
            note(3, NoteTarget::Task, 1, MemoryKind::Handoff, "resume here"),
            note(
                1,
                NoteTarget::Project,
                0,
                MemoryKind::Legacy,
                "legacy decision",
            ),
            note(
                2,
                NoteTarget::Plan,
                1,
                MemoryKind::Decision,
                "use dependency-free reports",
            ),
        ],
        vec![commit(1)],
    )
}
