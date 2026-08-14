use std::fmt;
use std::fmt::Write as _;

use crate::{Counts, Issue, Note, ProjectSnapshot, Task, TaskStatus};

const CONTEXT_RECENT_NOTES: usize = 5;
const CONTEXT_BLOCKED_SHOWN: usize = 8;
const CONTEXT_ISSUES_SHOWN: usize = 8;

/// A report query could not find its required root entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReportError {
    NotFound { entity: &'static str, id: u64 },
}

impl ReportError {
    pub(crate) const fn not_found(entity: &'static str, id: u64) -> Self {
        Self::NotFound { entity, id }
    }

    /// Returns the missing entity name.
    #[must_use]
    pub const fn entity(self) -> &'static str {
        match self {
            Self::NotFound { entity, .. } => entity,
        }
    }

    /// Returns the missing persistent ID.
    #[must_use]
    pub const fn id(self) -> u64 {
        match self {
            Self::NotFound { id, .. } => id,
        }
    }
}

impl fmt::Display for ReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { entity, id } => write!(formatter, "{entity} #{id} not found"),
        }
    }
}

impl std::error::Error for ReportError {}

/// The bounded cold-start restore view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Digest {
    pub goal: String,
    pub summary: String,
    pub active_plan: Option<PlanBrief>,
    pub blocked: Vec<TaskLine>,
    pub blocked_more: usize,
    pub open_issues: Vec<IssueLine>,
    pub open_issues_more: usize,
    pub recent_notes: Vec<NoteLine>,
    pub inventory: Counts,
}

/// A compact issue reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueLine {
    pub id: u64,
    pub title: String,
    pub severity: String,
    pub status: String,
    pub task_id: u64,
}

/// An active plan plus its open tasks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanBrief {
    pub id: u64,
    pub title: String,
    pub open_tasks: Vec<TaskLine>,
}

/// A compact task reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskLine {
    pub id: u64,
    pub plan_id: u64,
    pub title: String,
    pub status: String,
}

/// A compact note reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteLine {
    pub id: u64,
    pub target: String,
    pub target_id: u64,
    /// Empty for legacy notes and omitted by JSON adapters.
    pub kind: String,
    pub body: String,
}

/// Assembles the bounded restore digest from one consistent project snapshot.
#[must_use]
pub fn context(snapshot: &ProjectSnapshot) -> Digest {
    let active_plan = if snapshot.meta.active_plan == 0 {
        None
    } else {
        snapshot
            .plan(snapshot.meta.active_plan)
            .map(|plan| PlanBrief {
                id: plan.id,
                title: plan.title.clone(),
                open_tasks: snapshot
                    .tasks_for_plan(plan.id)
                    .filter(|task| task.status.is_open())
                    .map(task_line)
                    .collect(),
            })
    };

    let mut blocked = Vec::new();
    let mut blocked_more = 0;
    for task in snapshot
        .tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Blocked)
    {
        if blocked.len() < CONTEXT_BLOCKED_SHOWN {
            blocked.push(task_line(task));
        } else {
            blocked_more += 1;
        }
    }

    let mut open_issues = Vec::new();
    let mut open_issues_more = 0;
    for issue in snapshot
        .issues
        .iter()
        .filter(|issue| issue.status == crate::IssueStatus::Open)
    {
        if open_issues.len() < CONTEXT_ISSUES_SHOWN {
            open_issues.push(issue_line(issue));
        } else {
            open_issues_more += 1;
        }
    }

    Digest {
        goal: snapshot.meta.goal.clone(),
        summary: snapshot.meta.summary.clone(),
        active_plan,
        blocked,
        blocked_more,
        open_issues,
        open_issues_more,
        recent_notes: snapshot
            .recent_notes(CONTEXT_RECENT_NOTES)
            .into_iter()
            .map(note_line)
            .collect(),
        inventory: snapshot.counts(),
    }
}

impl Digest {
    /// Renders the exact Go-compatible context Markdown.
    #[must_use]
    pub fn markdown(&self) -> String {
        let mut output = String::from("# ptrack context\n\n");

        output.push_str("## Goal\n");
        output.push_str(or_dash(&self.goal));
        output.push_str("\n\n## Summary\n");
        output.push_str(or_dash(&self.summary));
        output.push_str("\n\n## Active plan\n");
        write_active_plan(&mut output, self.active_plan.as_ref());
        write_blocked(&mut output, &self.blocked, self.blocked_more);
        write_open_issues(&mut output, &self.open_issues, self.open_issues_more);
        write_recent_notes(&mut output, &self.recent_notes);
        write_inventory(&mut output, self.inventory);
        output
    }
}

fn write_active_plan(output: &mut String, plan: Option<&PlanBrief>) {
    let Some(plan) = plan else {
        output.push_str("_none_\n\n");
        return;
    };
    writeln!(output, "**#{} {}**\n", plan.id, plan.title).expect("writing to String cannot fail");
    output.push_str("### Open tasks\n");
    if plan.open_tasks.is_empty() {
        output.push_str("_none_\n");
    } else {
        for task in &plan.open_tasks {
            writeln!(output, "- [{}] #{} {}", task.status, task.id, task.title)
                .expect("writing to String cannot fail");
        }
    }
    output.push('\n');
}

fn write_blocked(output: &mut String, tasks: &[TaskLine], more: usize) {
    if tasks.is_empty() {
        return;
    }
    output.push_str("## Blocked (project-wide)\n");
    for task in tasks {
        writeln!(
            output,
            "- #{} {} (plan {})",
            task.id, task.title, task.plan_id
        )
        .expect("writing to String cannot fail");
    }
    if more > 0 {
        writeln!(
            output,
            "- … +{more} more (use `ptrack task list --status blocked`)"
        )
        .expect("writing to String cannot fail");
    }
    output.push('\n');
}

fn write_open_issues(output: &mut String, issues: &[IssueLine], more: usize) {
    if issues.is_empty() {
        return;
    }
    output.push_str("## Open issues\n");
    for issue in issues {
        if issue.task_id == 0 {
            writeln!(
                output,
                "- #{} [{}] {}",
                issue.id, issue.severity, issue.title
            )
            .expect("writing to String cannot fail");
        } else {
            writeln!(
                output,
                "- #{} [{}] {} (task {})",
                issue.id, issue.severity, issue.title, issue.task_id
            )
            .expect("writing to String cannot fail");
        }
    }
    if more > 0 {
        writeln!(output, "- … +{more} more (use `ptrack issue list`)")
            .expect("writing to String cannot fail");
    }
    output.push('\n');
}

fn write_recent_notes(output: &mut String, notes: &[NoteLine]) {
    output.push_str("## Recent decisions\n");
    if notes.is_empty() {
        output.push_str("_none_\n");
    } else {
        for note in notes {
            output.push_str("- ");
            output.push_str(&note_markdown(note));
            output.push('\n');
        }
    }
}

fn write_inventory(output: &mut String, counts: Counts) {
    output.push_str("\n## Inventory\n");
    writeln!(
        output,
        "{} milestones ({} done) · {} plans ({} done) · {} tasks ({} done · {} blocked · {} open) · {} issues ({} open) · {} notes\n",
        counts.milestones,
        counts.milestones_done,
        counts.plans,
        counts.plans_done,
        counts.tasks,
        counts.tasks_done,
        counts.tasks_blocked,
        counts.tasks_open,
        counts.issues,
        counts.issues_open,
        counts.notes
    )
    .expect("writing to String cannot fail");
    output.push_str(
        "Drill deeper: `ptrack next` · `ptrack milestone list` · `ptrack plan show <id>` · \
         `ptrack task show <id>` · `ptrack task list --status doing,blocked` · `ptrack issue list` · \
         `ptrack note list` · `ptrack search <term>` · `ptrack board`\n",
    );
}

pub(crate) fn issue_line(issue: &Issue) -> IssueLine {
    IssueLine {
        id: issue.id,
        title: issue.title.clone(),
        severity: issue.severity.as_str().to_owned(),
        status: issue.status.as_str().to_owned(),
        task_id: issue.task_id,
    }
}

pub(crate) fn task_line(task: &Task) -> TaskLine {
    TaskLine {
        id: task.id,
        plan_id: task.plan_id,
        title: task.title.clone(),
        status: task.status.as_str().to_owned(),
    }
}

pub(crate) fn note_line(note: &Note) -> NoteLine {
    NoteLine {
        id: note.id,
        target: note.target.as_str().to_owned(),
        target_id: note.target_id,
        kind: note.kind.as_str().to_owned(),
        body: note.body.clone(),
    }
}

pub(crate) fn note_markdown(note: &NoteLine) -> String {
    let kind = if note.kind.is_empty() {
        String::new()
    } else {
        format!("[{}] ", note.kind)
    };
    if note.target_id == 0 {
        format!("{kind}({}) {}", note.target, note.body)
    } else {
        format!("{kind}({} #{}) {}", note.target, note.target_id, note.body)
    }
}

pub(crate) fn notes_markdown(notes: &[NoteLine]) -> String {
    if notes.is_empty() {
        return "_none_\n".to_owned();
    }
    let mut output = String::new();
    for note in notes {
        output.push_str("- ");
        output.push_str(&note_markdown(note));
        output.push('\n');
    }
    output
}

fn or_dash(value: &str) -> &str {
    if value.trim().is_empty() {
        "_(unset)_"
    } else {
        value
    }
}
