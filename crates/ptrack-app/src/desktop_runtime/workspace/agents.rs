//! Agent commands: the linked agent launch and its rollback, handoffs, task
//! ownership, worktrees, and the reviewed agent workflows.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ptrack_agent::{
    AgentWorkflowKind, AssociationCatalog, AssociationHost,
    AssociationPointer as AgentAssociationPointer, BoundedItems, LaunchContextStore,
    ScanBoundedItems, build_launch_context,
};
use ptrack_core::{Commit, Issue, IssueStatus, Meta, Note, Plan, ProjectSnapshot, Task};
use ptrack_terminal::TerminalAssociationPointer;
use serde_json::Value;

use super::super::command::{AgentCommand, AgentRegistryCommand, LinkedAgentCommand};
use super::super::support::{lock, unavailable, value};
use super::terminals::validate_association_pointer;
use super::{BoundDesktopWorkspace, DesktopAgentRuntime};
use crate::{AppError, AppResult};

impl BoundDesktopWorkspace {
    pub(super) fn agent_command(&self, command: AgentCommand<'_>) -> AppResult<Value> {
        match command {
            AgentCommand::Linked(command) => self.linked_agent_command(command),
            AgentCommand::Registry(command) => self.agent_registry_command(command),
        }
    }

    fn linked_agent_command(&self, command: LinkedAgentCommand<'_>) -> AppResult<Value> {
        match command {
            LinkedAgentCommand::Launch {
                generation,
                profile_id,
                cwd,
                rows,
                columns,
                pointer,
            } => {
                self.require_exact_generation(generation)?;
                self.launch_linked_agent(profile_id, cwd, rows, columns, pointer)
            }
            LinkedAgentCommand::Rollback {
                generation,
                session_id,
            } => {
                self.require_exact_generation(generation)?;
                self.rollback_linked_agent_launch(session_id)
            }
        }
    }

    /// Every registry command requires the registry before its generation.
    fn agent_registry_command(&self, command: AgentRegistryCommand<'_>) -> AppResult<Value> {
        let agent = self.agent_registry()?;
        match command {
            AgentRegistryCommand::PreviewHandoff { generation, run_id } => {
                self.require_generation(generation)?;
                value(agent.preview_handoff(self.generation, run_id)?)
            }
            AgentRegistryCommand::SendHandoff {
                generation,
                source_run_id,
                target_run_id,
                expected_source_revision,
                expected_target_revision,
            } => {
                self.require_exact_generation(generation)?;
                value(agent.send_handoff(
                    self.generation,
                    source_run_id,
                    target_run_id,
                    expected_source_revision,
                    expected_target_revision,
                )?)
            }
            AgentRegistryCommand::AcknowledgeHandoff {
                generation,
                id,
                target_run_id,
            } => {
                self.require_exact_generation(generation)?;
                value(agent.acknowledge_handoff(self.generation, id, target_run_id)?)
            }
            AgentRegistryCommand::SetTaskOwnership {
                generation,
                run_id,
                expected_association_revision,
                owned,
            } => {
                self.require_exact_generation(generation)?;
                value(agent.set_task_ownership(
                    self.generation,
                    run_id,
                    expected_association_revision,
                    owned,
                )?)
            }
            AgentRegistryCommand::SetWorktree {
                generation,
                run_id,
                expected_association_revision,
                root,
                associated,
            } => {
                self.require_exact_generation(generation)?;
                value(agent.set_worktree(
                    self.generation,
                    run_id,
                    expected_association_revision,
                    root,
                    associated,
                )?)
            }
            AgentRegistryCommand::PrepareWorkflow {
                generation,
                run_id,
                expected_association_revision,
                kind,
                target_branch,
            } => {
                self.require_exact_generation(generation)?;
                let kind = parse_workflow_kind(kind)?;
                value(agent.prepare_workflow(
                    self.generation,
                    run_id,
                    expected_association_revision,
                    kind,
                    target_branch,
                )?)
            }
            AgentRegistryCommand::ApproveWorkflow { generation, id } => {
                self.require_exact_generation(generation)?;
                value(agent.approve_workflow(self.generation, id)?)
            }
            AgentRegistryCommand::DismissWorkflow { generation, id } => {
                self.require_exact_generation(generation)?;
                value(agent.dismiss_workflow(self.generation, id)?)
            }
        }
    }

    fn agent_registry(&self) -> AppResult<&Arc<dyn DesktopAgentRuntime>> {
        self.agent
            .as_ref()
            .ok_or_else(|| unavailable("AgentRun registry"))
    }

    fn launch_linked_agent(
        &self,
        profile_id: &str,
        cwd: &str,
        rows: u16,
        columns: u16,
        pointer: TerminalAssociationPointer,
    ) -> AppResult<Value> {
        let _admission = self.begin_resource_admission()?;
        if profile_id.is_empty() || profile_id.trim() != profile_id {
            return Err(AppError::Message(
                "an installed agent profile is required".to_owned(),
            ));
        }
        let snapshot = self.snapshot()?;
        validate_association_pointer(&snapshot, pointer)?;
        let _transition = lock(&self.resource_transition);
        let terminal = self
            .terminal
            .as_ref()
            .ok_or_else(|| unavailable("terminal manager"))?;
        let profiles = terminal.profiles(self.generation).map_err(|error| {
            AppError::Message(format!("discover installed agent profiles: {error}"))
        })?;
        let profile = profiles
            .profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| {
                AppError::Message(format!(
                    "installed agent profile {profile_id:?} is unavailable"
                ))
            })?;
        if profile.kind != ptrack_terminal::ProfileKind::Agent {
            return Err(AppError::Message(format!(
                "terminal profile {profile_id:?} is not an agent"
            )));
        }
        let agent = self
            .agent
            .as_ref()
            .ok_or_else(|| unavailable("AgentRun registry"))?;
        let cwd_value = cwd;
        if cwd_value.len() > 4_096 {
            return Err(AppError::Message(
                "linked launch working directory is too long".to_owned(),
            ));
        }
        let cwd = self.resolve_linked_launch_cwd(cwd_value)?;
        let context_store = ProjectLaunchContextStore {
            root: &self.endpoint.root,
            snapshot: &snapshot,
        };
        let host = AssociationHost::new(&self.endpoint.root, self.generation, Some(&context_store))
            .map_err(|error| AppError::Message(error.to_string()))?;
        let context = build_launch_context(
            Some(&context_store),
            Some(&host),
            AgentAssociationPointer {
                version: pointer.version,
                plan_id: pointer.plan_id,
                task_id: pointer.task_id,
            },
        )
        .map_err(|error| AppError::Message(error.to_string()))?;
        let result = terminal.create_linked(
            self.generation,
            profile_id,
            Some(&cwd),
            rows,
            columns,
            pointer,
            &context.text,
        )?;
        confirm_linked_launch(
            agent.has_linked_terminal(self.generation, &result.session_id),
            || terminal.rollback_failed_linked(self.generation, &result.session_id),
        )?;
        value(result)
    }

    fn rollback_linked_agent_launch(&self, session_id: &str) -> AppResult<Value> {
        let _transition = lock(&self.resource_transition);
        let agent = self
            .agent
            .as_ref()
            .ok_or_else(|| unavailable("AgentRun registry"))?;
        if !agent.has_linked_terminal(self.generation, session_id)? {
            return Err(AppError::Message(
                "linked agent launch is unavailable".to_owned(),
            ));
        }
        self.terminal
            .as_ref()
            .ok_or_else(|| unavailable("terminal manager"))?
            .rollback_linked(self.generation, session_id)?;
        Ok(Value::Null)
    }

    pub(crate) fn resolve_linked_launch_cwd(&self, requested: &str) -> AppResult<PathBuf> {
        let candidate = if requested.is_empty() {
            self.endpoint.root.clone()
        } else {
            let requested = Path::new(requested);
            if requested.is_absolute() {
                requested.to_path_buf()
            } else {
                self.endpoint.root.join(requested)
            }
        };
        let canonical = fs::canonicalize(candidate).map_err(|error| {
            AppError::Message(format!(
                "canonicalize linked launch working directory: {error}"
            ))
        })?;
        if canonical.starts_with(&self.endpoint.root) {
            return Ok(canonical);
        }
        let cancellation = ptrack_git::CancellationToken::new();
        ptrack_git::RepositoryService::new()
            .inspect_worktree(&cancellation, &self.endpoint.root, &canonical)
            .map(|_| canonical)
            .map_err(|_| {
                AppError::Message(
                    "linked launch working directory is outside the current project or its existing worktrees"
                        .to_owned(),
                )
            })
    }
}

struct ProjectLaunchContextStore<'a> {
    root: &'a Path,
    snapshot: &'a ProjectSnapshot,
}

impl LaunchContextStore for ProjectLaunchContextStore<'_> {
    fn project_root(&self) -> Result<PathBuf, String> {
        Ok(self.root.to_path_buf())
    }

    fn meta(&self) -> Result<Meta, String> {
        Ok(self.snapshot.meta.clone())
    }

    fn plan(&self, id: u64) -> Result<Option<Plan>, String> {
        Ok(self.snapshot.plan(id).cloned())
    }

    fn task(&self, id: u64) -> Result<Option<Task>, String> {
        Ok(self.snapshot.task(id).cloned())
    }

    fn recent_notes(&self, limit: usize) -> Result<BoundedItems<Note>, String> {
        let total = self.snapshot.notes.len();
        Ok(BoundedItems {
            items: self
                .snapshot
                .notes
                .iter()
                .rev()
                .take(limit)
                .cloned()
                .collect(),
            more: total.saturating_sub(limit),
        })
    }

    fn open_issues(&self, limit: usize) -> Result<ScanBoundedItems<Issue>, String> {
        let mut issues = self
            .snapshot
            .issues
            .iter()
            .filter(|issue| issue.status == IssueStatus::Open)
            .cloned()
            .collect::<Vec<_>>();
        let truncated = issues.len() > limit;
        issues.truncate(limit);
        Ok(ScanBoundedItems {
            items: issues,
            truncated,
        })
    }

    fn recent_commits(&self, limit: usize) -> Result<BoundedItems<Commit>, String> {
        let total = self.snapshot.commits.len();
        Ok(BoundedItems {
            items: self
                .snapshot
                .commits
                .iter()
                .rev()
                .take(limit)
                .cloned()
                .collect(),
            more: total.saturating_sub(limit),
        })
    }
}

impl AssociationCatalog for ProjectLaunchContextStore<'_> {
    fn validate_plan(&self, plan_id: u64) -> Result<(), String> {
        self.snapshot
            .plan(plan_id)
            .map(|_| ())
            .ok_or_else(|| "project plan is unavailable".to_owned())
    }

    fn task_plan(&self, task_id: u64) -> Result<u64, String> {
        self.snapshot
            .task(task_id)
            .map(|task| task.plan_id)
            .ok_or_else(|| "project task is unavailable".to_owned())
    }
}

pub(crate) fn confirm_linked_launch(
    ownership: AppResult<bool>,
    cleanup: impl FnOnce() -> AppResult<()>,
) -> AppResult<()> {
    let primary = match ownership {
        Ok(true) => return Ok(()),
        Ok(false) => {
            AppError::Message("linked terminal and AgentRun associations differ".to_owned())
        }
        Err(error) => error,
    };
    match cleanup() {
        Ok(()) => Err(primary),
        Err(cleanup) => Err(AppError::Message(format!("{primary}\n{cleanup}"))),
    }
}

fn parse_workflow_kind(value: &str) -> AppResult<AgentWorkflowKind> {
    match value {
        "validation" => Ok(AgentWorkflowKind::Validation),
        "commit" => Ok(AgentWorkflowKind::Commit),
        "pullRequest" => Ok(AgentWorkflowKind::PullRequest),
        "merge" => Ok(AgentWorkflowKind::Merge),
        _ => Err(AppError::Message(format!(
            "unsupported workflow kind {value:?}"
        ))),
    }
}
