use ptrack_core::{
    Board, Commit, Counts, Digest, Issue, IssueLine, IssueShow, Milestone, MilestoneRef,
    MilestoneShow, NextView, NoteLine, PlanRef, PlanShow, ProjectRef, SearchView, TaskLine,
    TaskShow, Timestamp,
};
use serde::Serialize;

#[derive(Serialize)]
#[allow(non_snake_case)]
pub struct MilestoneJson<'a> {
    ID: u64,
    Title: &'a str,
    Status: &'a str,
    Due: String,
    Order: i64,
    CreatedAt: String,
    UpdatedAt: String,
}

impl<'a> From<&'a Milestone> for MilestoneJson<'a> {
    fn from(value: &'a Milestone) -> Self {
        Self {
            ID: value.id,
            Title: &value.title,
            Status: value.status.as_str(),
            Due: timestamp(value.due),
            Order: value.order,
            CreatedAt: timestamp(value.created_at),
            UpdatedAt: timestamp(value.updated_at),
        }
    }
}

#[derive(Serialize)]
#[allow(non_snake_case)]
pub struct IssueJson<'a> {
    ID: u64,
    Title: &'a str,
    Body: &'a str,
    Status: &'a str,
    Severity: &'a str,
    TaskID: u64,
    CreatedAt: String,
    UpdatedAt: String,
}

impl<'a> From<&'a Issue> for IssueJson<'a> {
    fn from(value: &'a Issue) -> Self {
        Self {
            ID: value.id,
            Title: &value.title,
            Body: &value.body,
            Status: value.status.as_str(),
            Severity: value.severity.as_str(),
            TaskID: value.task_id,
            CreatedAt: timestamp(value.created_at),
            UpdatedAt: timestamp(value.updated_at),
        }
    }
}

#[derive(Serialize)]
#[allow(non_snake_case)]
pub struct CommitJson<'a> {
    ID: u64,
    SHA: &'a str,
    Subject: &'a str,
    PlanID: u64,
    TaskID: u64,
    CreatedAt: String,
}

impl<'a> From<&'a Commit> for CommitJson<'a> {
    fn from(value: &'a Commit) -> Self {
        Self {
            ID: value.id,
            SHA: &value.sha,
            Subject: &value.subject,
            PlanID: value.plan_id,
            TaskID: value.task_id,
            CreatedAt: timestamp(value.created_at),
        }
    }
}

#[derive(Serialize)]
#[allow(non_snake_case)]
pub struct ProjectJson<'a> {
    Name: &'a str,
    Path: &'a str,
    LastSeen: String,
}

impl<'a> From<&'a ProjectRef> for ProjectJson<'a> {
    fn from(value: &'a ProjectRef) -> Self {
        Self {
            Name: &value.name,
            Path: &value.path,
            LastSeen: timestamp(value.last_seen),
        }
    }
}

#[derive(Serialize)]
pub struct PlanRow<'a> {
    pub id: u64,
    pub title: &'a str,
    pub status: &'a str,
    pub active: bool,
    pub hold_reason: Option<&'a str>,
    /// Identity holding the hard claim on this plan; `None` when unclaimed.
    pub claimed_by: Option<&'a str>,
    /// Identity that last mutated this record; [`ptrack_core::LEGACY_ACTOR`]
    /// when unset.
    pub actor: &'a str,
}

#[derive(Serialize)]
pub struct TaskRow<'a> {
    pub id: u64,
    pub plan_id: u64,
    pub title: &'a str,
    pub status: &'a str,
    pub hold_reason: Option<&'a str>,
    /// Identity that last mutated this record; [`ptrack_core::LEGACY_ACTOR`]
    /// when unset.
    pub actor: &'a str,
}

#[derive(Serialize)]
pub struct NoteRow<'a> {
    pub id: u64,
    pub target: &'a str,
    pub target_id: u64,
    #[serde(skip_serializing_if = "str::is_empty")]
    pub kind: &'a str,
    pub body: &'a str,
}

#[derive(Serialize)]
pub struct StatusJson<'a> {
    pub goal: &'a str,
    pub active_plan: u64,
    pub active_plan_title: &'a str,
    pub plans: usize,
    pub todo: usize,
    pub doing: usize,
    pub done: usize,
    pub blocked: usize,
    /// Held tasks; orthogonal to the status totals above.
    pub on_hold: usize,
    /// Held plans; orthogonal to the plan total above.
    pub plans_on_hold: usize,
}

#[derive(Serialize)]
struct TaskLineJson<'a> {
    id: u64,
    plan_id: u64,
    title: &'a str,
    status: &'a str,
    hold_reason: Option<&'a str>,
}

impl<'a> From<&'a TaskLine> for TaskLineJson<'a> {
    fn from(value: &'a TaskLine) -> Self {
        Self {
            id: value.id,
            plan_id: value.plan_id,
            title: &value.title,
            status: &value.status,
            hold_reason: value.hold_reason.as_deref(),
        }
    }
}

#[derive(Serialize)]
struct NoteLineJson<'a> {
    id: u64,
    target: &'a str,
    target_id: u64,
    #[serde(skip_serializing_if = "str::is_empty")]
    kind: &'a str,
    body: &'a str,
}

impl<'a> From<&'a NoteLine> for NoteLineJson<'a> {
    fn from(value: &'a NoteLine) -> Self {
        Self {
            id: value.id,
            target: &value.target,
            target_id: value.target_id,
            kind: &value.kind,
            body: &value.body,
        }
    }
}

#[derive(Serialize)]
struct IssueLineJson<'a> {
    id: u64,
    title: &'a str,
    severity: &'a str,
    status: &'a str,
    task_id: u64,
}

impl<'a> From<&'a IssueLine> for IssueLineJson<'a> {
    fn from(value: &'a IssueLine) -> Self {
        Self {
            id: value.id,
            title: &value.title,
            severity: &value.severity,
            status: &value.status,
            task_id: value.task_id,
        }
    }
}

#[derive(Serialize)]
struct PlanRefJson<'a> {
    id: u64,
    title: &'a str,
    status: &'a str,
    hold_reason: Option<&'a str>,
    claimed_by: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    claimed_by_name: Option<&'a str>,
}

impl<'a> From<&'a PlanRef> for PlanRefJson<'a> {
    fn from(value: &'a PlanRef) -> Self {
        Self {
            id: value.id,
            title: &value.title,
            status: &value.status,
            hold_reason: value.hold_reason.as_deref(),
            claimed_by: value.claimed_by.as_deref(),
            claimed_by_name: value.claimed_by_name.as_deref(),
        }
    }
}

#[derive(Serialize)]
struct MilestoneRefJson<'a> {
    id: u64,
    title: &'a str,
    status: &'a str,
}

impl<'a> From<&'a MilestoneRef> for MilestoneRefJson<'a> {
    fn from(value: &'a MilestoneRef) -> Self {
        Self {
            id: value.id,
            title: &value.title,
            status: &value.status,
        }
    }
}

#[derive(Serialize)]
struct PlanBriefJson<'a> {
    id: u64,
    title: &'a str,
    open_tasks: Option<Vec<TaskLineJson<'a>>>,
    hold_reason: Option<&'a str>,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct CountsJson {
    Milestones: usize,
    MilestonesDone: usize,
    Plans: usize,
    PlansDone: usize,
    PlansOnHold: usize,
    Tasks: usize,
    TasksDone: usize,
    TasksBlocked: usize,
    TasksOpen: usize,
    TasksOnHold: usize,
    Issues: usize,
    IssuesOpen: usize,
    Commits: usize,
    Notes: usize,
}

impl From<Counts> for CountsJson {
    fn from(value: Counts) -> Self {
        Self {
            Milestones: value.milestones,
            MilestonesDone: value.milestones_done,
            Plans: value.plans,
            PlansDone: value.plans_done,
            PlansOnHold: value.plans_on_hold,
            Tasks: value.tasks,
            TasksDone: value.tasks_done,
            TasksBlocked: value.tasks_blocked,
            TasksOpen: value.tasks_open,
            TasksOnHold: value.tasks_on_hold,
            Issues: value.issues,
            IssuesOpen: value.issues_open,
            Commits: value.commits,
            Notes: value.notes,
        }
    }
}

#[derive(Serialize)]
pub struct DigestJson<'a> {
    goal: &'a str,
    summary: &'a str,
    active_plan: Option<PlanBriefJson<'a>>,
    blocked: Option<Vec<TaskLineJson<'a>>>,
    blocked_more: usize,
    on_hold: Option<Vec<TaskLineJson<'a>>>,
    on_hold_more: usize,
    open_issues: Option<Vec<IssueLineJson<'a>>>,
    open_issues_more: usize,
    recent_notes: Option<Vec<NoteLineJson<'a>>>,
    inventory: CountsJson,
}

impl<'a> From<&'a Digest> for DigestJson<'a> {
    fn from(value: &'a Digest) -> Self {
        Self {
            goal: &value.goal,
            summary: &value.summary,
            active_plan: value.active_plan.as_ref().map(|plan| PlanBriefJson {
                id: plan.id,
                title: &plan.title,
                open_tasks: nonempty(plan.open_tasks.iter().map(Into::into).collect()),
                hold_reason: plan.hold_reason.as_deref(),
            }),
            blocked: nonempty(value.blocked.iter().map(Into::into).collect()),
            blocked_more: value.blocked_more,
            on_hold: nonempty(value.on_hold.iter().map(Into::into).collect()),
            on_hold_more: value.on_hold_more,
            open_issues: nonempty(value.open_issues.iter().map(Into::into).collect()),
            open_issues_more: value.open_issues_more,
            recent_notes: nonempty(value.recent_notes.iter().map(Into::into).collect()),
            inventory: value.inventory.into(),
        }
    }
}

#[derive(Serialize)]
pub struct NextJson<'a> {
    task: Option<TaskLineJson<'a>>,
    #[serde(skip_serializing_if = "str::is_empty")]
    plan_title: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    message: &'a str,
    /// Present only when the active plan's hold is why no task was picked, so
    /// a consumer never parses the reason back out of `message`.
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_hold_reason: Option<&'a str>,
}

impl<'a> From<&'a NextView> for NextJson<'a> {
    fn from(value: &'a NextView) -> Self {
        Self {
            task: value.task.as_ref().map(Into::into),
            plan_title: &value.plan_title,
            message: &value.message,
            plan_hold_reason: value.plan_hold_reason.as_deref(),
        }
    }
}

#[derive(Serialize)]
pub struct PlanShowJson<'a> {
    plan: PlanRefJson<'a>,
    tasks: Option<Vec<TaskLineJson<'a>>>,
    notes: Option<Vec<NoteLineJson<'a>>>,
}

impl<'a> From<&'a PlanShow> for PlanShowJson<'a> {
    fn from(value: &'a PlanShow) -> Self {
        Self {
            plan: (&value.plan).into(),
            tasks: nonempty(value.tasks.iter().map(Into::into).collect()),
            notes: nonempty(value.notes.iter().map(Into::into).collect()),
        }
    }
}

#[derive(Serialize)]
pub struct TaskShowJson<'a> {
    task: TaskLineJson<'a>,
    plan: Option<PlanRefJson<'a>>,
    notes: Option<Vec<NoteLineJson<'a>>>,
}

impl<'a> From<&'a TaskShow> for TaskShowJson<'a> {
    fn from(value: &'a TaskShow) -> Self {
        Self {
            task: (&value.task).into(),
            plan: value.plan.as_ref().map(Into::into),
            notes: nonempty(value.notes.iter().map(Into::into).collect()),
        }
    }
}

#[derive(Serialize)]
pub struct MilestoneShowJson<'a> {
    id: u64,
    title: &'a str,
    status: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    due: &'a str,
    plans: Option<Vec<PlanRefJson<'a>>>,
    tasks_done: usize,
    tasks_open: usize,
}

impl<'a> From<&'a MilestoneShow> for MilestoneShowJson<'a> {
    fn from(value: &'a MilestoneShow) -> Self {
        Self {
            id: value.id,
            title: &value.title,
            status: &value.status,
            due: &value.due,
            plans: nonempty(value.plans.iter().map(Into::into).collect()),
            tasks_done: value.tasks_done,
            tasks_open: value.tasks_open,
        }
    }
}

#[derive(Serialize)]
pub struct IssueShowJson<'a> {
    id: u64,
    title: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    body: &'a str,
    status: &'a str,
    severity: &'a str,
    task: Option<TaskLineJson<'a>>,
}

impl<'a> From<&'a IssueShow> for IssueShowJson<'a> {
    fn from(value: &'a IssueShow) -> Self {
        Self {
            id: value.id,
            title: &value.title,
            body: &value.body,
            status: &value.status,
            severity: &value.severity,
            task: value.task.as_ref().map(Into::into),
        }
    }
}

#[derive(Serialize)]
pub struct BoardJson<'a> {
    plan_id: u64,
    plan_title: &'a str,
    todo: Option<Vec<TaskLineJson<'a>>>,
    doing: Option<Vec<TaskLineJson<'a>>>,
    blocked: Option<Vec<TaskLineJson<'a>>>,
    done: Option<Vec<TaskLineJson<'a>>>,
}

impl<'a> From<&'a Board> for BoardJson<'a> {
    fn from(value: &'a Board) -> Self {
        Self {
            plan_id: value.plan_id,
            plan_title: &value.plan_title,
            todo: nonempty(value.todo.iter().map(Into::into).collect()),
            doing: nonempty(value.doing.iter().map(Into::into).collect()),
            blocked: nonempty(value.blocked.iter().map(Into::into).collect()),
            done: nonempty(value.done.iter().map(Into::into).collect()),
        }
    }
}

#[derive(Serialize)]
pub struct SearchJson<'a> {
    term: &'a str,
    milestones: Option<Vec<MilestoneRefJson<'a>>>,
    plans: Option<Vec<PlanRefJson<'a>>>,
    tasks: Option<Vec<TaskLineJson<'a>>>,
    issues: Option<Vec<IssueLineJson<'a>>>,
    notes: Option<Vec<NoteLineJson<'a>>>,
}

impl<'a> From<&'a SearchView> for SearchJson<'a> {
    fn from(value: &'a SearchView) -> Self {
        Self {
            term: &value.term,
            milestones: nonempty(value.milestones.iter().map(Into::into).collect()),
            plans: nonempty(value.plans.iter().map(Into::into).collect()),
            tasks: nonempty(value.tasks.iter().map(Into::into).collect()),
            issues: nonempty(value.issues.iter().map(Into::into).collect()),
            notes: nonempty(value.notes.iter().map(Into::into).collect()),
        }
    }
}

#[derive(Serialize)]
pub struct ConfigUserJson<'a> {
    pub id: Option<&'a str>,
    pub name: Option<&'a str>,
}

impl<'a> From<Option<&'a ptrack_app::ActorIdentity>> for ConfigUserJson<'a> {
    fn from(value: Option<&'a ptrack_app::ActorIdentity>) -> Self {
        Self {
            id: value.map(|identity| identity.id.as_str()),
            name: value.map(|identity| identity.name.as_str()),
        }
    }
}

pub fn raw_or_null<T>(values: Vec<T>) -> Option<Vec<T>> {
    nonempty(values)
}

fn nonempty<T>(values: Vec<T>) -> Option<Vec<T>> {
    (!values.is_empty()).then_some(values)
}

#[must_use]
pub fn timestamp(value: Timestamp) -> String {
    let Timestamp::Fixed {
        seconds,
        nanoseconds,
        offset_seconds,
    } = value
    else {
        return "0001-01-01T00:00:00Z".to_owned();
    };
    let local_seconds = i128::from(seconds) + i128::from(offset_seconds);
    let date = value.stored_date().expect("fixed timestamp has date");
    let seconds_of_day = local_seconds.rem_euclid(86_400);
    let hour = seconds_of_day / 3_600;
    let minute = seconds_of_day % 3_600 / 60;
    let second = seconds_of_day % 60;
    let fraction = if nanoseconds == 0 {
        String::new()
    } else {
        format!(".{nanoseconds:09}")
            .trim_end_matches('0')
            .to_owned()
    };
    let zone = if offset_seconds == 0 {
        "Z".to_owned()
    } else {
        let sign = if offset_seconds < 0 { '-' } else { '+' };
        let absolute = offset_seconds.unsigned_abs();
        format!("{sign}{:02}:{:02}", absolute / 3_600, absolute % 3_600 / 60)
    };
    format!("{date}T{hour:02}:{minute:02}:{second:02}{fraction}{zone}")
}
