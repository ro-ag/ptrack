//! Exact linked-resource checks: the live terminals and agent runs a task or
//! plan owns, read from one consistent snapshot of each runtime so a
//! destructive move can refuse while anything is still attached.

use std::path::Path;

use ptrack_agent::{LeaseState, ProcessState, RegistrationKind, Run, RunState};
use ptrack_core::ProjectSnapshot;
use ptrack_terminal::{SessionInfo, SessionState, TerminalAssociation};

use super::super::TASK_RESOURCE_LIMIT;
use super::BoundDesktopWorkspace;
use crate::{AppError, AppResult};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct TaskResource {
    pub(super) kind: &'static str,
    pub(super) id: String,
    pub(super) revision: u64,
    pub(super) state: String,
    pub(super) process_state: String,
    pub(super) lease_state: String,
    pub(super) lifecycle_revision: u64,
}

impl BoundDesktopWorkspace {
    pub(super) fn resource_revisions(&self) -> AppResult<(u64, u64)> {
        let terminal = self.terminal.as_ref().map_or(Ok(0), |terminal| {
            terminal.resource_revision(self.generation)
        })?;
        let agent = self
            .agent
            .as_ref()
            .map_or(Ok(0), |agent| agent.resource_revision(self.generation))?;
        Ok((terminal, agent))
    }

    pub(super) fn with_exact_task_resources<T>(
        &self,
        snapshot: &ProjectSnapshot,
        task_id: u64,
        use_resources: impl FnOnce(Vec<TaskResource>) -> AppResult<T>,
    ) -> AppResult<T> {
        self.with_exact_resources(snapshot, ResourceScope::Task(task_id), use_resources)
    }

    /// The live terminals and agents linked to a plan or to any of its tasks,
    /// read from the same exact snapshots a task move uses.
    pub(super) fn with_exact_plan_resources<T>(
        &self,
        snapshot: &ProjectSnapshot,
        plan_id: u64,
        use_resources: impl FnOnce(Vec<TaskResource>) -> AppResult<T>,
    ) -> AppResult<T> {
        self.with_exact_resources(snapshot, ResourceScope::Plan(plan_id), use_resources)
    }

    pub(super) fn with_exact_resources<T>(
        &self,
        snapshot: &ProjectSnapshot,
        scope: ResourceScope,
        use_resources: impl FnOnce(Vec<TaskResource>) -> AppResult<T>,
    ) -> AppResult<T> {
        let mut use_resources = Some(use_resources);
        let mut run = |sessions: &[SessionInfo]| {
            let terminal_resources = terminal_task_resources(snapshot, scope, sessions);
            if let Some(agent) = &self.agent {
                let mut output = None;
                let mut callback = |runs: &[Run]| {
                    let mut resources = terminal_resources.clone();
                    resources.extend(agent_task_resources(
                        snapshot,
                        &self.endpoint.root,
                        self.generation,
                        scope,
                        runs,
                    ));
                    resources.sort();
                    output = Some(use_resources
                        .take()
                        .expect("exact task resource callback is single-use")(
                        resources
                    ));
                };
                agent.with_exact_runtime_snapshot(
                    self.generation,
                    TASK_RESOURCE_LIMIT,
                    &mut callback,
                )?;
                output.ok_or_else(|| {
                    AppError::Message("exact AgentRun resource snapshot is unavailable".to_owned())
                })?
            } else {
                use_resources
                    .take()
                    .expect("exact task resource callback is single-use")(
                    terminal_resources
                )
            }
        };
        if let Some(terminal) = &self.terminal {
            terminal.with_exact_session_snapshot(self.generation, run)
        } else {
            run(&[])
        }
    }
}

/// Which linked resources one exact resource check covers.
#[derive(Clone, Copy, Debug)]
pub(super) enum ResourceScope {
    /// Resources linked to exactly this task.
    Task(u64),
    /// Resources linked to this plan or to any task in it.
    Plan(u64),
}

impl ResourceScope {
    fn covers(self, snapshot: &ProjectSnapshot, plan_id: u64, task_id: u64) -> bool {
        match self {
            Self::Task(wanted) => {
                task_id == wanted && snapshot.task(wanted).map(|task| task.plan_id) == Some(plan_id)
            }
            Self::Plan(wanted) => plan_id == wanted,
        }
    }
}

pub(super) fn terminal_task_resources(
    snapshot: &ProjectSnapshot,
    scope: ResourceScope,
    sessions: &[SessionInfo],
) -> Vec<TaskResource> {
    sessions
        .iter()
        .filter(|session| {
            matches!(
                session.state,
                SessionState::Starting | SessionState::Running | SessionState::Closing
            )
        })
        .filter_map(|session| {
            let association = session.association.as_ref()?;
            (association.revision != 0
                && scope.covers(
                    snapshot,
                    association.pointer.plan_id,
                    association.pointer.task_id,
                ))
            .then(|| TaskResource {
                kind: "terminal",
                id: session.id.clone(),
                revision: association.revision,
                state: session.state.to_string(),
                process_state: String::new(),
                lease_state: String::new(),
                lifecycle_revision: 0,
            })
        })
        .collect()
}

pub(super) fn agent_task_resources(
    snapshot: &ProjectSnapshot,
    project_root: &Path,
    generation: u64,
    scope: ResourceScope,
    runs: &[Run],
) -> Vec<TaskResource> {
    runs.iter()
        .filter(|run| agent_run_is_live(run))
        .filter_map(|run| {
            let association = run.association.as_ref()?;
            (association.version == 1
                && association.project_root == project_root.to_string_lossy()
                && association.generation == generation
                && association.live_id == run.id
                && association.revision != 0
                && scope.covers(
                    snapshot,
                    association.target.plan_id,
                    association.target.task_id,
                ))
            .then(|| TaskResource {
                kind: "agent",
                id: run.id.clone(),
                revision: association.revision,
                state: run.state.as_str().to_owned(),
                process_state: run.process_state.as_str().to_owned(),
                lease_state: run.lease_state.as_str().to_owned(),
                lifecycle_revision: run.lifecycle_revision,
            })
        })
        .collect()
}

pub(super) fn agent_run_is_live(run: &Run) -> bool {
    if run.state != RunState::Running || run.process_state == ProcessState::Exited {
        return false;
    }
    match run.registration_kind {
        RegistrationKind::External => run.lease_state == LeaseState::Active,
        RegistrationKind::Launched => run.process_state == ProcessState::Running,
        RegistrationKind::Unset => false,
    }
}

pub(super) fn agent_association(
    project_root: &Path,
    generation: u64,
    live_id: &str,
    association: &TerminalAssociation,
) -> ptrack_agent::Association {
    ptrack_agent::Association {
        version: association.pointer.version,
        project_root: project_root.to_string_lossy().into_owned(),
        generation,
        live_id: live_id.to_owned(),
        target: ptrack_agent::AssociationTarget {
            plan_id: association.pointer.plan_id,
            task_id: association.pointer.task_id,
        },
        revision: association.revision,
    }
}
