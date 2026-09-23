//! Read projections: the board, task and issue views, linked-runtime
//! summaries, project storage, and the git snapshot capture the workspace
//! snapshot is assembled from.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::thread;
use std::time::Instant;

use ptrack_agent::{AgentRuntimeSummary, RegistrationKind, RuntimeAssociation};
use ptrack_core::{
    Commit, Issue, IssueStatus, MemoryKind, Meta, Note, NoteTarget, ProjectSnapshot, Task,
    TaskStatus, open_plan_deps, open_task_deps,
};
use ptrack_store::ProjectStore;
use ptrack_terminal::{SessionInfo, SessionState};
use serde::Serialize;
use serde_json::{Value, json};

use super::support::{bound, timestamp};
use super::{SNAPSHOT_PLAN_LIMIT, SNAPSHOT_TASK_LIMIT};
use crate::{AppError, AppResult};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PlanSummaryView {
    pub(super) id: u64,
    pub(super) title: String,
    pub(super) status: String,
    pub(super) is_active: bool,
    pub(super) tasks_total: usize,
    pub(super) tasks_done: usize,
    /// Present only while the plan is on hold; the frontend renders it as a
    /// marker on the existing row rather than a separate grouping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) hold_reason: Option<String>,
    /// Present only while the plan is claimed; carries the resolved display
    /// label (actor name, else the raw identity ID). Display-only — claims
    /// are mutated through the CLI only, exactly like holds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) claimed_by: Option<String>,
    /// Plan-dep IDs still open, computed snapshot-side so the frontend never
    /// re-derives openness. Empty (and omitted) when nothing blocks the plan.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) deps_open: Vec<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskView {
    pub(super) id: u64,
    pub(super) title: String,
    pub(super) status: String,
    pub(super) updated_at: String,
    pub(super) note_count: usize,
    pub(super) commit_count: usize,
    pub(super) issue_count: usize,
    pub(super) latest_note: String,
    /// Present only while the task is on hold. Hold is orthogonal to status, so
    /// the card stays in its status column and only gains a badge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) hold_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) linked_runtime: Option<TaskLinkedRuntimeSummaryView>,
    /// All declared task-dep IDs, in stored order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) deps: Vec<u64>,
    /// The subset of `deps` still open, computed snapshot-side so the
    /// frontend never re-derives openness. Deps are orthogonal to status:
    /// the card keeps its column and only gains a badge.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) deps_open: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskLinkedRuntimeSummaryView {
    pub(super) terminals: usize,
    pub(super) live_terminals: usize,
    pub(super) agents: usize,
    pub(super) live_agents: usize,
    pub(super) terminal_backed_runs: usize,
    pub(super) external_runs: usize,
    pub(super) truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TerminalRuntimeSummaryView {
    pub(super) session_id: String,
    pub(super) profile_kind: ptrack_terminal::ProfileKind,
    pub(super) state: SessionState,
    pub(super) live: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) association: Option<RuntimeAssociation>,
}

pub(super) struct RuntimeProjectionView {
    pub(super) terminals: Vec<TerminalRuntimeSummaryView>,
    pub(super) terminal_total: usize,
    pub(super) agents: Vec<AgentRuntimeSummary>,
    pub(super) agent_total: usize,
    pub(super) sources_truncated: bool,
}

pub(super) struct SnapshotTrackingBounds {
    pub(super) plans: usize,
    pub(super) tasks: usize,
    pub(super) blockers: usize,
    pub(super) notes: usize,
    pub(super) activity: usize,
    pub(super) issues: usize,
}

pub(super) struct SnapshotTrackingCapture {
    pub(super) snapshot: ProjectSnapshot,
    pub(super) board: BoardView,
    pub(super) blockers: Vec<TaskView>,
    pub(super) bounds: SnapshotTrackingBounds,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskLinkedRuntimeDetailView {
    pub(super) summary: TaskLinkedRuntimeSummaryView,
    pub(super) terminals: Vec<TerminalRuntimeSummaryView>,
    pub(super) agents: Vec<AgentRuntimeSummary>,
    pub(super) terminal_rows_more: usize,
    pub(super) agent_rows_more: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct ColumnView {
    pub(super) status: String,
    pub(super) title: String,
    pub(super) tasks: Vec<TaskView>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectStatsView {
    pub(super) plan_tasks: usize,
    pub(super) plan_tasks_done: usize,
    pub(super) tasks_open: usize,
    pub(super) tasks_blocked: usize,
    pub(super) notes: usize,
    pub(super) commits: usize,
    pub(super) open_issues: usize,
    /// Project-wide totals: the Overview renders these regardless of the
    /// selected plan, while `plan_tasks*` stays scoped to the board's plan.
    pub(super) tasks: usize,
    pub(super) tasks_done: usize,
    pub(super) plans: usize,
    pub(super) plans_done: usize,
    pub(super) milestones: usize,
    pub(super) milestones_done: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ActivityView {
    pub(super) kind: String,
    pub(super) title: String,
    pub(super) detail: String,
    pub(super) target: String,
    pub(super) occurred_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct IssueView {
    pub(super) id: u64,
    pub(super) title: String,
    pub(super) severity: String,
    pub(super) task_id: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BoardView {
    pub(super) project_name: String,
    pub(super) goal: String,
    pub(super) summary: String,
    /// When the rolling summary was last written, as RFC 3339; `null` when
    /// unknown (a summary written before payload schema 9, or never written).
    pub(super) summary_updated_at: Option<String>,
    pub(super) plans: Vec<PlanSummaryView>,
    pub(super) plan_id: u64,
    pub(super) plan_title: String,
    pub(super) columns: Vec<ColumnView>,
    pub(super) stats: ProjectStatsView,
    pub(super) activity: Vec<ActivityView>,
    pub(super) open_issues: Vec<IssueView>,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn board_view(
    snapshot: &ProjectSnapshot,
    project_name: String,
    requested_plan: u64,
) -> AppResult<BoardView> {
    let plan_id = if requested_plan == 0 {
        snapshot.meta.active_plan
    } else {
        requested_plan
    };
    if plan_id == 0 {
        return Err(AppError::Message(
            "no active plan; set one with 'ptrack plan use <id>' or pass --plan".to_owned(),
        ));
    }
    let selected = snapshot
        .plan(plan_id)
        .ok_or_else(|| AppError::Message(format!("plan #{plan_id} not found")))?;
    let tasks = snapshot.tasks_for_plan(plan_id).collect::<Vec<_>>();
    let task_ids = tasks.iter().map(|task| task.id).collect::<BTreeSet<_>>();
    let mut note_counts = BTreeMap::new();
    let mut commit_counts = BTreeMap::new();
    let mut issue_counts = BTreeMap::new();
    let mut latest_notes = BTreeMap::new();
    for note in &snapshot.notes {
        if note.target == NoteTarget::Task && task_ids.contains(&note.target_id) {
            *note_counts.entry(note.target_id).or_insert(0) += 1;
            latest_notes.insert(note.target_id, note.body.clone());
        }
    }
    for commit in &snapshot.commits {
        if task_ids.contains(&commit.task_id) {
            *commit_counts.entry(commit.task_id).or_insert(0) += 1;
        }
    }
    let mut open_issues = Vec::new();
    for issue in &snapshot.issues {
        if issue.status == IssueStatus::Open {
            if task_ids.contains(&issue.task_id) {
                *issue_counts.entry(issue.task_id).or_insert(0) += 1;
            }
            open_issues.push(IssueView {
                id: issue.id,
                title: issue.title.clone(),
                severity: issue.severity.as_str().to_owned(),
                task_id: issue.task_id,
            });
        }
    }
    open_issues.truncate(5);
    let statuses = [
        (TaskStatus::Todo, "Todo"),
        (TaskStatus::Doing, "Doing"),
        (TaskStatus::Blocked, "Blocked"),
        (TaskStatus::Done, "Done"),
    ];
    let columns = statuses
        .into_iter()
        .map(|(status, title)| ColumnView {
            status: status.as_str().to_owned(),
            title: title.to_owned(),
            tasks: tasks
                .iter()
                .filter(|task| task.status == status)
                .map(|task| TaskView {
                    id: task.id,
                    title: task.title.clone(),
                    status: task.status.as_str().to_owned(),
                    updated_at: timestamp(task.updated_at),
                    note_count: *note_counts.get(&task.id).unwrap_or(&0),
                    commit_count: *commit_counts.get(&task.id).unwrap_or(&0),
                    issue_count: *issue_counts.get(&task.id).unwrap_or(&0),
                    latest_note: latest_notes.get(&task.id).cloned().unwrap_or_default(),
                    hold_reason: task.hold_reason.clone(),
                    linked_runtime: None,
                    deps: task.deps.clone(),
                    deps_open: open_task_deps(snapshot, task),
                })
                .collect(),
        })
        .collect();
    let counts = snapshot.counts();
    let mut progress = BTreeMap::<u64, (usize, usize)>::new();
    for task in &snapshot.tasks {
        let entry = progress.entry(task.plan_id).or_default();
        entry.0 += 1;
        if task.status == TaskStatus::Done {
            entry.1 += 1;
        }
    }
    let plans = snapshot
        .plans
        .iter()
        .map(|plan| {
            let entry = progress.get(&plan.id).copied().unwrap_or_default();
            PlanSummaryView {
                id: plan.id,
                title: plan.title.clone(),
                status: plan.status.as_str().to_owned(),
                is_active: plan.id == snapshot.meta.active_plan,
                tasks_total: entry.0,
                tasks_done: entry.1,
                hold_reason: plan.hold_reason.clone(),
                claimed_by: plan
                    .claim_owner
                    .as_deref()
                    .map(|owner| snapshot.meta.actor_name(owner).unwrap_or(owner).to_owned()),
                deps_open: open_plan_deps(snapshot, plan),
            }
        })
        .collect();
    let plan_done = tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Done)
        .count();
    Ok(BoardView {
        project_name,
        goal: snapshot.meta.goal.clone(),
        summary: snapshot.meta.summary.clone(),
        summary_updated_at: snapshot.meta.summary_updated_at.map(timestamp),
        plans,
        plan_id,
        plan_title: selected.title.clone(),
        columns,
        stats: ProjectStatsView {
            plan_tasks: tasks.len(),
            plan_tasks_done: plan_done,
            tasks_open: counts.tasks_open,
            tasks_blocked: counts.tasks_blocked,
            notes: counts.notes,
            commits: counts.commits,
            open_issues: counts.issues_open,
            tasks: counts.tasks,
            tasks_done: counts.tasks_done,
            plans: counts.plans,
            plans_done: counts.plans_done,
            milestones: counts.milestones,
            milestones_done: counts.milestones_done,
        },
        activity: recent_activity(snapshot),
        open_issues,
    })
}

pub(crate) fn snapshot_board_view(
    snapshot: &ProjectSnapshot,
    project_name: String,
    plan_id: u64,
) -> AppResult<BoardView> {
    if plan_id != 0 {
        return board_view(snapshot, project_name, plan_id);
    }
    let counts = snapshot.counts();
    let mut progress = BTreeMap::<u64, (usize, usize)>::new();
    for task in &snapshot.tasks {
        let entry = progress.entry(task.plan_id).or_default();
        entry.0 += 1;
        if task.status == TaskStatus::Done {
            entry.1 += 1;
        }
    }
    let plans = snapshot
        .plans
        .iter()
        .map(|plan| {
            let entry = progress.get(&plan.id).copied().unwrap_or_default();
            PlanSummaryView {
                id: plan.id,
                title: plan.title.clone(),
                status: plan.status.as_str().to_owned(),
                is_active: false,
                tasks_total: entry.0,
                tasks_done: entry.1,
                hold_reason: plan.hold_reason.clone(),
                claimed_by: plan
                    .claim_owner
                    .as_deref()
                    .map(|owner| snapshot.meta.actor_name(owner).unwrap_or(owner).to_owned()),
                deps_open: open_plan_deps(snapshot, plan),
            }
        })
        .collect();
    Ok(BoardView {
        project_name,
        goal: snapshot.meta.goal.clone(),
        summary: snapshot.meta.summary.clone(),
        summary_updated_at: snapshot.meta.summary_updated_at.map(timestamp),
        plans,
        plan_id: 0,
        plan_title: String::new(),
        columns: [
            (TaskStatus::Todo, "Todo"),
            (TaskStatus::Doing, "Doing"),
            (TaskStatus::Blocked, "Blocked"),
            (TaskStatus::Done, "Done"),
        ]
        .into_iter()
        .map(|(status, title)| ColumnView {
            status: status.as_str().to_owned(),
            title: title.to_owned(),
            tasks: Vec::new(),
        })
        .collect(),
        stats: ProjectStatsView {
            plan_tasks: 0,
            plan_tasks_done: 0,
            tasks_open: counts.tasks_open,
            tasks_blocked: counts.tasks_blocked,
            notes: counts.notes,
            commits: counts.commits,
            open_issues: counts.issues_open,
            tasks: counts.tasks,
            tasks_done: counts.tasks_done,
            plans: counts.plans,
            plans_done: counts.plans_done,
            milestones: counts.milestones,
            milestones_done: counts.milestones_done,
        },
        activity: recent_activity(snapshot),
        open_issues: snapshot
            .issues
            .iter()
            .filter(|issue| issue.status == IssueStatus::Open)
            .take(5)
            .map(|issue| IssueView {
                id: issue.id,
                title: issue.title.clone(),
                severity: issue.severity.as_str().to_owned(),
                task_id: issue.task_id,
            })
            .collect(),
    })
}

/// The Overview's Recent Memory is project-global by contract: every note and
/// commit the project keeps, regardless of which plan the board has selected.
/// The renderer has no other consumer of `BoardView::activity`, so plan
/// scoping here only ever emptied the feed — a no-plan selection showed just
/// the project-level notes.
pub(super) fn recent_activity(snapshot: &ProjectSnapshot) -> Vec<ActivityView> {
    let mut events = Vec::<(Option<i128>, ActivityView)>::new();
    for note in &snapshot.notes {
        let target = match note.target {
            NoteTarget::Project => "Project".to_owned(),
            NoteTarget::Plan => format!("Plan #{}", note.target_id),
            NoteTarget::Task => format!("Task #{}", note.target_id),
        };
        let kind = if note.kind == MemoryKind::Legacy {
            "note"
        } else {
            note.kind.as_str()
        };
        let title = match note.kind {
            MemoryKind::Decision => "Decision recorded",
            MemoryKind::Blocker => "Blocker recorded",
            MemoryKind::Handoff => "Handoff recorded",
            _ => "Memory recorded",
        };
        events.push((
            note.created_at.unix_nanoseconds(),
            ActivityView {
                kind: kind.to_owned(),
                title: title.to_owned(),
                detail: note.body.clone(),
                target,
                occurred_at: timestamp(note.created_at),
            },
        ));
    }
    for commit in &snapshot.commits {
        let detail = commit.sha.chars().take(8).collect();
        events.push((
            commit.created_at.unix_nanoseconds(),
            ActivityView {
                kind: "commit".to_owned(),
                title: commit.subject.clone(),
                detail,
                target: if commit.task_id == 0 {
                    format!("Plan #{}", commit.plan_id)
                } else {
                    format!("Task #{}", commit.task_id)
                },
                occurred_at: timestamp(commit.created_at),
            },
        ));
    }
    events.sort_by_key(|event| std::cmp::Reverse(event.0));
    events
        .into_iter()
        .take(24)
        .map(|(_, event)| event)
        .collect()
}

pub(super) fn task_card(snapshot: &ProjectSnapshot, task: &Task) -> TaskView {
    let notes = snapshot.notes_for_task(task.id).collect::<Vec<_>>();
    TaskView {
        id: task.id,
        title: task.title.clone(),
        status: task.status.as_str().to_owned(),
        updated_at: timestamp(task.updated_at),
        note_count: notes.len(),
        commit_count: snapshot
            .commits
            .iter()
            .filter(|commit| commit.task_id == task.id)
            .count(),
        issue_count: snapshot
            .issues
            .iter()
            .filter(|issue| issue.task_id == task.id)
            .count(),
        latest_note: notes
            .last()
            .map_or_else(String::new, |note| note.body.clone()),
        hold_reason: task.hold_reason.clone(),
        linked_runtime: None,
        deps: task.deps.clone(),
        deps_open: open_task_deps(snapshot, task),
    }
}

pub(super) fn snapshot_blocker_card(snapshot: &ProjectSnapshot, task: &Task) -> TaskView {
    TaskView {
        id: task.id,
        title: task.title.clone(),
        status: task.status.as_str().to_owned(),
        updated_at: timestamp(task.updated_at),
        note_count: 0,
        commit_count: 0,
        issue_count: 0,
        latest_note: String::new(),
        hold_reason: task.hold_reason.clone(),
        linked_runtime: None,
        deps: task.deps.clone(),
        deps_open: open_task_deps(snapshot, task),
    }
}

pub(super) fn terminal_runtime_summary(
    store: &ProjectStore,
    session: &SessionInfo,
) -> TerminalRuntimeSummaryView {
    let association = session.association.as_ref().and_then(|association| {
        let pointer = association.pointer;
        (pointer.version == 1
            && association.revision != 0
            && store
                .plan(pointer.plan_id)
                .ok()
                .zip(store.task(pointer.task_id).ok())
                .is_some_and(|(_, task)| task.plan_id == pointer.plan_id))
        .then_some(RuntimeAssociation {
            plan_id: pointer.plan_id,
            task_id: pointer.task_id,
            revision: association.revision,
        })
    });
    TerminalRuntimeSummaryView {
        session_id: session.id.clone(),
        profile_kind: session.profile_kind,
        state: session.state,
        live: matches!(
            session.state,
            SessionState::Starting | SessionState::Running | SessionState::Closing
        ),
        association,
    }
}

pub(super) fn task_linked_runtime(
    projection: &RuntimeProjectionView,
    task_id: u64,
) -> TaskLinkedRuntimeDetailView {
    let terminal_candidates = projection
        .terminals
        .iter()
        .filter(|terminal| {
            terminal
                .association
                .is_some_and(|association| association.task_id == task_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    let agent_candidates = projection
        .agents
        .iter()
        .filter(|agent| {
            agent
                .association
                .is_some_and(|association| association.task_id == task_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    let summary = TaskLinkedRuntimeSummaryView {
        terminals: terminal_candidates.len(),
        live_terminals: terminal_candidates
            .iter()
            .filter(|terminal| terminal.live)
            .count(),
        agents: agent_candidates.len(),
        live_agents: agent_candidates.iter().filter(|agent| agent.live).count(),
        terminal_backed_runs: agent_candidates
            .iter()
            .filter(|agent| agent.terminal_backed)
            .count(),
        external_runs: agent_candidates
            .iter()
            .filter(|agent| agent.registration_kind == RegistrationKind::External)
            .count(),
        truncated: projection.sources_truncated,
    };
    let terminal_rows_more = terminal_candidates.len().saturating_sub(64);
    let agent_rows_more = agent_candidates.len().saturating_sub(64);
    TaskLinkedRuntimeDetailView {
        summary,
        terminals: terminal_candidates.into_iter().take(64).collect(),
        agents: agent_candidates.into_iter().take(64).collect(),
        terminal_rows_more,
        agent_rows_more,
    }
}

pub(super) fn apply_linked_runtime_to_board(
    board: &mut BoardView,
    projection: &RuntimeProjectionView,
) {
    for column in &mut board.columns {
        for task in &mut column.tasks {
            let detail = task_linked_runtime(projection, task.id);
            if detail.summary.terminals != 0
                || detail.summary.agents != 0
                || detail.summary.truncated
            {
                task.linked_runtime = Some(detail.summary);
            }
        }
    }
}

pub(super) fn agent_intelligence_for_task(
    run: &AgentRuntimeSummary,
    task_id: u64,
    intelligence: ptrack_agent::AgentIntelligenceV2,
) -> Option<ptrack_agent::AgentIntelligenceV2> {
    let expected = run.association?;
    let observed = intelligence.association?;
    (run.run_id == intelligence.run_id
        && expected.task_id == task_id
        && observed.task_id == task_id
        && expected.plan_id == observed.plan_id
        && expected.revision == observed.revision)
        .then_some(intelligence)
}

pub(crate) fn agent_intelligence_for_task_result(
    run: &AgentRuntimeSummary,
    task_id: u64,
    result: AppResult<ptrack_agent::AgentIntelligenceV2>,
) -> AppResult<Option<ptrack_agent::AgentIntelligenceV2>> {
    match result {
        Ok(value) => Ok(agent_intelligence_for_task(run, task_id, value)),
        Err(error) if error.to_string() == "AgentRun not found" => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn task_detail_value(
    generation: u64,
    snapshot: &ProjectSnapshot,
    task_id: u64,
    linked_runtime: &TaskLinkedRuntimeDetailView,
    agent_intelligence: &[ptrack_agent::AgentIntelligenceV2],
) -> AppResult<Value> {
    let task = snapshot
        .task(task_id)
        .ok_or_else(|| AppError::Message(format!("task #{task_id} not found")))?;
    let mut notes = snapshot
        .notes_for_task(task_id)
        .map(|note| {
            json!({
                "id": note.id,
                "kind": note.kind.as_str(),
                "body": note.body,
                "occurredAt": timestamp(note.created_at)
            })
        })
        .collect::<Vec<_>>();
    notes.reverse();
    let mut commits = snapshot
        .commits
        .iter()
        .filter(|commit| commit.task_id == task_id)
        .map(commit_view)
        .collect::<Vec<_>>();
    commits.reverse();
    let issues = snapshot
        .issues
        .iter()
        .filter(|issue| issue.task_id == task_id)
        .map(|issue| {
            json!({
                "id": issue.id,
                "title": issue.title,
                "severity": issue.severity.as_str(),
                "status": issue.status.as_str(),
                "taskId": issue.task_id
            })
        })
        .collect::<Vec<_>>();
    let mut task = task_card(snapshot, task);
    if linked_runtime.summary.terminals != 0
        || linked_runtime.summary.agents != 0
        || linked_runtime.summary.truncated
    {
        task.linked_runtime = Some(linked_runtime.summary);
    }
    Ok(json!({
        "generation": generation,
        "task": task,
        "linkedRuntime": linked_runtime,
        "agentIntelligence": agent_intelligence,
        "notes": notes,
        "commits": commits,
        "issues": issues
    }))
}

pub(super) fn commit_view(commit: &Commit) -> Value {
    json!({
        "id": commit.id,
        "sha": commit.sha,
        "subject": commit.subject,
        "occurredAt": timestamp(commit.created_at)
    })
}

pub(super) fn note_snapshot_view(note: &Note) -> Value {
    json!({
        "id": note.id,
        "target": note.target.as_str(),
        "targetId": note.target_id,
        "kind": note.kind.as_str(),
        "body": note.body,
        "occurredAt": timestamp(note.created_at)
    })
}

pub(super) fn issue_snapshot_view(issue: &Issue) -> Value {
    json!({
        "id": issue.id,
        "title": issue.title,
        "severity": issue.severity.as_str(),
        "taskId": issue.task_id
    })
}

pub(super) fn issue_record_value(issue: &Issue) -> Value {
    json!({
        "id": issue.id,
        "title": issue.title,
        "body": issue.body,
        "status": issue.status.as_str(),
        "severity": issue.severity.as_str(),
        "taskId": issue.task_id,
        "createdAt": timestamp(issue.created_at),
        "updatedAt": timestamp(issue.updated_at),
    })
}

pub(super) fn issue_detail_summary(snapshot: &ProjectSnapshot, issue: &Issue) -> Value {
    let task = snapshot.task(issue.task_id);
    let plan = task.and_then(|task| snapshot.plan(task.plan_id));
    let mut value = issue_record_value(issue);
    value["taskTitle"] = Value::String(task.map_or_else(String::new, |task| task.title.clone()));
    value["taskStatus"] =
        Value::String(task.map_or_else(String::new, |task| task.status.as_str().to_owned()));
    value["planId"] = json!(task.map_or(0, |task| task.plan_id));
    value["planTitle"] = Value::String(plan.map_or_else(String::new, |plan| plan.title.clone()));
    value
}

pub(super) fn issue_detail_value(
    generation: u64,
    snapshot: &ProjectSnapshot,
    issue_id: u64,
    query: &str,
) -> AppResult<Value> {
    let issue = snapshot
        .issue(issue_id)
        .ok_or_else(|| AppError::Message(format!("issue #{issue_id} not found")))?;
    let query = query.trim().to_lowercase();
    let matches = |id: u64, title: &str| {
        if let Ok(wanted) = query.trim_start_matches('#').parse::<u64>() {
            id == wanted
        } else {
            title.to_lowercase().contains(&query)
        }
    };
    let matching_plans = snapshot
        .plans
        .iter()
        .filter(|plan| plan.status != ptrack_core::PlanStatus::Done)
        .filter(|plan| matches(plan.id, &plan.title))
        .collect::<Vec<_>>();
    let plans = matching_plans
        .iter()
        .take(SNAPSHOT_PLAN_LIMIT)
        .map(|plan| {
            json!({
                "id": plan.id,
                "title": plan.title,
                "status": plan.status.as_str(),
                "holdReason": plan.hold_reason,
            })
        })
        .collect::<Vec<_>>();
    let matching_tasks = snapshot
        .tasks
        .iter()
        .filter(|task| matches(task.id, &task.title))
        .collect::<Vec<_>>();
    let tasks = matching_tasks
        .iter()
        .take(SNAPSHOT_TASK_LIMIT)
        .map(|task| {
            json!({
                "id": task.id,
                "planId": task.plan_id,
                "title": task.title,
                "status": task.status.as_str(),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "generation": generation,
        "issue": issue_detail_summary(snapshot, issue),
        "plans": plans,
        "tasks": tasks,
        "bounds": {
            "plans": bound(plans.len(), matching_plans.len()),
            "tasks": bound(tasks.len(), matching_tasks.len()),
        },
    }))
}

pub(crate) fn project_storage(database: &str, meta: &Meta) -> Value {
    let metadata = fs::metadata(database);
    let (status, exists, size, error) = match metadata {
        Ok(metadata) => ("ready", true, metadata.len(), None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ("error", false, 0, Some("p-track database is missing"))
        }
        Err(_) => (
            "error",
            false,
            0,
            Some("p-track database status is unavailable"),
        ),
    };
    let mut value = json!({
        "status": status,
        "exists": exists,
        "dbPath": database,
        "sizeBytes": size,
        "formatVersion": meta.format_version,
        "lastWriteVersion": meta.last_write_version
    });
    if let Some(error) = error {
        value["error"] = Value::String(error.to_owned());
    }
    value
}

pub(super) fn ensure_snapshot_deadline(deadline: Instant) -> AppResult<()> {
    if Instant::now() >= deadline {
        Err(AppError::Message("context deadline exceeded".to_owned()))
    } else {
        Ok(())
    }
}

pub(crate) struct CapturedGitSnapshot {
    pub(crate) wire: Value,
    pub(crate) agent: ptrack_agent::CoordinationGitSnapshot,
}

pub(super) fn capture_git_snapshot(root: &Path, deadline: Instant) -> CapturedGitSnapshot {
    capture_git_snapshot_with(root.to_path_buf(), deadline, |cancellation, root| {
        ptrack_git::capture(cancellation, root)
    })
}

pub(crate) fn capture_git_snapshot_with<C>(
    root: PathBuf,
    deadline: Instant,
    capture: C,
) -> CapturedGitSnapshot
where
    C: FnOnce(
            &ptrack_git::CancellationToken,
            &Path,
        ) -> Result<ptrack_git::Snapshot, ptrack_git::RepositoryError>
        + Send
        + 'static,
{
    let cancellation = ptrack_git::CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let (sender, receiver) = channel();
    let _worker = thread::Builder::new()
        .name("ptrack-workspace-git-snapshot".to_owned())
        .spawn(move || {
            let _ = sender.send(capture(&worker_cancellation, &root));
        });
    let remaining = deadline.saturating_duration_since(Instant::now());
    let result = if remaining.is_zero() {
        cancellation.cancel();
        Err(ptrack_git::RepositoryError::Cancelled)
    } else {
        match receiver.recv_timeout(remaining) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                cancellation.cancel();
                Err(ptrack_git::RepositoryError::Cancelled)
            }
            Err(RecvTimeoutError::Disconnected) => Err(ptrack_git::RepositoryError::Filesystem(
                "snapshot worker failed",
            )),
        }
    };
    match result {
        Ok(snapshot) => {
            let agent =
                crate::agent_runtime::map_git_snapshot(snapshot.clone()).unwrap_or_default();
            CapturedGitSnapshot {
                wire: json!({ "state": "ready", "snapshot": snapshot }),
                agent,
            }
        }
        Err(error) => CapturedGitSnapshot {
            wire: json!({
                "state": "error",
                "error": format!("Git snapshot unavailable: {error}"),
                "snapshot": ptrack_git::Snapshot::default()
            }),
            agent: ptrack_agent::CoordinationGitSnapshot::default(),
        },
    }
}

pub(super) fn empty_agent_candidates(generation: u64) -> ptrack_agent::AgentRuntimeCandidatesV2 {
    ptrack_agent::AgentRuntimeCandidatesV2 {
        generation,
        runs: Vec::new(),
        bounds: ptrack_agent::BoundedSnapshot::new(0, 0),
        sources_truncated: false,
        analysis_incomplete: false,
    }
}

pub(super) fn empty_agent_activity() -> Value {
    json!({
        "state": "ready",
        "items": [],
        "counts": { "running": 0, "waiting": 0, "blocked": 0, "completed": 0, "failed": 0, "stale": 0, "unknown": 0 },
        "bounds": bound(0, 0),
        "conflicts": [],
        "conflictBounds": bound(0, 0),
        "analysisIncomplete": false,
        "notifications": [],
        "notificationBounds": bound(0, 0),
        "notificationsIncomplete": false,
        "handoffs": { "items": [], "bounds": bound(0, 0), "incomplete": false },
        "worktrees": [],
        "worktreeBounds": bound(0, 0),
        "worktreesIncomplete": false,
        "workflows": { "items": [], "bounds": bound(0, 0), "incomplete": false, "notice": ptrack_agent::AGENT_WORKFLOW_NOTICE },
        "workflowTargets": [],
        "workflowTargetsIncomplete": false
    })
}
