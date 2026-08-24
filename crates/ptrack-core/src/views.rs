use std::fmt::Write as _;

use crate::report::{
    ReportError, claim_marker, hold_marker, id_list, note_line, notes_markdown, open_plan_deps,
    open_task_deps, task_line,
};
use crate::{NoteLine, ProjectSnapshot, TaskLine, TaskStatus};

/// A compact plan reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanRef {
    pub id: u64,
    pub title: String,
    pub status: String,
    /// Set while the plan is on hold; orthogonal to its status.
    pub hold_reason: Option<String>,
    /// Identity holding the hard claim on this plan; `None` when unclaimed.
    pub claimed_by: Option<String>,
    /// Resolved display name for `claimed_by`; `None` when the directory has
    /// no entry for that identity.
    pub claimed_by_name: Option<String>,
}

/// The single most actionable task, or an explanation of its absence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NextView {
    /// The project's north-star goal; empty when unset.
    pub goal: String,
    pub task: Option<TaskLine>,
    /// Empty when omitted by JSON adapters.
    pub plan_title: String,
    /// Empty when omitted by JSON adapters.
    pub message: String,
    /// Set when the active plan's own hold is why no task was picked, so a
    /// consumer never has to parse it back out of `message`.
    pub plan_hold_reason: Option<String>,
    /// Open plan IDs the active plan waits on; nonempty when those deps are
    /// why no task was picked.
    pub plan_waiting_on: Vec<u64>,
    /// Candidates passed over only because their task-deps are still open.
    pub skipped: Vec<DepSkip>,
}

/// A candidate task passed over because its dependencies are still open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DepSkip {
    pub task_id: u64,
    /// The open dependency IDs blocking the task, in stored order.
    pub waiting_on: Vec<u64>,
}

/// Returns the first doing task in the active plan, then the first todo task.
///
/// Held tasks are never selected, and a held active plan short-circuits with a
/// message naming the reason instead of picking any task at all. Open
/// dependencies work the same way, computed rather than stored: a plan waiting
/// on open plan-deps short-circuits, and a task waiting on open task-deps is
/// passed over and reported in `skipped`.
///
/// # Errors
///
/// Returns [`ReportError::NotFound`] when the active-plan pointer is nonzero
/// but its plan is missing.
pub fn next(snapshot: &ProjectSnapshot) -> Result<NextView, ReportError> {
    let goal = snapshot.meta.goal.clone();
    if snapshot.meta.active_plan == 0 {
        return Ok(NextView {
            goal,
            task: None,
            plan_title: String::new(),
            message: "no active plan (set one with 'ptrack plan use <id>')".to_owned(),
            plan_hold_reason: None,
            plan_waiting_on: Vec::new(),
            skipped: Vec::new(),
        });
    }
    let plan = snapshot
        .plan(snapshot.meta.active_plan)
        .ok_or_else(|| ReportError::not_found("plan", snapshot.meta.active_plan))?;
    if let Some(reason) = &plan.hold_reason {
        return Ok(NextView {
            goal,
            task: None,
            plan_title: plan.title.clone(),
            message: format!("active plan on hold: {reason}"),
            plan_hold_reason: Some(reason.clone()),
            plan_waiting_on: Vec::new(),
            skipped: Vec::new(),
        });
    }
    let plan_waiting_on = open_plan_deps(snapshot, plan);
    if !plan_waiting_on.is_empty() {
        return Ok(NextView {
            goal,
            task: None,
            plan_title: plan.title.clone(),
            message: format!("active plan waiting on {}", id_list(&plan_waiting_on)),
            plan_hold_reason: None,
            plan_waiting_on,
            skipped: Vec::new(),
        });
    }
    let mut tasks = Vec::new();
    let mut skipped = Vec::new();
    for task in snapshot
        .tasks_for_plan(plan.id)
        .filter(|task| task.hold_reason.is_none())
    {
        let waiting_on = open_task_deps(snapshot, task);
        if waiting_on.is_empty() {
            tasks.push(task);
        } else if matches!(task.status, TaskStatus::Doing | TaskStatus::Todo) {
            skipped.push(DepSkip {
                task_id: task.id,
                waiting_on,
            });
        }
    }
    let selected = tasks
        .iter()
        .find(|task| task.status == TaskStatus::Doing)
        .or_else(|| tasks.iter().find(|task| task.status == TaskStatus::Todo));
    if let Some(task) = selected {
        return Ok(NextView {
            goal,
            task: Some(task_line(task)),
            plan_title: plan.title.clone(),
            message: String::new(),
            plan_hold_reason: None,
            plan_waiting_on: Vec::new(),
            skipped,
        });
    }
    Ok(NextView {
        goal,
        task: None,
        plan_title: plan.title.clone(),
        message: "no actionable task in the active plan".to_owned(),
        plan_hold_reason: None,
        plan_waiting_on: Vec::new(),
        skipped,
    })
}

impl NextView {
    /// Renders the exact Go-compatible next-task Markdown, plus one
    /// `skipped:` line per dep-blocked candidate.
    #[must_use]
    pub fn markdown(&self) -> String {
        let mut output = goal_line(&self.goal);
        match &self.task {
            Some(task) => writeln!(
                &mut output,
                "next: [{}] #{} {} (plan: {})",
                task.status, task.id, task.title, self.plan_title
            )
            .expect("writing to String cannot fail"),
            None => {
                writeln!(&mut output, "{}", self.message).expect("writing to String cannot fail")
            }
        }
        for skip in &self.skipped {
            writeln!(
                &mut output,
                "skipped: #{} (waiting on {})",
                skip.task_id,
                id_list(&skip.waiting_on)
            )
            .expect("writing to String cannot fail");
        }
        output
    }
}

/// A single plan with its tasks and notes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanShow {
    pub plan: PlanRef,
    pub tasks: Vec<TaskLine>,
    pub notes: Vec<NoteLine>,
}

/// Assembles a full view of one plan.
///
/// # Errors
///
/// Returns [`ReportError::NotFound`] when the plan does not exist.
pub fn show_plan(snapshot: &ProjectSnapshot, id: u64) -> Result<PlanShow, ReportError> {
    let plan = snapshot
        .plan(id)
        .ok_or_else(|| ReportError::not_found("plan", id))?;
    Ok(PlanShow {
        plan: plan_ref(&snapshot.meta, plan),
        tasks: snapshot.tasks_for_plan(id).map(task_line).collect(),
        notes: snapshot.notes_for_plan(id).map(note_line).collect(),
    })
}

impl PlanShow {
    /// Renders the exact Go-compatible plan Markdown.
    #[must_use]
    pub fn markdown(&self) -> String {
        let mut output = format!(
            "# Plan #{} {} [{}]{}{}\n\n## Tasks\n",
            self.plan.id,
            self.plan.title,
            self.plan.status,
            hold_marker(self.plan.hold_reason.as_deref()),
            claim_marker(
                self.plan
                    .claimed_by_name
                    .as_deref()
                    .or(self.plan.claimed_by.as_deref())
            )
        );
        if self.tasks.is_empty() {
            output.push_str("_none_\n");
        } else {
            for task in &self.tasks {
                writeln!(
                    &mut output,
                    "- [{}] #{} {}{}",
                    task.status,
                    task.id,
                    task.title,
                    hold_marker(task.hold_reason.as_deref())
                )
                .expect("writing to String cannot fail");
            }
        }
        output.push_str("\n## Notes\n");
        output.push_str(&notes_markdown(&self.notes));
        output
    }
}

/// A single task with its best-effort parent plan and notes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskShow {
    /// The project's north-star goal; empty when unset.
    pub goal: String,
    pub task: TaskLine,
    pub plan: Option<PlanRef>,
    pub notes: Vec<NoteLine>,
}

/// Assembles a full view of one task.
///
/// A missing parent plan is tolerated and represented as `None`, matching the
/// Go report service's best-effort reference resolution.
///
/// # Errors
///
/// Returns [`ReportError::NotFound`] when the requested task does not exist.
pub fn show_task(snapshot: &ProjectSnapshot, id: u64) -> Result<TaskShow, ReportError> {
    let task = snapshot
        .task(id)
        .ok_or_else(|| ReportError::not_found("task", id))?;
    Ok(TaskShow {
        goal: snapshot.meta.goal.clone(),
        task: task_line(task),
        plan: snapshot
            .plan(task.plan_id)
            .map(|plan| plan_ref(&snapshot.meta, plan)),
        notes: snapshot.notes_for_task(id).map(note_line).collect(),
    })
}

impl TaskShow {
    /// Renders the task Markdown, led by the goal line so every task view
    /// re-anchors the reader to the north star.
    #[must_use]
    pub fn markdown(&self) -> String {
        let mut output = goal_line(&self.goal);
        write!(
            &mut output,
            "# Task #{} {} [{}]{}\n\n",
            self.task.id,
            self.task.title,
            self.task.status,
            hold_marker(self.task.hold_reason.as_deref())
        )
        .expect("writing to String cannot fail");
        if let Some(plan) = &self.plan {
            writeln!(
                &mut output,
                "Plan: #{} {}{}{}\n",
                plan.id,
                plan.title,
                hold_marker(plan.hold_reason.as_deref()),
                claim_marker(
                    plan.claimed_by_name
                        .as_deref()
                        .or(plan.claimed_by.as_deref())
                )
            )
            .expect("writing to String cannot fail");
        }
        output.push_str("## Notes\n");
        output.push_str(&notes_markdown(&self.notes));
        output
    }
}

/// A compact milestone reference used by search results.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneRef {
    pub id: u64,
    pub title: String,
    pub status: String,
}

/// A milestone with its plans and task rollup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneShow {
    pub id: u64,
    pub title: String,
    pub status: String,
    /// Empty for a zero due timestamp and omitted by JSON adapters.
    pub due: String,
    pub plans: Vec<PlanRef>,
    pub tasks_done: usize,
    pub tasks_open: usize,
}

/// Assembles a full view of one milestone.
///
/// # Errors
///
/// Returns [`ReportError::NotFound`] when the milestone does not exist.
pub fn show_milestone(snapshot: &ProjectSnapshot, id: u64) -> Result<MilestoneShow, ReportError> {
    let milestone = snapshot
        .milestone(id)
        .ok_or_else(|| ReportError::not_found("milestone", id))?;
    let plans: Vec<_> = snapshot.plans_for_milestone(id).collect();
    let mut tasks_done = 0;
    let mut tasks_open = 0;
    for plan in &plans {
        for task in snapshot.tasks_for_plan(plan.id) {
            if task.status == TaskStatus::Done {
                tasks_done += 1;
            } else {
                tasks_open += 1;
            }
        }
    }
    Ok(MilestoneShow {
        id: milestone.id,
        title: milestone.title.clone(),
        status: milestone.status.as_str().to_owned(),
        due: milestone
            .due
            .stored_date()
            .map(|date| date.to_string())
            .unwrap_or_default(),
        plans: plans
            .into_iter()
            .map(|plan| plan_ref(&snapshot.meta, plan))
            .collect(),
        tasks_done,
        tasks_open,
    })
}

impl MilestoneShow {
    /// Renders the exact Go-compatible milestone Markdown.
    #[must_use]
    pub fn markdown(&self) -> String {
        let due = if self.due.is_empty() {
            String::new()
        } else {
            format!(" (due {})", self.due)
        };
        let mut output = format!(
            "# Milestone #{} {} [{}]{}\n\nTasks: {} done · {} open\n\n## Plans\n",
            self.id, self.title, self.status, due, self.tasks_done, self.tasks_open
        );
        if self.plans.is_empty() {
            output.push_str("_none_\n");
            return output;
        }
        for plan in &self.plans {
            writeln!(
                &mut output,
                "- #{} {} [{}]{}{}",
                plan.id,
                plan.title,
                plan.status,
                hold_marker(plan.hold_reason.as_deref()),
                claim_marker(
                    plan.claimed_by_name
                        .as_deref()
                        .or(plan.claimed_by.as_deref())
                )
            )
            .expect("writing to String cannot fail");
        }
        output
    }
}

/// A single issue with its best-effort linked task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueShow {
    pub id: u64,
    pub title: String,
    /// Empty when omitted by JSON adapters.
    pub body: String,
    pub status: String,
    pub severity: String,
    pub task: Option<TaskLine>,
}

/// Assembles a full view of one issue.
///
/// A missing linked task is tolerated and represented as `None`.
///
/// # Errors
///
/// Returns [`ReportError::NotFound`] when the requested issue does not exist.
pub fn show_issue(snapshot: &ProjectSnapshot, id: u64) -> Result<IssueShow, ReportError> {
    let issue = snapshot
        .issue(id)
        .ok_or_else(|| ReportError::not_found("issue", id))?;
    Ok(IssueShow {
        id: issue.id,
        title: issue.title.clone(),
        body: issue.body.clone(),
        status: issue.status.as_str().to_owned(),
        severity: issue.severity.as_str().to_owned(),
        task: if issue.task_id == 0 {
            None
        } else {
            snapshot.task(issue.task_id).map(task_line)
        },
    })
}

impl IssueShow {
    /// Renders the exact Go-compatible issue Markdown.
    #[must_use]
    pub fn markdown(&self) -> String {
        let mut output = format!(
            "# Issue #{} {}\n\nStatus: {} · Severity: {}\n",
            self.id, self.title, self.status, self.severity
        );
        if let Some(task) = &self.task {
            writeln!(&mut output, "Task: #{} {}", task.id, task.title)
                .expect("writing to String cannot fail");
        }
        if !self.body.trim().is_empty() {
            output.push('\n');
            output.push_str(&self.body);
            output.push('\n');
        }
        output
    }
}

/// A plan's tasks grouped into kanban columns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Board {
    pub plan_id: u64,
    pub plan_title: String,
    pub todo: Vec<TaskLine>,
    pub doing: Vec<TaskLine>,
    pub blocked: Vec<TaskLine>,
    pub done: Vec<TaskLine>,
}

/// Assembles the kanban board for one plan.
///
/// # Errors
///
/// Returns [`ReportError::NotFound`] when the plan does not exist.
pub fn board_for(snapshot: &ProjectSnapshot, plan_id: u64) -> Result<Board, ReportError> {
    let plan = snapshot
        .plan(plan_id)
        .ok_or_else(|| ReportError::not_found("plan", plan_id))?;
    let mut board = Board {
        plan_id: plan.id,
        plan_title: plan.title.clone(),
        todo: Vec::new(),
        doing: Vec::new(),
        blocked: Vec::new(),
        done: Vec::new(),
    };
    for task in snapshot.tasks_for_plan(plan_id) {
        let line = task_line(task);
        match task.status {
            TaskStatus::Todo => board.todo.push(line),
            TaskStatus::Doing => board.doing.push(line),
            TaskStatus::Blocked => board.blocked.push(line),
            TaskStatus::Done => board.done.push(line),
        }
    }
    Ok(board)
}

impl Board {
    /// Renders the exact Go-compatible board Markdown.
    #[must_use]
    pub fn markdown(&self) -> String {
        let mut output = format!("# Board — #{} {}\n\n", self.plan_id, self.plan_title);
        for (name, tasks) in [
            ("Todo", &self.todo),
            ("Doing", &self.doing),
            ("Blocked", &self.blocked),
            ("Done", &self.done),
        ] {
            writeln!(&mut output, "## {} ({})", name, tasks.len())
                .expect("writing to String cannot fail");
            if tasks.is_empty() {
                output.push_str("_none_\n\n");
            } else {
                for task in tasks {
                    writeln!(
                        &mut output,
                        "- #{} {}{}",
                        task.id,
                        task.title,
                        hold_marker(task.hold_reason.as_deref())
                    )
                    .expect("writing to String cannot fail");
                }
                output.push('\n');
            }
        }
        output
    }
}

/// Renders the leading `Goal:` line shared by decision-point views, or an
/// empty string when no goal is set.
fn goal_line(goal: &str) -> String {
    if goal.is_empty() {
        String::new()
    } else {
        format!("Goal: {goal}\n")
    }
}

/// Milestone progress attached to a checkpoint when the closed plan has one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneProgress {
    pub title: String,
    pub plans_done: usize,
    pub plans_total: usize,
}

/// The whole-picture re-evaluation block printed after `plan done` and by
/// `ptrack checkpoint`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointView {
    /// Empty when no goal is set.
    pub goal: String,
    /// Empty when no rolling summary is set.
    pub summary: String,
    /// Open (active) plans as `(id, title)`, in display order.
    pub open_plans: Vec<(u64, String)>,
    pub open_issues: usize,
    /// Open issues at high or critical severity.
    pub high_issues: usize,
    /// Present when the checkpoint was computed for a plan in a milestone.
    pub milestone: Option<MilestoneProgress>,
}

/// Assembles the checkpoint block. `plan_id` names the plan just closed (or
/// under review) so its milestone progress can be reported; `None` renders the
/// project-wide picture only.
#[must_use]
pub fn checkpoint(snapshot: &ProjectSnapshot, plan_id: Option<u64>) -> CheckpointView {
    let open_plans = snapshot
        .plans
        .iter()
        .filter(|plan| plan.status == crate::PlanStatus::Active)
        .map(|plan| (plan.id, plan.title.clone()))
        .collect();
    let open_issues: Vec<_> = snapshot
        .issues
        .iter()
        .filter(|issue| issue.status == crate::IssueStatus::Open)
        .collect();
    let high_issues = open_issues
        .iter()
        .filter(|issue| {
            matches!(
                issue.severity,
                crate::Severity::High | crate::Severity::Critical
            )
        })
        .count();
    let milestone = plan_id
        .and_then(|id| snapshot.plan(id))
        .filter(|plan| plan.milestone_id != 0)
        .and_then(|plan| {
            let milestone = snapshot.milestone(plan.milestone_id)?;
            let plans: Vec<_> = snapshot.plans_for_milestone(milestone.id).collect();
            Some(MilestoneProgress {
                title: milestone.title.clone(),
                plans_done: plans
                    .iter()
                    .filter(|plan| plan.status == crate::PlanStatus::Done)
                    .count(),
                plans_total: plans.len(),
            })
        });
    CheckpointView {
        goal: snapshot.meta.goal.clone(),
        summary: snapshot.meta.summary.clone(),
        open_plans,
        open_issues: open_issues.len(),
        high_issues,
        milestone,
    }
}

impl CheckpointView {
    /// Renders the checkpoint Markdown, ending with the re-evaluation prompt.
    #[must_use]
    pub fn markdown(&self) -> String {
        let mut output = if self.goal.is_empty() {
            "Goal: (not set — set one with 'ptrack goal set \"...\"')\n".to_owned()
        } else {
            format!("Goal: {}\n", self.goal)
        };
        writeln!(
            &mut output,
            "Rolling summary: {}",
            if self.summary.is_empty() {
                "(not set)"
            } else {
                &self.summary
            }
        )
        .expect("writing to String cannot fail");
        let plans = if self.open_plans.is_empty() {
            "none".to_owned()
        } else {
            self.open_plans
                .iter()
                .map(|(id, title)| format!("#{id} {title}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        writeln!(&mut output, "Remaining open plans: {plans}")
            .expect("writing to String cannot fail");
        writeln!(
            &mut output,
            "Open issues: {} ({} high)",
            self.open_issues, self.high_issues
        )
        .expect("writing to String cannot fail");
        if let Some(milestone) = &self.milestone {
            writeln!(
                &mut output,
                "Milestone: {} — {}/{} plans done",
                milestone.title, milestone.plans_done, milestone.plans_total
            )
            .expect("writing to String cannot fail");
        }
        output.push_str(
            "\nCHECKPOINT — before continuing, re-evaluate:\n\
             - Does the remaining roadmap still reach the goal? Missing plans? Obsolete ones?\n\
             - What did this plan change that the next plans must know?\n\
             - Update: ptrack summary set \"...\" | ptrack plan add \"...\" | ptrack issue add \"...\"\n",
        );
        output
    }
}

pub(crate) fn plan_ref(meta: &crate::Meta, plan: &crate::Plan) -> PlanRef {
    PlanRef {
        id: plan.id,
        title: plan.title.clone(),
        status: plan.status.as_str().to_owned(),
        hold_reason: plan.hold_reason.clone(),
        claimed_by: plan.claim_owner.clone(),
        claimed_by_name: plan
            .claim_owner
            .as_deref()
            .and_then(|owner| meta.actor_name(owner))
            .map(str::to_owned),
    }
}
