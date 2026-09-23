//! The bounded workspace snapshot the desktop renders from, and the runtime
//! projection of the terminals and agent runs linked to its tasks.

use std::collections::BTreeSet;
use std::time::Instant;

use ptrack_core::{IssueStatus, ProjectSnapshot, open_plan_deps};
use ptrack_store::StoreError;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::super::ports::DesktopWorkspace;
#[cfg(test)]
use super::super::projection::board_view;
use super::super::projection::{
    PlanSummaryView, ProjectStatsView, RuntimeProjectionView, SnapshotTrackingBounds,
    SnapshotTrackingCapture, apply_linked_runtime_to_board, capture_git_snapshot,
    empty_agent_activity, empty_agent_candidates, ensure_snapshot_deadline, issue_snapshot_view,
    note_snapshot_view, project_storage, snapshot_blocker_card, snapshot_board_view,
    terminal_runtime_summary,
};
use super::super::support::{bound, lock, message};
use super::super::wire::WorkspaceProject;
use super::super::{
    SNAPSHOT_ACTIVITY_LIMIT, SNAPSHOT_BLOCKER_LIMIT, SNAPSHOT_ISSUE_LIMIT, SNAPSHOT_NOTE_LIMIT,
    SNAPSHOT_PLAN_LIMIT, SNAPSHOT_RUNTIME_LIMIT, SNAPSHOT_TASK_LIMIT, WORKSPACE_SNAPSHOT_TIMEOUT,
};
use super::BoundDesktopWorkspace;
use crate::{AppError, AppResult};

impl BoundDesktopWorkspace {
    /// The `GetWorkspaceSnapshot` answer: one bounded capture of tracking,
    /// git, agent, and terminal state under a single deadline.
    pub(super) fn workspace_snapshot_v1(&self, plan_id: Option<u64>) -> AppResult<Value> {
        let deadline = Instant::now() + WORKSPACE_SNAPSHOT_TIMEOUT;
        let mut tracking = self.bounded_snapshot_tracking(plan_id, deadline)?;
        let git = capture_git_snapshot(&self.endpoint.root, deadline);
        ensure_snapshot_deadline(deadline)?;
        let agent_sections = self.agent.as_ref().map_or_else(
            || Ok(None),
            |agent| {
                agent
                    .workspace_snapshot(self.generation, &git.agent, deadline)
                    .map(Some)
            },
        )?;
        let candidates = agent_sections.as_ref().map_or_else(
            || empty_agent_candidates(self.generation),
            |sections| sections.runtime.clone(),
        );
        let projection = self.runtime_projection_with_agents_until(
            &tracking.snapshot,
            candidates,
            Some(deadline),
        )?;
        apply_linked_runtime_to_board(&mut tracking.board, &projection);
        Self::workspace_snapshot(
            self.generation,
            &self.project(),
            tracking,
            &projection,
            agent_sections.as_ref(),
            &git.wire,
            deadline,
        )
    }

    #[allow(clippy::too_many_lines)] // One bounded storage capture keeps totals and rows aligned.
    fn bounded_snapshot_tracking(
        &self,
        requested_plan: Option<u64>,
        deadline: Instant,
    ) -> AppResult<SnapshotTrackingCapture> {
        let store = self.project_store()?;
        let mut meta = store.meta()?;
        let identity = lock(&self.application).identity()?;
        meta.active_plan = meta.active_plan_for(identity.as_ref().map(|i| i.id.as_str()));
        ensure_snapshot_deadline(deadline)?;
        let plans = store.plans_bounded(SNAPSHOT_PLAN_LIMIT)?;
        let selected_plan = match requested_plan {
            None => 0,
            Some(0) if self.initial_plan == 0 => meta.active_plan,
            Some(0) => self.initial_plan,
            Some(plan_id) => plan_id,
        };
        let tasks = if selected_plan == 0 {
            ptrack_store::Bounded {
                items: Vec::new(),
                total: 0,
                more: 0,
            }
        } else {
            store.tasks_by_plan_bounded_until(selected_plan, SNAPSHOT_TASK_LIMIT, deadline)?
        };
        ensure_snapshot_deadline(deadline)?;
        let blockers = store.blocked_tasks_bounded_until(SNAPSHOT_BLOCKER_LIMIT, deadline)?;
        let notes = store.recent_notes_bounded(SNAPSHOT_NOTE_LIMIT)?;
        let commits = store.recent_commits_bounded(SNAPSHOT_NOTE_LIMIT)?;
        let issues = store.open_issues_bounded_until(SNAPSHOT_ISSUE_LIMIT, deadline)?;
        let task_ids = tasks
            .items
            .iter()
            .map(|task| task.id)
            .collect::<BTreeSet<_>>();
        let associations = store.task_associations_until(&task_ids, deadline)?;
        let mut plan_ids = plans
            .items
            .iter()
            .map(|plan| plan.id)
            .collect::<BTreeSet<_>>();
        if selected_plan != 0 {
            plan_ids.insert(selected_plan);
        }
        let plan_progress = store.plan_task_progress_for_until(&plan_ids, deadline)?;
        let counts = store.counts_until(deadline)?;
        ensure_snapshot_deadline(deadline)?;
        let mut snapshot_plans = plans.items.clone();
        if selected_plan != 0 && !snapshot_plans.iter().any(|plan| plan.id == selected_plan) {
            snapshot_plans.push(store.plan(selected_plan).map_err(|error| match error {
                StoreError::NotFound => {
                    AppError::Message(format!("plan #{selected_plan} not found"))
                }
                other => AppError::from(other),
            })?);
        }
        let snapshot = ProjectSnapshot::new(
            meta,
            Vec::new(),
            snapshot_plans,
            tasks.items,
            issues.items,
            notes.items,
            commits.items,
        );
        let mut board = snapshot_board_view(&snapshot, self.project().name, selected_plan)?;
        let mut board_plans = plans.items.clone();
        if selected_plan != 0 && !board_plans.iter().any(|plan| plan.id == selected_plan) {
            board_plans.pop();
            board_plans.push(
                snapshot
                    .plan(selected_plan)
                    .ok_or_else(|| AppError::Message(format!("plan #{selected_plan} not found")))?
                    .clone(),
            );
        }
        board.plans = board_plans
            .iter()
            .map(|plan| {
                let progress = plan_progress.get(&plan.id).copied().unwrap_or_default();
                PlanSummaryView {
                    id: plan.id,
                    title: plan.title.clone(),
                    status: plan.status.as_str().to_owned(),
                    is_active: plan.id == snapshot.meta.active_plan,
                    tasks_total: progress.total,
                    tasks_done: progress.done,
                    hold_reason: plan.hold_reason.clone(),
                    claimed_by: plan
                        .claim_owner
                        .as_deref()
                        .map(|owner| snapshot.meta.actor_name(owner).unwrap_or(owner).to_owned()),
                    // Openness against the bounded snapshot: a dep plan not
                    // in this page counts as satisfied, matching the core
                    // missing-target rule.
                    deps_open: open_plan_deps(&snapshot, plan),
                }
            })
            .collect();
        for column in &mut board.columns {
            for task in &mut column.tasks {
                task.note_count = *associations.note_counts.get(&task.id).unwrap_or(&0);
                task.commit_count = *associations.commit_counts.get(&task.id).unwrap_or(&0);
                task.issue_count = *associations.issue_counts.get(&task.id).unwrap_or(&0);
                task.latest_note = associations
                    .latest_notes
                    .get(&task.id)
                    .cloned()
                    .unwrap_or_default();
            }
        }
        let progress = plan_progress
            .get(&selected_plan)
            .copied()
            .unwrap_or_default();
        board.stats = ProjectStatsView {
            plan_tasks: progress.total,
            plan_tasks_done: progress.done,
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
        };
        let activity_total = notes.total.saturating_add(commits.total);
        let blocker_cards = blockers
            .items
            .iter()
            .map(|task| snapshot_blocker_card(&snapshot, task))
            .collect();
        Ok(SnapshotTrackingCapture {
            snapshot,
            board,
            blockers: blocker_cards,
            bounds: SnapshotTrackingBounds {
                plans: plans.total,
                tasks: tasks.total,
                blockers: blockers.total,
                notes: notes.total,
                activity: activity_total,
                issues: issues.total,
            },
        })
    }

    /// The full board projection the retired `GetBoardV2` bridge command
    /// served. The desktop reads the bounded board inside
    /// `GetWorkspaceSnapshot`; tests still assert the unbounded shape here.
    #[cfg(test)]
    pub(crate) fn board_v2(&self, generation: u64, plan_id: u64) -> AppResult<Value> {
        let _workspace_call = self.begin_workspace_call()?;
        self.require_generation(generation)?;
        let snapshot = self.snapshot()?;
        let mut board = board_view(
            &snapshot,
            self.project().name,
            if plan_id == 0 {
                self.initial_plan
            } else {
                plan_id
            },
        )?;
        apply_linked_runtime_to_board(&mut board, &self.runtime_projection(&snapshot)?);
        Ok(json!({ "generation": self.generation, "board": board }))
    }

    pub(super) fn runtime_projection(
        &self,
        snapshot: &ProjectSnapshot,
    ) -> AppResult<RuntimeProjectionView> {
        let candidates = self.agent.as_ref().map_or_else(
            || Ok(empty_agent_candidates(self.generation)),
            |agent| agent.agent_runtime_candidates(self.generation),
        )?;
        self.runtime_projection_with_agents(snapshot, candidates)
    }

    fn runtime_projection_with_agents(
        &self,
        snapshot: &ProjectSnapshot,
        candidates: ptrack_agent::AgentRuntimeCandidatesV2,
    ) -> AppResult<RuntimeProjectionView> {
        self.runtime_projection_with_agents_until(snapshot, candidates, None)
    }

    fn runtime_projection_with_agents_until(
        &self,
        _snapshot: &ProjectSnapshot,
        candidates: ptrack_agent::AgentRuntimeCandidatesV2,
        deadline: Option<Instant>,
    ) -> AppResult<RuntimeProjectionView> {
        if let Some(deadline) = deadline {
            ensure_snapshot_deadline(deadline)?;
        }
        let store = self.project_store()?;
        let (terminals, terminal_total) = self.terminal.as_ref().map_or_else(
            || Ok((Vec::new(), 0)),
            |terminal| {
                terminal
                    .runtime_session_snapshot(self.generation)
                    .and_then(|(sessions, total)| {
                        let mut rows = Vec::with_capacity(sessions.len());
                        for session in &sessions {
                            if let Some(deadline) = deadline {
                                ensure_snapshot_deadline(deadline)?;
                            }
                            rows.push(terminal_runtime_summary(&store, session));
                        }
                        Ok((rows, total))
                    })
            },
        )?;
        if let Some(deadline) = deadline {
            ensure_snapshot_deadline(deadline)?;
        }
        Ok(RuntimeProjectionView {
            sources_truncated: candidates.sources_truncated || terminal_total > terminals.len(),
            terminals,
            terminal_total,
            agent_total: candidates.bounds.total,
            agents: candidates.runs,
        })
    }
    fn workspace_snapshot(
        generation: u64,
        project: &WorkspaceProject,
        mut tracking: SnapshotTrackingCapture,
        runtime: &RuntimeProjectionView,
        agent_sections: Option<&ptrack_agent::AgentWorkspaceSnapshotV2>,
        git: &Value,
        deadline: Instant,
    ) -> AppResult<Value> {
        ensure_snapshot_deadline(deadline)?;
        let snapshot = &tracking.snapshot;
        let blockers = tracking.blockers;
        let notes = snapshot
            .notes
            .iter()
            .rev()
            .take(SNAPSHOT_NOTE_LIMIT)
            .map(note_snapshot_view)
            .collect::<Vec<_>>();
        let issues = snapshot
            .issues
            .iter()
            .rev()
            .filter(|issue| issue.status == IssueStatus::Open)
            .take(SNAPSHOT_ISSUE_LIMIT)
            .map(issue_snapshot_view)
            .collect::<Vec<_>>();
        let (agent_activity, drift) = agent_sections.map_or_else(
            || {
                Ok((
                    empty_agent_activity(),
                    json!({ "state": "ready", "findings": [], "bounds": bound(0, 0), "incomplete": false }),
                ))
            },
            |sections| {
                Ok::<_, AppError>((
                    serde_json::to_value(&sections.activity).map_err(message)?,
                    serde_json::to_value(&sections.drift).map_err(message)?,
                ))
            },
        )?;
        ensure_snapshot_deadline(deadline)?;
        let terminal_total = runtime.terminal_total;
        let terminals = json!({
            "state": "ready",
            "sessions": runtime.terminals.iter().take(SNAPSHOT_RUNTIME_LIMIT).collect::<Vec<_>>(),
            "bounds": bound(terminal_total.min(SNAPSHOT_RUNTIME_LIMIT), terminal_total)
        });
        let agent_total = runtime.agent_total;
        let agent_runs = json!({
            "state": "ready",
            "runs": runtime.agents.iter().take(SNAPSHOT_RUNTIME_LIMIT).collect::<Vec<_>>(),
            "bounds": bound(agent_total.min(SNAPSHOT_RUNTIME_LIMIT), agent_total)
        });
        tracking.board.plans.truncate(SNAPSHOT_PLAN_LIMIT);
        let mut remaining_tasks = SNAPSHOT_TASK_LIMIT;
        for column in &mut tracking.board.columns {
            column.tasks.truncate(remaining_tasks);
            remaining_tasks = remaining_tasks.saturating_sub(column.tasks.len());
        }
        tracking.board.activity.truncate(SNAPSHOT_ACTIVITY_LIMIT);
        let board_task_count = tracking
            .board
            .columns
            .iter()
            .map(|column| column.tasks.len())
            .sum::<usize>();
        let board_plan_count = tracking.board.plans.len();
        let board_activity_count = tracking.board.activity.len();
        let storage = project_storage(&project.db_path, &snapshot.meta);
        Ok(json!({
            "generation": generation,
            "capturedAt": OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default(),
            "project": {
                "name": project.name,
                "root": project.root,
                "storage": storage
            },
            "tracking": {
                "state": "ready",
                "board": tracking.board,
                "blockers": blockers,
                "notes": notes,
                "issues": issues,
                "bounds": {
                    "plans": bound(board_plan_count, tracking.bounds.plans),
                    "tasks": bound(board_task_count, tracking.bounds.tasks),
                    "blockers": bound(blockers.len(), tracking.bounds.blockers),
                    "notes": bound(notes.len(), tracking.bounds.notes),
                    "activity": bound(board_activity_count, tracking.bounds.activity),
                    "issues": bound(issues.len(), tracking.bounds.issues)
                }
            },
            "git": git,
            "agentActivity": agent_activity,
            "terminals": terminals,
            "agentRuns": agent_runs,
            "drift": drift
        }))
    }
}
