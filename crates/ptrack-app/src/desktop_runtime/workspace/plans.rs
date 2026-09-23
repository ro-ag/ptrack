//! Plan commands: creation, renames, holds, completion, and the destructive
//! delete, move, and copy lifecycle operations.

use ptrack_core::PlanStatus;
use serde_json::{Value, json};

use super::super::command::PlanCommand;
use super::super::ports::DesktopWorkspace;
use super::super::support::{
    first_plan_view, first_run_title, lock, message, optional_string, trimmed_nonempty,
    unavailable, value,
};
use super::super::wire::CreateFirstPlanResultV1;
use super::BoundDesktopWorkspace;
use crate::{
    AppError, AppResult, Mutation, MutationResult, PlanLifecycleOutcome, PlanLifecycleRequest,
    complete_plan,
};

impl BoundDesktopWorkspace {
    pub(super) fn plan_command(&self, command: PlanCommand<'_>) -> AppResult<Value> {
        match command {
            PlanCommand::Add { generation, title } => {
                self.require_exact_generation(generation)?;
                let title = first_run_title(title, "plan")?;
                let result = lock(&self.application).mutate(Mutation::AddPlan {
                    title,
                    milestone_id: 0,
                })?;
                let MutationResult::Plan(plan) = result else {
                    return Err(unavailable("plan mutation"));
                };
                Ok(json!({ "generation": self.generation, "plan": first_plan_view(&plan) }))
            }
            PlanCommand::CreateFirst { generation, title } => {
                self.require_exact_generation(generation)?;
                let title = first_run_title(title, "plan")?;
                let plan = self.project_store()?.create_first_plan(title)?;
                value(CreateFirstPlanResultV1 {
                    plan: first_plan_view(&plan),
                    state: self.first_run_state(),
                })
            }
            PlanCommand::Rename {
                generation,
                plan_id,
                title,
            } => {
                self.require_exact_generation(generation)?;
                let title = trimmed_nonempty(title, "plan title cannot be empty")?;
                lock(&self.application).mutate(Mutation::SetPlanTitle { id: plan_id, title })?;
                Ok(json!({ "generation": self.generation }))
            }
            PlanCommand::Complete {
                generation,
                plan_id,
            } => {
                self.require_exact_generation(generation)?;
                self.complete_plan_v1(plan_id)
            }
            command @ (PlanCommand::Hold { .. }
            | PlanCommand::Resume { .. }
            | PlanCommand::Reopen { .. }
            | PlanCommand::SetActive { .. }) => self.plan_state_command(command),
            PlanCommand::Delete {
                generation,
                plan_id,
                confirm,
                preview_revision,
            } => {
                self.require_exact_generation(generation)?;
                self.delete_plan_v1(plan_id, confirm, preview_revision)
            }
            PlanCommand::Move {
                generation,
                plan_id,
                to,
                rename,
            } => {
                self.require_exact_generation(generation)?;
                self.move_plan_v1(plan_id, to, optional_string(rename))
            }
            PlanCommand::Copy {
                generation,
                plan_id,
                to,
                rename,
            } => {
                self.require_exact_generation(generation)?;
                self.copy_plan_v1(plan_id, optional_string(to), optional_string(rename))
            }
        }
    }

    /// Holds, resumes, reopens, or makes a plan current: the plan commands
    /// that change only its state, never its contents.
    fn plan_state_command(&self, command: PlanCommand<'_>) -> AppResult<Value> {
        match command {
            PlanCommand::Hold {
                generation,
                plan_id,
                reason,
            } => {
                self.require_exact_generation(generation)?;
                let reason = trimmed_nonempty(reason, "hold reason cannot be empty")?;
                lock(&self.application).mutate(Mutation::SetPlanHold {
                    id: plan_id,
                    reason: Some(reason),
                })?;
                Ok(json!({ "generation": self.generation }))
            }
            PlanCommand::Resume {
                generation,
                plan_id,
            } => {
                self.require_exact_generation(generation)?;
                lock(&self.application).mutate(Mutation::SetPlanHold {
                    id: plan_id,
                    reason: None,
                })?;
                Ok(json!({ "generation": self.generation }))
            }
            PlanCommand::Reopen {
                generation,
                plan_id,
            } => {
                self.require_exact_generation(generation)?;
                self.reopen_plan_v1(plan_id)
            }
            PlanCommand::SetActive {
                generation,
                plan_id,
            } => {
                self.require_exact_generation(generation)?;
                self.set_active_plan_v1(plan_id)
            }
            _ => Err(unavailable("plan state command")),
        }
    }

    fn complete_plan_v1(&self, plan_id: u64) -> AppResult<Value> {
        let mut application = lock(&self.application);
        let result = complete_plan(application.as_mut(), plan_id, false)?;
        drop(application);
        Ok(json!({
            "generation": self.generation,
            "checkpoint": {
                "markdown": result.checkpoint.markdown(),
                "openPlans": result.checkpoint.open_plans
                    .iter()
                    .map(|(id, title)| json!({ "id": id, "title": title }))
                    .collect::<Vec<_>>(),
            },
        }))
    }

    fn copy_plan_v1(
        &self,
        plan_id: u64,
        to: Option<String>,
        rename: Option<String>,
    ) -> AppResult<Value> {
        let outcome = lock(&self.application).plan_lifecycle(PlanLifecycleRequest::Copy {
            plan_id,
            to,
            rename,
        })?;
        let PlanLifecycleOutcome::Transferred(summary) = outcome else {
            return Err(unavailable("plan copy result"));
        };
        Ok(json!({ "generation": self.generation, "summary": transfer_summary_json(&summary) }))
    }

    /// Runs a destructive plan operation only while no live terminal or agent
    /// is linked to the plan or to any of its tasks — the same exact resource
    /// check a task move to another plan applies, fenced the same way.
    fn with_plan_resources_released<T>(
        &self,
        plan_id: u64,
        action: &str,
        operation: impl FnOnce() -> AppResult<T>,
    ) -> AppResult<T> {
        let _transition = lock(&self.resource_transition);
        let _admission = self.fence_resource_admission()?;
        if lock(&self.resource_admission.state).pending != 0 {
            return Err(AppError::Message(format!(
                "plan {action} must retry after resource admission completes"
            )));
        }
        let snapshot = self.snapshot()?;
        self.with_exact_plan_resources(&snapshot, plan_id, |resources| {
            if !resources.is_empty() {
                return Err(AppError::Message(format!(
                    "stop or detach linked terminals and agents before {action} this plan"
                )));
            }
            operation()
        })
    }

    /// Previews or deletes one plan. The preview carries a revision of exactly
    /// what it counted; a delete that names one is refused when the plan no
    /// longer matches it, so a stale dialog can never confirm counts the user
    /// was not shown.
    fn delete_plan_v1(
        &self,
        plan_id: u64,
        confirm: bool,
        preview_revision: &str,
    ) -> AppResult<Value> {
        self.with_plan_resources_released(plan_id, "deleting", || {
            let mut application = lock(&self.application);
            if !confirm {
                let PlanLifecycleOutcome::Preview(summary) =
                    application.plan_lifecycle(PlanLifecycleRequest::DeletePreview { plan_id })?
                else {
                    return Err(unavailable("plan delete preview"));
                };
                let summary = delete_summary_json(&summary);
                return Ok(json!({
                    "generation": self.generation,
                    "preview": true,
                    "previewRevision": delete_preview_revision(&summary),
                    "summary": summary,
                }));
            }
            if !preview_revision.is_empty() {
                let PlanLifecycleOutcome::Preview(current) =
                    application.plan_lifecycle(PlanLifecycleRequest::DeletePreview { plan_id })?
                else {
                    return Err(unavailable("plan delete preview"));
                };
                if delete_preview_revision(&delete_summary_json(&current)) != preview_revision {
                    return Err(message(
                        "plan changed since the delete preview; review it again",
                    ));
                }
            }
            let PlanLifecycleOutcome::Deleted(summary) =
                application.plan_lifecycle(PlanLifecycleRequest::Delete { plan_id })?
            else {
                return Err(unavailable("plan delete result"));
            };
            drop(application);
            Ok(json!({
                "generation": self.generation,
                "preview": false,
                "summary": delete_summary_json(&summary),
            }))
        })
    }

    fn move_plan_v1(&self, plan_id: u64, to: &str, rename: Option<String>) -> AppResult<Value> {
        self.with_plan_resources_released(plan_id, "moving", || {
            let outcome = lock(&self.application).plan_lifecycle(PlanLifecycleRequest::Move {
                plan_id,
                to: to.to_owned(),
                rename,
            })?;
            let PlanLifecycleOutcome::Transferred(summary) = outcome else {
                return Err(unavailable("plan move result"));
            };
            Ok(json!({ "generation": self.generation, "summary": transfer_summary_json(&summary) }))
        })
    }

    /// Returns a done plan to active. The store's claim gate applies exactly
    /// as it does to every other plan status change.
    fn reopen_plan_v1(&self, plan_id: u64) -> AppResult<Value> {
        let snapshot = self.snapshot()?;
        let plan = snapshot
            .plan(plan_id)
            .ok_or_else(|| AppError::Message(format!("plan #{plan_id} not found")))?;
        if plan.status != PlanStatus::Done {
            return Err(AppError::Message(format!(
                "plan #{plan_id} is {} and cannot be reopened",
                plan.status.as_str()
            )));
        }
        lock(&self.application).mutate(Mutation::SetPlanStatus {
            id: plan_id,
            status: PlanStatus::Active,
        })?;
        Ok(json!({ "generation": self.generation, "planId": plan_id, "status": "active" }))
    }

    /// Makes one plan the caller's current plan, or clears it with `0`.
    ///
    /// This is the exact mutation `ptrack plan use` runs, so the desktop and
    /// the CLI agree on what "current" means: with a configured identity it
    /// claims the plan and sets that identity's entry in the per-actor map,
    /// and a plan someone else holds is refused by the store's claim gate.
    /// A done or archived plan takes no new work, so it is refused here
    /// rather than pinned as current.
    fn set_active_plan_v1(&self, plan_id: u64) -> AppResult<Value> {
        if plan_id != 0 {
            let plan = self
                .project_store()?
                .plan(plan_id)
                .map_err(|error| match error {
                    ptrack_store::StoreError::NotFound => {
                        AppError::Message(format!("plan #{plan_id} not found"))
                    }
                    other => AppError::from(other),
                })?;
            if matches!(plan.status, PlanStatus::Done | PlanStatus::Archived) {
                return Err(AppError::Message(format!(
                    "plan #{plan_id} is {} and cannot be the current plan",
                    plan.status.as_str()
                )));
            }
        }
        lock(&self.application).mutate(Mutation::SetActivePlan(plan_id))?;
        Ok(json!({ "generation": self.generation }))
    }
}

fn delete_summary_json(summary: &ptrack_store::PlanDeleteSummary) -> Value {
    json!({
        "planId": summary.plan_id,
        "title": summary.title,
        "tasks": summary.tasks,
        "notes": summary.notes,
        "commits": summary.commits_unlinked,
        "detachedIssues": summary
            .issues
            .iter()
            .map(|(id, title)| json!({ "id": id, "title": title }))
            .collect::<Vec<_>>(),
    })
}

/// A stable revision of one delete preview: it changes whenever anything the
/// preview counted changes.
fn delete_preview_revision(summary: &Value) -> String {
    use std::hash::{DefaultHasher, Hash as _, Hasher as _};
    let mut hasher = DefaultHasher::new();
    summary.to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn transfer_summary_json(summary: &crate::PlanTransferSummary) -> Value {
    json!({
        "sourcePlanId": summary.source_plan_id,
        "newPlanId": summary.new_plan_id,
        "title": summary.title,
        "sourceProject": summary.source_project,
        "targetProject": summary.target_project,
        "moved": summary.moved,
        "tasks": summary.tasks,
        "notes": summary.notes,
        "issues": summary.issues,
        "commits": summary.commits,
    })
}
