//! The production desktop workspace factory and the adapters it wires a
//! bound workspace from: terminal coordination sessions for the agent runtime
//! and a silent terminal event sink for headless use.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ptrack_agent::{Association, AssociationTarget, CoordinationSession, CoordinationSessions};
use ptrack_terminal::{
    Manager, ProfileKind, discover_profiles, load_profile_config_if_exists, merge_profiles,
    profile_config_path,
};
use serde_json::Value;

use super::lock;
use super::runtime::ActiveRuntime;
use crate::{
    AgentRuntime, AgentRuntimeConfig, AppError, AppResult, BoundDesktopWorkspace,
    DesktopAgentRuntime, DesktopEventSink, DesktopTerminalEventSink, DesktopWorkspace,
    DesktopWorkspaceFactory, LocalApplication, ProductionTerminalIdentityAuthority,
    TerminalAgentAuthority, TerminalEventSink, TerminalIdentityAuthority, TerminalRuntime,
    TerminalRuntimeConfig, WorkspaceProject,
};

pub struct ProductionDesktopWorkspaceFactory {
    /// Replaceable so a marker reloaded after the command line registered a
    /// project serves new workspaces without discarding `async_runtime`,
    /// which the open workspace's terminal manager still runs on.
    runtime: Mutex<Arc<ActiveRuntime>>,
    events: Option<Arc<dyn DesktopEventSink>>,
    async_runtime: tokio::runtime::Runtime,
    initial_plan: u64,
}

impl ProductionDesktopWorkspaceFactory {
    /// Constructs a production factory with a persistent asynchronous runtime.
    ///
    /// # Errors
    /// Returns an error when the terminal runtime cannot be created.
    pub fn new(
        runtime: Arc<ActiveRuntime>,
        events: Option<Arc<dyn DesktopEventSink>>,
        initial_plan: u64,
    ) -> AppResult<Arc<Self>> {
        let async_runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|_| AppError::Message("terminal runtime is unavailable".to_owned()))?;
        Ok(Arc::new(Self {
            runtime: Mutex::new(runtime),
            events,
            async_runtime,
            initial_plan,
        }))
    }

    pub(super) fn replace_runtime(&self, runtime: Arc<ActiveRuntime>) {
        *lock(&self.runtime) = runtime;
    }
}

impl DesktopWorkspaceFactory for ProductionDesktopWorkspaceFactory {
    fn build(&self, root: &Path, generation: u64) -> AppResult<Arc<dyn DesktopWorkspace>> {
        let runtime = Arc::clone(&lock(&self.runtime));
        let bindings = runtime.bindings_for_exact_root(root)?;
        let endpoint = bindings.project.clone().ok_or(AppError::NoProject)?;
        let discovered =
            discover_profiles().map_err(|error| AppError::Message(error.to_string()))?;
        let configured = load_profile_config_if_exists(&profile_config_path(&bindings.global_home))
            .map_err(|error| AppError::Message(error.to_string()))?
            .map_or_else(Vec::new, |config| config.profiles);
        let profiles = merge_profiles(&discovered, &configured)
            .map_err(|error| AppError::Message(error.to_string()))?;
        let manager = self
            .async_runtime
            .block_on(Manager::native(&endpoint.root, profiles))
            .map_err(|error| AppError::Message(error.to_string()))?;
        let sessions: Arc<dyn CoordinationSessions> = Arc::new(TerminalCoordinationSessions {
            manager: Arc::clone(&manager),
            project_root: endpoint.root.clone(),
            generation,
        });
        let agent = Arc::new(AgentRuntime::start(AgentRuntimeConfig::production(
            generation,
            endpoint.clone(),
            bindings.global_home.clone(),
            bindings.global_database.clone(),
            bindings.global_binding.clone(),
            bindings.writer_version.clone(),
            sessions,
        ))?);
        let terminal_agent: Arc<dyn TerminalAgentAuthority> = agent.clone();
        let identity: Arc<dyn TerminalIdentityAuthority> = Arc::new(
            ProductionTerminalIdentityAuthority::new(Some(terminal_agent)),
        );
        let terminal_events: Arc<dyn TerminalEventSink> = self.events.as_ref().map_or_else(
            || Arc::new(SilentTerminalEvents) as Arc<dyn TerminalEventSink>,
            |sink| DesktopTerminalEventSink::new(Arc::clone(sink)),
        );
        let terminal = TerminalRuntime::new(TerminalRuntimeConfig {
            generation,
            project_root: endpoint.root.clone(),
            manager,
            identity,
            events: terminal_events,
            attachment_lease: Duration::from_secs(30),
        })?;
        let desktop_agent: Arc<dyn DesktopAgentRuntime> = agent;
        let inner = BoundDesktopWorkspace::new(
            generation,
            self.initial_plan,
            bindings.clone(),
            Box::new(LocalApplication::new(bindings)),
            Some(terminal),
            Some(desktop_agent),
        );
        Ok(Arc::new(ProductionDesktopWorkspace {
            inner,
            _runtime: runtime,
        }))
    }
}

struct TerminalCoordinationSessions {
    manager: Arc<Manager>,
    project_root: PathBuf,
    generation: u64,
}

impl CoordinationSessions for TerminalCoordinationSessions {
    fn snapshot(&self, limit: usize) -> (Vec<CoordinationSession>, usize) {
        let (sessions, total) = self.manager.runtime_session_snapshot_bounded(limit);
        let sessions = sessions
            .into_iter()
            .map(|session| CoordinationSession {
                id: session.id.clone(),
                profile_kind: match session.profile_kind {
                    ProfileKind::Shell => "shell",
                    ProfileKind::Agent => "agent",
                }
                .to_owned(),
                state: session.state.to_string(),
                association: session.association.map(|association| Association {
                    version: association.pointer.version,
                    project_root: self.project_root.to_string_lossy().into_owned(),
                    generation: self.generation,
                    live_id: session.id,
                    target: AssociationTarget {
                        plan_id: association.pointer.plan_id,
                        task_id: association.pointer.task_id,
                    },
                    revision: association.revision,
                }),
            })
            .collect();
        (sessions, total)
    }
}

struct SilentTerminalEvents;

impl TerminalEventSink for SilentTerminalEvents {
    fn status(&self, _: crate::TerminalStatusV2) {}
    fn exited(&self, _: crate::TerminalExitV2) {}
    fn runtime_changed(&self, _: u64) {}
}

struct ProductionDesktopWorkspace {
    inner: BoundDesktopWorkspace,
    _runtime: Arc<ActiveRuntime>,
}

impl DesktopWorkspace for ProductionDesktopWorkspace {
    fn project(&self) -> WorkspaceProject {
        self.inner.project()
    }

    fn invoke(&self, method: &str, arguments: &[Value]) -> AppResult<Value> {
        self.inner.invoke(method, arguments)
    }

    fn active_resources(&self) -> AppResult<crate::ActiveResourceSummary> {
        self.inner.active_resources()
    }

    fn fence_resource_admission(&self) -> AppResult<crate::DesktopAdmissionFence> {
        self.inner.fence_resource_admission()
    }

    fn drain_runtime_invalidations(&self) -> AppResult<bool> {
        self.inner.drain_runtime_invalidations()
    }

    fn notification_snapshot(&self) -> AppResult<crate::DesktopNotificationSnapshotV1> {
        self.inner.notification_snapshot()
    }

    fn capability_counts(&self) -> Option<crate::diagnostics_report::CapabilityCountsV1> {
        self.inner.capability_counts()
    }

    fn revoke_capability_grants(&self) -> AppResult<usize> {
        self.inner.revoke_capability_grants()
    }

    fn shutdown(&self) -> AppResult<()> {
        self.inner.shutdown()
    }
}
