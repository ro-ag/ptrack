//! The production project workspace, [`BoundDesktopWorkspace`].
//!
//! [`DesktopWorkspace::invoke`] parses one [`WorkspaceCommand`] and hands it
//! to the handler for its group, one child module each: `plans`, `tasks`,
//! `issues`, `project` (reads, the stack profile, the scratchpad),
//! `snapshot` (the bounded workspace snapshot), `terminals`, and `agents`.
//! `resources` holds the exact linked-resource checks destructive moves share.

mod agents;
mod issues;
mod plans;
mod project;
mod resources;
mod snapshot;
mod tasks;
mod terminals;

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use ptrack_agent::{AgentRuntimeSummary, IntelligenceState};
use ptrack_core::ProjectSnapshot;
use ptrack_store::ProjectStore;
use serde_json::Value;

use super::WORKSPACE_OPERATION_DRAIN_TIMEOUT;
use super::admission::{
    DesktopAdmissionFence, ResourceAdmissionFence, ResourceAdmissionGate, ResourceAdmissionLease,
    ResourceAdmissionState, WorkspaceCallGate, WorkspaceCallLease, WorkspaceCallState,
};
use super::command::WorkspaceCommand;
use super::ports::DesktopWorkspace;
use super::support::{lock, message, timestamp_expired};
use super::wire::{
    ActiveResourceSummary, DesktopNotificationEventV1, DesktopNotificationKindV1,
    DesktopNotificationSnapshotV1, FirstRunWorkspaceStateV1, WorkspaceProject, WorkspaceStatus,
};
use crate::diagnostics_report::CapabilityCountsV1;
use crate::{
    AgentRuntimeService, AppError, AppResult, ApplicationPort, LaunchedEventAuthority,
    LinkedAgentRuntimeHooks, ProjectEndpoint, TerminalRuntime, WorkspaceBindings,
};

#[cfg(test)]
pub(crate) use agents::confirm_linked_launch;
use tasks::TaskChallenge;

/// Composite authority needed by desktop linked-launch and exact-resource
/// coordination. Presentation callers never receive this object.
pub trait DesktopAgentRuntime:
    AgentRuntimeService + LaunchedEventAuthority + LinkedAgentRuntimeHooks + Send + Sync
{
}

impl<T> DesktopAgentRuntime for T where
    T: AgentRuntimeService + LaunchedEventAuthority + LinkedAgentRuntimeHooks + Send + Sync
{
}

/// Explicitly bound production workspace used by the desktop coordinator.
/// Construction takes attested bindings and never performs project discovery.
pub struct BoundDesktopWorkspace {
    generation: u64,
    initial_plan: u64,
    bindings: WorkspaceBindings,
    endpoint: ProjectEndpoint,
    application: Mutex<Box<dyn ApplicationPort + Send>>,
    terminal: Option<Arc<TerminalRuntime>>,
    agent: Option<Arc<dyn DesktopAgentRuntime>>,
    resource_transition: Mutex<()>,
    resource_admission: Arc<ResourceAdmissionGate>,
    workspace_calls: Arc<WorkspaceCallGate>,
    task_challenges: Mutex<BTreeMap<String, TaskChallenge>>,
    timeline: Mutex<Option<TimelineCache>>,
}

/// The repository timeline cached by the HEAD it was read at.
/// Git history cannot change until HEAD changes, so reuse the cached result.
struct TimelineCache {
    head: Option<String>,
    value: Value,
}

impl BoundDesktopWorkspace {
    /// Constructs an explicitly bound desktop workspace.
    ///
    /// # Panics
    /// Panics when the attested bindings omit their project endpoint.
    #[must_use]
    pub fn new(
        generation: u64,
        initial_plan: u64,
        bindings: WorkspaceBindings,
        application: Box<dyn ApplicationPort + Send>,
        terminal: Option<Arc<TerminalRuntime>>,
        agent: Option<Arc<dyn DesktopAgentRuntime>>,
    ) -> Self {
        let endpoint = bindings
            .project
            .clone()
            .expect("bound desktop workspace requires a project endpoint");
        Self {
            generation,
            initial_plan,
            bindings,
            endpoint,
            application: Mutex::new(application),
            terminal,
            agent,
            resource_transition: Mutex::new(()),
            resource_admission: Arc::new(ResourceAdmissionGate {
                state: Mutex::new(ResourceAdmissionState {
                    fences: 0,
                    pending: 0,
                    revision: 0,
                }),
            }),
            workspace_calls: Arc::new(WorkspaceCallGate {
                state: Mutex::new(WorkspaceCallState {
                    closing: false,
                    active: 0,
                }),
                idle: Condvar::new(),
            }),
            task_challenges: Mutex::new(BTreeMap::new()),
            timeline: Mutex::new(None),
        }
    }

    pub(crate) fn begin_resource_admission(&self) -> AppResult<ResourceAdmissionLease> {
        let mut state = lock(&self.resource_admission.state);
        if state.fences != 0 {
            return Err(AppError::Message(
                "workspace resource admission is fenced".to_owned(),
            ));
        }
        state.pending = state.pending.saturating_add(1);
        state.revision = state.revision.saturating_add(1);
        drop(state);
        Ok(ResourceAdmissionLease(Arc::clone(&self.resource_admission)))
    }

    pub(crate) fn begin_workspace_call(&self) -> AppResult<WorkspaceCallLease> {
        let mut state = lock(&self.workspace_calls.state);
        if state.closing {
            return Err(AppError::Message("workspace is closing".to_owned()));
        }
        state.active = state.active.saturating_add(1);
        drop(state);
        Ok(WorkspaceCallLease(Arc::clone(&self.workspace_calls)))
    }

    fn close_workspace_calls(&self) -> bool {
        let deadline = Instant::now() + WORKSPACE_OPERATION_DRAIN_TIMEOUT;
        let mut state = lock(&self.workspace_calls.state);
        state.closing = true;
        while state.active != 0 {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let (next, _) = self
                .workspace_calls
                .idle
                .wait_timeout(state, deadline.saturating_duration_since(now))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
        }
        let drained = state.active == 0;
        drop(state);
        drained
    }

    fn admission_revision(&self) -> u64 {
        lock(&self.resource_admission.state).revision
    }

    fn require_generation(&self, expected: u64) -> AppResult<()> {
        if expected != 0 && expected != self.generation {
            return Err(AppError::Message(format!(
                "stale workspace generation: expected {expected}, active {}",
                self.generation
            )));
        }
        Ok(())
    }

    fn require_exact_generation(&self, expected: u64) -> AppResult<()> {
        if expected == 0 || expected != self.generation {
            return Err(AppError::Message(format!(
                "stale workspace generation: expected {expected}, active {}",
                self.generation
            )));
        }
        Ok(())
    }

    const fn first_run_state(&self) -> FirstRunWorkspaceStateV1 {
        FirstRunWorkspaceStateV1 {
            status: WorkspaceStatus::Open,
            generation: self.generation,
        }
    }

    fn snapshot(&self) -> AppResult<ProjectSnapshot> {
        lock(&self.application).snapshot()
    }

    fn project_store(&self) -> AppResult<ProjectStore> {
        let actor = lock(&self.application).identity()?;
        Ok(ProjectStore::open_existing(
            &self.endpoint.database,
            &self.endpoint.binding,
            &self.bindings.writer_version,
        )?
        .with_actor(actor))
    }
}

/// The descriptor the retired capability broker published per project.
const RETIRED_CAPABILITY_DESCRIPTOR: &str = "capability-broker.json";

impl DesktopWorkspace for BoundDesktopWorkspace {
    fn capability_counts(&self) -> Option<CapabilityCountsV1> {
        let _workspace_call = self.begin_workspace_call().ok()?;
        let capabilities = self.project_store().ok()?.capabilities().ok()?;
        Some(CapabilityCountsV1 {
            granted: capabilities
                .iter()
                .filter(|capability| {
                    capability.enabled && !timestamp_expired(capability.expires_at)
                })
                .count(),
            total: capabilities.len(),
        })
    }

    /// Capability brokering is retired (it moved to pam): no broker serves a
    /// grant any more, so an enabled grant left by an older build authorizes
    /// nothing. Clearing app data still disables those records and removes the
    /// broker descriptor an older build published, so no stale grant or
    /// descriptor outlives the reset.
    fn revoke_capability_grants(&self) -> AppResult<usize> {
        let _workspace_call = self.begin_workspace_call()?;
        let revoked = self.project_store()?.revoke_capability_grants()?;
        ptrack_agent::remove_runtime_file(
            &self.bindings.global_home,
            &self.endpoint.root,
            RETIRED_CAPABILITY_DESCRIPTOR,
        )
        .map_err(message)?;
        Ok(revoked)
    }

    fn project(&self) -> WorkspaceProject {
        WorkspaceProject {
            name: self
                .endpoint
                .root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned(),
            root: self.endpoint.root.to_string_lossy().into_owned(),
            db_path: self.endpoint.database.to_string_lossy().into_owned(),
        }
    }

    fn notification_snapshot(&self) -> AppResult<DesktopNotificationSnapshotV1> {
        let _workspace_call = self.begin_workspace_call()?;
        let Some(agent) = &self.agent else {
            return Ok(DesktopNotificationSnapshotV1 {
                generation: self.generation,
                events: Vec::new(),
            });
        };
        let candidates = agent.agent_runtime_candidates(self.generation)?;
        let handoffs = agent.handoff_inbox(self.generation)?;
        let mut events = handoffs
            .items
            .into_iter()
            .map(|handoff| {
                let association = handoff.target_association.unwrap_or_default();
                DesktopNotificationEventV1 {
                    id: format!("handoff:{}", handoff.id),
                    kind: DesktopNotificationKindV1::HandoffArrival,
                    run_id: handoff.target_run_id,
                    plan_id: association.plan_id,
                    task_id: association.task_id,
                }
            })
            .collect::<Vec<_>>();
        for run in candidates.runs {
            let Some(intelligence) = run.intelligence.as_ref() else {
                continue;
            };
            let kind = match intelligence.state {
                IntelligenceState::Failed => DesktopNotificationKindV1::RunFailure,
                IntelligenceState::Completed => DesktopNotificationKindV1::RunCompletion,
                IntelligenceState::PotentiallyDrifting if run.live && run.association.is_some() => {
                    DesktopNotificationKindV1::RunDrift
                }
                _ => continue,
            };
            let association = run.association.unwrap_or_default();
            events.push(DesktopNotificationEventV1 {
                id: notification_evidence_id(&run, kind),
                kind,
                run_id: run.run_id,
                plan_id: association.plan_id,
                task_id: association.task_id,
            });
        }
        events.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(DesktopNotificationSnapshotV1 {
            generation: self.generation,
            events,
        })
    }

    fn invoke(&self, method: &str, arguments: &[Value]) -> AppResult<Value> {
        let _workspace_call = self.begin_workspace_call()?;
        match WorkspaceCommand::parse(method, arguments)? {
            WorkspaceCommand::Plan(command) => self.plan_command(command),
            WorkspaceCommand::Task(command) => self.task_command(command),
            WorkspaceCommand::Issue(command) => self.issue_command(command),
            WorkspaceCommand::Project(command) => self.project_command(command),
            WorkspaceCommand::Scratchpad(command) => self.scratchpad_command(command),
            WorkspaceCommand::Terminal(command) => self.terminal_command(command),
            WorkspaceCommand::Agent(command) => self.agent_command(command),
        }
    }

    fn active_resources(&self) -> AppResult<ActiveResourceSummary> {
        let (terminals, terminal_revision) =
            self.terminal.as_ref().map_or(Ok((0, 0)), |terminal| {
                Ok::<_, AppError>((
                    terminal.active_session_count(self.generation)?,
                    terminal.resource_revision(self.generation)?,
                ))
            })?;
        let (agent_runs, agent_revision) = self.agent.as_ref().map_or(Ok((0, 0)), |agent| {
            let state = agent.resource_state(self.generation)?;
            Ok::<_, AppError>((state.active_runs, state.resource_revision))
        })?;
        let admission = lock(&self.resource_admission.state);
        let pending_admissions = admission.pending;
        let admission_revision = admission.revision;
        drop(admission);
        Ok(ActiveResourceSummary {
            terminals,
            agent_runs,
            pending_admissions,
            resource_revision: terminal_revision
                .saturating_add(agent_revision)
                .saturating_add(admission_revision),
        })
    }

    fn fence_resource_admission(&self) -> AppResult<DesktopAdmissionFence> {
        {
            let mut state = lock(&self.resource_admission.state);
            state.fences = state.fences.saturating_add(1);
        }
        let resource = ResourceAdmissionFence(Arc::clone(&self.resource_admission));
        let agent = self
            .agent
            .as_ref()
            .map(|agent| agent.fence_admission(self.generation))
            .transpose()?;
        Ok(DesktopAdmissionFence {
            _resource: Some(resource),
            _agent: agent,
        })
    }

    fn drain_runtime_invalidations(&self) -> AppResult<bool> {
        self.agent.as_ref().map_or(Ok(false), |agent| {
            Ok(agent.drain_invalidations(self.generation)?.event_count != 0)
        })
    }

    fn shutdown(&self) -> AppResult<()> {
        let mut errors = Vec::new();
        if !self.close_workspace_calls() {
            errors.push("workspace runtime calls did not stop before cleanup deadline".to_owned());
        }
        if let Some(terminal) = &self.terminal
            && let Err(error) = shutdown_terminal(Arc::clone(terminal))
        {
            errors.push(error.to_string());
        }
        if let Some(agent) = &self.agent
            && let Err(error) = agent.shutdown()
        {
            errors.push(error.to_string());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(AppError::Message(errors.join("\n")))
        }
    }
}

fn notification_evidence_id(run: &AgentRuntimeSummary, kind: DesktopNotificationKindV1) -> String {
    let intelligence = run
        .intelligence
        .as_ref()
        .expect("notification evidence requires intelligence");
    let observed = intelligence
        .last_event_at
        .as_ref()
        .and_then(|value| serde_json::to_string(value).ok())
        .unwrap_or_else(|| "none".to_owned());
    let association_revision = run.association.map_or(0, |value| value.revision);
    format!(
        "run:{}:{kind:?}:{association_revision}:{}:{}:{observed}",
        run.run_id, intelligence.evidence_count, intelligence.event_count
    )
}

fn shutdown_terminal(terminal: Arc<TerminalRuntime>) -> AppResult<()> {
    std::thread::Builder::new()
        .name("ptrack-terminal-desktop-shutdown".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(AppError::Io)?;
            runtime.block_on(terminal.shutdown())
        })
        .map_err(AppError::Io)?
        .join()
        .map_err(|_| AppError::Message("terminal shutdown worker failed".to_owned()))?
}
