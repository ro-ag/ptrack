use std::cell::Cell;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ptrack_agent::{
    AdmissionFence, AgentActivitySnapshot, AgentHandoffAcknowledgementV2, AgentHandoffEnvelopeV2,
    AgentHandoffV2, AgentIntelligenceV2, AgentNotification, AgentOwnershipMutationV2, AgentRunsV2,
    AgentWorkflowDismissalV2, AgentWorkflowKind, AgentWorkflowProposalV2, AgentWorktreeMutationV2,
    Association, AssociationCatalog, AssociationHost, AssociationPointer, BoundedSnapshot,
    CoordinationConfig, CoordinationDecision, CoordinationError, CoordinationGit,
    CoordinationGitSnapshot, CoordinationIssue, CoordinationSessions, CoordinationStore,
    CoordinationTarget, CoordinationTaskContext, Coordinator, ExistingWorktree, GitBranch,
    GitCommit, GitDivergence, IntegrationConfig, IntegrationServer, LinkedAssociationChange,
    Registration, Registry, RegistryConfig, RegistryMutationOutcome, Run, RuntimeAssociation,
    Timestamp, WorktreeIdentity, run_history_path,
};
use ptrack_core::{IssueStatus, MemoryKind, NoteTarget};
use ptrack_git::{CancellationToken, RepositoryService, RepositoryState, Snapshot};
use ptrack_store::{ActiveBinding, GlobalStore, ProjectStore, StoreKind};
use serde::Serialize;

use crate::{AppError, AppResult, ProjectEndpoint};

const INVALIDATION_CAPACITY: usize = 1_024;
const STORE_SUGGESTION_LIMIT: usize = 50;
const DEFAULT_OPERATION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_REGISTRY_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// Starts and stops the loopback integration without exposing its descriptor
/// registration secret to application or presentation code.
pub trait AgentIntegration: Send + Sync {
    fn event_endpoint(&self) -> &str;
    /// Stops the listener within `timeout` and compare-removes only its owned
    /// descriptor. Implementations must not detach work on timeout.
    ///
    /// # Errors
    /// Returns a content-free server, thread, or descriptor cleanup error.
    fn shutdown(&self, timeout: Duration) -> Result<(), String>;
}

/// Injectable integration construction seam. Tests can prove lifecycle order
/// without opening a listener or consulting an ambient home directory.
pub trait AgentIntegrationFactory: Send + Sync {
    /// Starts one generation-scoped integration service.
    ///
    /// # Errors
    /// Returns a content-free startup error.
    fn start(
        &self,
        registry: Arc<Registry>,
        config: IntegrationConfig,
    ) -> Result<Box<dyn AgentIntegration>, String>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProductionAgentIntegrationFactory;

impl AgentIntegrationFactory for ProductionAgentIntegrationFactory {
    fn start(
        &self,
        registry: Arc<Registry>,
        config: IntegrationConfig,
    ) -> Result<Box<dyn AgentIntegration>, String> {
        IntegrationServer::start(registry, config)
            .map(|server| Box::new(server) as Box<dyn AgentIntegration>)
            .map_err(|error| error.to_string())
    }
}

impl AgentIntegration for IntegrationServer {
    fn event_endpoint(&self) -> &str {
        self.event_endpoint()
    }

    fn shutdown(&self, timeout: Duration) -> Result<(), String> {
        self.shutdown_timeout(timeout)
            .map_err(|error| error.to_string())
    }
}

/// Explicit construction inputs for one already-selected project generation.
/// No default home, project discovery, database, or terminal authority exists.
pub struct AgentRuntimeConfig {
    pub generation: u64,
    pub endpoint: ProjectEndpoint,
    pub global_home: PathBuf,
    pub global_database: PathBuf,
    pub global_binding: ActiveBinding,
    pub writer_version: String,
    pub sessions: Arc<dyn CoordinationSessions>,
    pub git: Arc<dyn CoordinationGit>,
    pub git_cancellation: Option<CancellationToken>,
    pub integration_factory: Arc<dyn AgentIntegrationFactory>,
    pub operation_shutdown_timeout: Duration,
    pub integration_shutdown_timeout: Duration,
    pub registry_shutdown_timeout: Duration,
}

impl AgentRuntimeConfig {
    /// Builds explicit production adapters while leaving session observation
    /// host-owned until the terminal slice supplies it.
    #[must_use]
    pub fn production(
        generation: u64,
        endpoint: ProjectEndpoint,
        global_home: PathBuf,
        global_database: PathBuf,
        global_binding: ActiveBinding,
        writer_version: impl Into<String>,
        sessions: Arc<dyn CoordinationSessions>,
    ) -> Self {
        let git = Arc::new(PtrackCoordinationGit::new());
        Self {
            generation,
            endpoint,
            global_home,
            global_database,
            global_binding,
            writer_version: writer_version.into(),
            sessions,
            git_cancellation: Some(git.cancellation.clone()),
            git,
            integration_factory: Arc::new(ProductionAgentIntegrationFactory),
            operation_shutdown_timeout: DEFAULT_OPERATION_SHUTDOWN_TIMEOUT,
            integration_shutdown_timeout: Duration::from_secs(2),
            registry_shutdown_timeout: DEFAULT_REGISTRY_SHUTDOWN_TIMEOUT,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentResourceStateV2 {
    pub generation: u64,
    pub resource_revision: u64,
    pub active_runs: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInvalidationV2 {
    pub generation: u64,
    pub resource_revision: u64,
    pub event_count: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentNotificationsV2 {
    pub generation: u64,
    pub items: Vec<AgentNotification>,
    pub bounds: BoundedSnapshot,
    pub incomplete: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorkflowTargetsV2 {
    pub generation: u64,
    pub items: Vec<String>,
    pub incomplete: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMutationOutcome {
    pub matched: bool,
    pub changed: bool,
}

impl From<RegistryMutationOutcome> for AgentMutationOutcome {
    fn from(value: RegistryMutationOutcome) -> Self {
        Self {
            matched: value.matched,
            changed: value.changed,
        }
    }
}

/// Opaque, app-owned admission fence. Dropping or consuming it releases the
/// registry fence without exposing registry access.
pub struct AgentAdmissionFence(Option<AdmissionFence>);

impl AgentAdmissionFence {
    pub fn release(mut self) {
        if let Some(fence) = self.0.take() {
            fence.release();
        }
    }
}

thread_local! {
    static HOST_INVALIDATION_SUPPRESSIONS: Cell<usize> = const { Cell::new(0) };
}

/// Thread-affine guard for a combined mutation whose single frontend event is
/// owned by the terminal runtime. Resource revisions still advance.
pub struct AgentRuntimeEventSuppression {
    active: bool,
}

impl AgentRuntimeEventSuppression {
    fn new() -> Self {
        HOST_INVALIDATION_SUPPRESSIONS.with(|count| count.set(count.get().saturating_add(1)));
        Self { active: true }
    }
}

impl Drop for AgentRuntimeEventSuppression {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        HOST_INVALIDATION_SUPPRESSIONS.with(|count| count.set(count.get().saturating_sub(1)));
        self.active = false;
    }
}

/// Opaque exact-pair association transaction prepared for a linked terminal.
pub struct LinkedAgentAssociationChange(LinkedAssociationChange);

impl LinkedAgentAssociationChange {
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.0.run_id
    }

    #[must_use]
    pub fn terminal_id(&self) -> &str {
        &self.0.terminal_id
    }
}

/// UI-neutral, generation-fenced service contract. It exposes only bounded
/// projections and proposal state; approval never executes a workflow.
#[allow(clippy::missing_errors_doc)]
pub trait AgentRuntimeService {
    fn resource_state(&self, generation: u64) -> AppResult<AgentResourceStateV2>;
    fn resource_revision(&self, generation: u64) -> AppResult<u64>;
    fn drain_invalidations(&self, generation: u64) -> AppResult<AgentInvalidationV2>;
    fn agent_runs(&self, generation: u64) -> AppResult<AgentRunsV2>;
    fn agent_runtime_candidates(
        &self,
        generation: u64,
    ) -> AppResult<ptrack_agent::AgentRuntimeCandidatesV2>;
    fn workspace_snapshot(
        &self,
        generation: u64,
        git: &CoordinationGitSnapshot,
        deadline: Instant,
    ) -> AppResult<ptrack_agent::AgentWorkspaceSnapshotV2>;
    fn agent_intelligence(&self, generation: u64, run_id: &str) -> AppResult<AgentIntelligenceV2>;
    fn activity(&self, generation: u64) -> AppResult<AgentActivitySnapshot>;
    fn notifications(&self, generation: u64) -> AppResult<AgentNotificationsV2>;
    fn drift(&self, generation: u64) -> AppResult<ptrack_agent::DriftSnapshot>;
    fn preview_handoff(&self, generation: u64, run_id: &str) -> AppResult<AgentHandoffV2>;
    fn set_task_ownership(
        &self,
        generation: u64,
        run_id: &str,
        expected_association_revision: u64,
        owned: bool,
    ) -> AppResult<AgentOwnershipMutationV2>;
    fn set_worktree(
        &self,
        generation: u64,
        run_id: &str,
        expected_association_revision: u64,
        root: &str,
        associated: bool,
    ) -> AppResult<AgentWorktreeMutationV2>;
    fn send_handoff(
        &self,
        generation: u64,
        source_run_id: &str,
        target_run_id: &str,
        expected_source_revision: u64,
        expected_target_revision: u64,
    ) -> AppResult<AgentHandoffEnvelopeV2>;
    fn acknowledge_handoff(
        &self,
        generation: u64,
        id: &str,
        target_run_id: &str,
    ) -> AppResult<AgentHandoffAcknowledgementV2>;
    fn prepare_workflow(
        &self,
        generation: u64,
        run_id: &str,
        expected_association_revision: u64,
        kind: AgentWorkflowKind,
        target_branch: &str,
    ) -> AppResult<AgentWorkflowProposalV2>;
    fn approve_workflow(&self, generation: u64, id: &str) -> AppResult<AgentWorkflowProposalV2>;
    fn dismiss_workflow(&self, generation: u64, id: &str) -> AppResult<AgentWorkflowDismissalV2>;
    fn workflow_targets(&self, generation: u64) -> AppResult<AgentWorkflowTargetsV2>;
    fn with_exact_runtime_snapshot(
        &self,
        generation: u64,
        maximum: usize,
        use_snapshot: &mut dyn FnMut(&[Run]),
    ) -> AppResult<()>;
    fn shutdown(&self) -> AppResult<()>;
}

/// Narrow launch hook for the future terminal host. Tokens remain opaque and
/// association pointers remain descriptive; this interface spawns nothing.
#[allow(clippy::missing_errors_doc)]
pub trait LaunchedEventAuthority {
    fn event_endpoint(&self, generation: u64) -> AppResult<String>;
    fn issue_launched_event_token(&self, generation: u64) -> AppResult<String>;
    fn bind_launched_event_token(
        &self,
        generation: u64,
        token: &str,
        run_id: &str,
    ) -> AppResult<()>;
    fn revoke_launched_event_token(&self, generation: u64, token: &str) -> AppResult<bool>;
    fn register_launched(&self, generation: u64, registration: Registration) -> AppResult<Run>;
    fn register_linked_launched(
        &self,
        generation: u64,
        registration: Registration,
        pointer: AssociationPointer,
    ) -> AppResult<Run>;
    fn associate_run(
        &self,
        generation: u64,
        run_id: &str,
        pointer: AssociationPointer,
    ) -> AppResult<Association>;
    fn rollback_launched(
        &self,
        generation: u64,
        run_id: &str,
        terminal_id: &str,
    ) -> AppResult<bool>;
}

/// Narrow #70 lifecycle seam. It exposes exact outcomes and opaque guards/CAS
/// records, never the underlying registry or its authority-bearing tokens.
#[allow(clippy::missing_errors_doc)]
pub trait LinkedAgentRuntimeHooks {
    fn suppress_runtime_event(&self, generation: u64) -> AppResult<AgentRuntimeEventSuppression>;
    fn fence_admission(&self, generation: u64) -> AppResult<AgentAdmissionFence>;
    fn rollback_linked_launched(
        &self,
        generation: u64,
        run_id: &str,
        terminal_id: &str,
    ) -> AppResult<bool>;
    fn rollback_linked_terminal(&self, generation: u64, terminal_id: &str) -> AppResult<usize>;
    fn has_linked_terminal(&self, generation: u64, terminal_id: &str) -> AppResult<bool>;
    fn revoke_terminal_event_tokens(&self, generation: u64, terminal_id: &str) -> AppResult<bool>;
    fn record_terminal_activity(
        &self,
        generation: u64,
        terminal_id: &str,
        activity_at: Timestamp,
    ) -> AppResult<AgentMutationOutcome>;
    fn record_terminal_exit(
        &self,
        generation: u64,
        terminal_id: &str,
        code: i32,
        result: &str,
    ) -> AppResult<AgentMutationOutcome>;
    fn prepare_linked_association(
        &self,
        generation: u64,
        terminal_id: &str,
        terminal_previous: Option<&Association>,
        terminal_next: &Association,
        pointer: AssociationPointer,
    ) -> AppResult<Option<LinkedAgentAssociationChange>>;
    fn commit_linked_association(
        &self,
        generation: u64,
        change: &LinkedAgentAssociationChange,
    ) -> AppResult<()>;
    fn rollback_linked_association(
        &self,
        generation: u64,
        change: &LinkedAgentAssociationChange,
    ) -> AppResult<()>;
}

struct GateState {
    closing: bool,
    operations: usize,
    finished: bool,
    failure: Option<String>,
}

struct RuntimeGate {
    generation: u64,
    state: Mutex<GateState>,
    wake: Condvar,
}

struct OperationGuard {
    gate: Arc<RuntimeGate>,
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.gate.state);
        state.operations = state.operations.saturating_sub(1);
        if state.operations == 0 {
            self.gate.wake.notify_all();
        }
    }
}

/// Owns all resources for one published project generation. The owner retains
/// no project-store handle and holds no lifecycle lock while calling a store,
/// Git, integration, registry, or projection adapter.
pub struct AgentRuntime {
    generation: u64,
    registry: Arc<Registry>,
    coordinator: Coordinator,
    catalog: ProjectCoordinationStore,
    integration: Box<dyn AgentIntegration>,
    git_cancellation: Option<CancellationToken>,
    invalidation_sender: SyncSender<()>,
    invalidation_receiver: Mutex<Receiver<()>>,
    resource_revision: Arc<AtomicU64>,
    operation_shutdown_timeout: Duration,
    integration_shutdown_timeout: Duration,
    registry_shutdown_timeout: Duration,
    gate: Arc<RuntimeGate>,
}

impl AgentRuntime {
    /// Starts a complete generation or fails without publishing a usable owner.
    ///
    /// # Errors
    /// Returns invalid configuration, history-path, or integration startup errors.
    #[allow(clippy::too_many_lines)] // Ordered construction must unwind one unpublished candidate.
    pub fn start(config: AgentRuntimeConfig) -> AppResult<Self> {
        if config.generation == 0 {
            return Err(AppError::Message(
                "AgentRun workspace generation must be nonzero".to_owned(),
            ));
        }
        let endpoint = validated_endpoint(&config.endpoint)?;
        let global_home = validated_global_attestation(
            &config.global_home,
            &config.global_database,
            &config.global_binding,
            endpoint.binding.generation,
        )?;
        // Prove the exact activation binding before any listener, descriptor,
        // sweeper, or in-memory generation is created. The handle is not kept.
        drop(ProjectStore::open_existing(
            &endpoint.database,
            &endpoint.binding,
            &config.writer_version,
        )?);
        drop(GlobalStore::open_existing(
            &config.global_database,
            &config.global_binding,
        )?);
        let state_path = run_history_path(&global_home, &endpoint.root)
            .map_err(|error| AppError::Message(error.to_string()))?;
        let cwd_project_root = endpoint.root.clone();
        let cwd_git = RepositoryService::new();
        let registry = Arc::new(Registry::new(RegistryConfig {
            project_root: endpoint.root.clone(),
            state_path,
            additional_cwd_validator: Some(Arc::new(move |candidate| {
                let cancellation = CancellationToken::new();
                cwd_git
                    .inspect_worktree(&cancellation, &cwd_project_root, candidate)
                    .is_ok_and(|identity| candidate.starts_with(Path::new(&identity.root)))
            })),
            ..RegistryConfig::default()
        }));
        let catalog = ProjectCoordinationStore::new(
            endpoint.clone(),
            config.writer_version,
            config.generation,
        );
        let store: Arc<dyn CoordinationStore> = Arc::new(catalog.clone());
        let (invalidation_sender, invalidation_receiver) = sync_channel(INVALIDATION_CAPACITY);
        let mutation_revision = Arc::new(AtomicU64::new(0));
        let coordinator = Coordinator::new(CoordinationConfig {
            generation: config.generation,
            project_root: endpoint.root.clone(),
            registry: Arc::clone(&registry),
            store,
            git: Arc::clone(&config.git),
            sessions: config.sessions,
            now: None,
            random: None,
            mutation_revision: Some(Arc::clone(&mutation_revision)),
            runtime_changed: Some(invalidation_sender.clone()),
        });
        let integration = match config.integration_factory.start(
            Arc::clone(&registry),
            IntegrationConfig {
                global_home,
                project_root: endpoint.root,
                generation: config.generation,
                mutation_revision: Some(Arc::clone(&mutation_revision)),
                runtime_changed: Some(invalidation_sender.clone()),
            },
        ) {
            Ok(integration) => integration,
            Err(error) => {
                coordinator.shutdown();
                let _ = registry.shutdown();
                return Err(AppError::Message(error));
            }
        };
        Ok(Self {
            generation: config.generation,
            registry,
            coordinator,
            catalog,
            integration,
            git_cancellation: config.git_cancellation,
            invalidation_sender,
            invalidation_receiver: Mutex::new(invalidation_receiver),
            resource_revision: mutation_revision,
            operation_shutdown_timeout: positive_timeout(
                config.operation_shutdown_timeout,
                DEFAULT_OPERATION_SHUTDOWN_TIMEOUT,
            ),
            integration_shutdown_timeout: positive_timeout(
                config.integration_shutdown_timeout,
                Duration::from_secs(2),
            ),
            registry_shutdown_timeout: positive_timeout(
                config.registry_shutdown_timeout,
                DEFAULT_REGISTRY_SHUTDOWN_TIMEOUT,
            ),
            gate: Arc::new(RuntimeGate {
                generation: config.generation,
                state: Mutex::new(GateState {
                    closing: false,
                    operations: 0,
                    finished: false,
                    failure: None,
                }),
                wake: Condvar::new(),
            }),
        })
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    fn begin(&self, generation: u64) -> AppResult<OperationGuard> {
        let mut state = lock(&self.gate.state);
        if generation != 0 && generation != self.gate.generation {
            return Err(stale_generation(generation, self.gate.generation));
        }
        if state.closing {
            return Err(AppError::Message("workspace is closing".to_owned()));
        }
        state.operations = state.operations.saturating_add(1);
        Ok(OperationGuard {
            gate: Arc::clone(&self.gate),
        })
    }

    fn notify_host(&self) {
        let _ = self.invalidation_sender.try_send(());
    }

    fn material_changed(&self) {
        increment_saturating(self.resource_revision.as_ref());
        let suppressed = HOST_INVALIDATION_SUPPRESSIONS.with(|count| count.get() != 0);
        if !suppressed {
            self.notify_host();
        }
    }

    fn association_host(&self) -> ProjectCoordinationStore {
        self.catalog.clone()
    }
}

impl AgentRuntimeService for AgentRuntime {
    fn resource_state(&self, generation: u64) -> AppResult<AgentResourceStateV2> {
        let _operation = self.begin(generation)?;
        Ok(AgentResourceStateV2 {
            generation: self.generation,
            resource_revision: self.resource_revision.load(Ordering::Acquire),
            active_runs: self.registry.active_count(),
        })
    }

    fn resource_revision(&self, generation: u64) -> AppResult<u64> {
        let _operation = self.begin(generation)?;
        Ok(self.resource_revision.load(Ordering::Acquire))
    }

    fn drain_invalidations(&self, generation: u64) -> AppResult<AgentInvalidationV2> {
        let _operation = self.begin(generation)?;
        let receiver = lock(&self.invalidation_receiver);
        let mut event_count = 0_usize;
        while let Ok(()) = receiver.try_recv() {
            event_count = event_count.saturating_add(1);
        }
        Ok(AgentInvalidationV2 {
            generation: self.generation,
            resource_revision: self.resource_revision.load(Ordering::Acquire),
            event_count,
        })
    }

    fn agent_runs(&self, generation: u64) -> AppResult<AgentRunsV2> {
        let _operation = self.begin(generation)?;
        map_coordination(self.coordinator.agent_runs(generation))
    }

    fn agent_runtime_candidates(
        &self,
        generation: u64,
    ) -> AppResult<ptrack_agent::AgentRuntimeCandidatesV2> {
        let _operation = self.begin(generation)?;
        map_coordination(self.coordinator.agent_runtime_candidates(generation))
    }

    fn workspace_snapshot(
        &self,
        generation: u64,
        git: &CoordinationGitSnapshot,
        deadline: Instant,
    ) -> AppResult<ptrack_agent::AgentWorkspaceSnapshotV2> {
        let _operation = self.begin(generation)?;
        map_coordination(
            self.coordinator
                .workspace_snapshot(generation, git, deadline),
        )
    }

    fn agent_intelligence(&self, generation: u64, run_id: &str) -> AppResult<AgentIntelligenceV2> {
        let _operation = self.begin(generation)?;
        map_coordination(self.coordinator.agent_intelligence(generation, run_id))
    }

    fn activity(&self, generation: u64) -> AppResult<AgentActivitySnapshot> {
        let _operation = self.begin(generation)?;
        map_coordination(self.coordinator.activity(generation))
    }

    fn notifications(&self, generation: u64) -> AppResult<AgentNotificationsV2> {
        let activity = self.activity(generation)?;
        Ok(AgentNotificationsV2 {
            generation: self.generation,
            items: activity.notifications,
            bounds: activity.notification_bounds,
            incomplete: activity.notifications_incomplete,
        })
    }

    fn drift(&self, generation: u64) -> AppResult<ptrack_agent::DriftSnapshot> {
        let _operation = self.begin(generation)?;
        map_coordination(self.coordinator.drift(generation))
    }

    fn preview_handoff(&self, generation: u64, run_id: &str) -> AppResult<AgentHandoffV2> {
        let _operation = self.begin(generation)?;
        map_coordination(self.coordinator.preview_handoff(generation, run_id))
    }

    fn set_task_ownership(
        &self,
        generation: u64,
        run_id: &str,
        expected_association_revision: u64,
        owned: bool,
    ) -> AppResult<AgentOwnershipMutationV2> {
        let _operation = self.begin(generation)?;
        map_coordination(self.coordinator.set_task_ownership(
            generation,
            run_id,
            expected_association_revision,
            owned,
        ))
    }

    fn set_worktree(
        &self,
        generation: u64,
        run_id: &str,
        expected_association_revision: u64,
        root: &str,
        associated: bool,
    ) -> AppResult<AgentWorktreeMutationV2> {
        let _operation = self.begin(generation)?;
        map_coordination(self.coordinator.set_worktree(
            generation,
            run_id,
            expected_association_revision,
            root,
            associated,
        ))
    }

    fn send_handoff(
        &self,
        generation: u64,
        source_run_id: &str,
        target_run_id: &str,
        expected_source_revision: u64,
        expected_target_revision: u64,
    ) -> AppResult<AgentHandoffEnvelopeV2> {
        let _operation = self.begin(generation)?;
        map_coordination(self.coordinator.send_handoff(
            generation,
            source_run_id,
            target_run_id,
            expected_source_revision,
            expected_target_revision,
        ))
    }

    fn acknowledge_handoff(
        &self,
        generation: u64,
        id: &str,
        target_run_id: &str,
    ) -> AppResult<AgentHandoffAcknowledgementV2> {
        let _operation = self.begin(generation)?;
        map_coordination(
            self.coordinator
                .acknowledge_handoff(generation, id, target_run_id),
        )
    }

    fn prepare_workflow(
        &self,
        generation: u64,
        run_id: &str,
        expected_association_revision: u64,
        kind: AgentWorkflowKind,
        target_branch: &str,
    ) -> AppResult<AgentWorkflowProposalV2> {
        let _operation = self.begin(generation)?;
        map_coordination(self.coordinator.prepare_workflow(
            generation,
            run_id,
            expected_association_revision,
            kind,
            target_branch,
        ))
    }

    fn approve_workflow(&self, generation: u64, id: &str) -> AppResult<AgentWorkflowProposalV2> {
        let _operation = self.begin(generation)?;
        map_coordination(self.coordinator.approve_workflow(generation, id))
    }

    fn dismiss_workflow(&self, generation: u64, id: &str) -> AppResult<AgentWorkflowDismissalV2> {
        let _operation = self.begin(generation)?;
        map_coordination(self.coordinator.dismiss_workflow(generation, id))
    }

    fn workflow_targets(&self, generation: u64) -> AppResult<AgentWorkflowTargetsV2> {
        let activity = self.activity(generation)?;
        Ok(AgentWorkflowTargetsV2 {
            generation: self.generation,
            items: activity.workflow_targets,
            incomplete: activity.workflow_targets_incomplete,
        })
    }

    fn with_exact_runtime_snapshot(
        &self,
        generation: u64,
        maximum: usize,
        use_snapshot: &mut dyn FnMut(&[Run]),
    ) -> AppResult<()> {
        let _operation = self.begin(generation)?;
        self.registry
            .with_exact_runtime_snapshot(maximum, |runs| {
                use_snapshot(runs);
                Ok(())
            })
            .map_err(agent_error)
    }

    fn shutdown(&self) -> AppResult<()> {
        let mut state = lock(&self.gate.state);
        if state.closing {
            while !state.finished {
                state = wait(&self.gate.wake, state);
            }
            return state
                .failure
                .as_ref()
                .map_or(Ok(()), |error| Err(AppError::Message(error.clone())));
        }
        state.closing = true;
        drop(state);

        let mut failures = Vec::new();
        self.coordinator.shutdown();
        if let Some(cancellation) = &self.git_cancellation {
            cancellation.cancel();
        }
        let state = lock(&self.gate.state);
        let (state, timeout) = self
            .gate
            .wake
            .wait_timeout_while(state, self.operation_shutdown_timeout, |current| {
                current.operations != 0
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if timeout.timed_out() && state.operations != 0 {
            failures.push("wait for AgentRun operations: timeout".to_owned());
        }
        drop(state);
        if let Err(error) = self.integration.shutdown(self.integration_shutdown_timeout) {
            failures.push(format!("AgentRun integration shutdown: {error}"));
        }
        if let Err(error) = self
            .registry
            .shutdown_timeout(self.registry_shutdown_timeout)
        {
            failures.push(format!("AgentRun registry shutdown: {error}"));
        }
        let failure = (!failures.is_empty()).then(|| failures.join("\n"));
        let mut state = lock(&self.gate.state);
        state.failure.clone_from(&failure);
        state.finished = true;
        self.gate.wake.notify_all();
        failure.map_or(Ok(()), |error| Err(AppError::Message(error)))
    }
}

impl LaunchedEventAuthority for AgentRuntime {
    fn event_endpoint(&self, generation: u64) -> AppResult<String> {
        let _operation = self.begin(generation)?;
        Ok(self.integration.event_endpoint().to_owned())
    }

    fn issue_launched_event_token(&self, generation: u64) -> AppResult<String> {
        let _operation = self.begin(generation)?;
        let token = self
            .registry
            .issue_launched_event_token()
            .map_err(agent_error)?;
        self.material_changed();
        Ok(token)
    }

    fn bind_launched_event_token(
        &self,
        generation: u64,
        token: &str,
        run_id: &str,
    ) -> AppResult<()> {
        let _operation = self.begin(generation)?;
        self.registry
            .bind_launched_event_token(token, run_id)
            .map_err(agent_error)?;
        self.material_changed();
        Ok(())
    }

    fn revoke_launched_event_token(&self, generation: u64, token: &str) -> AppResult<bool> {
        let _operation = self.begin(generation)?;
        let changed = self.registry.revoke_launched_event_token(token);
        if changed {
            self.material_changed();
        }
        Ok(changed)
    }

    fn register_launched(&self, generation: u64, registration: Registration) -> AppResult<Run> {
        let _operation = self.begin(generation)?;
        let run = self
            .registry
            .register_launched(registration)
            .map_err(agent_error)?;
        self.material_changed();
        Ok(run)
    }

    fn register_linked_launched(
        &self,
        generation: u64,
        registration: Registration,
        pointer: AssociationPointer,
    ) -> AppResult<Run> {
        let _operation = self.begin(generation)?;
        let catalog = self.association_host();
        let host = AssociationHost::new(&catalog.endpoint.root, self.generation, Some(&catalog))
            .map_err(agent_error)?;
        let run = self
            .registry
            .register_linked_launched(registration, Some(&host), pointer)
            .map_err(agent_error)?;
        self.material_changed();
        Ok(run)
    }

    fn associate_run(
        &self,
        generation: u64,
        run_id: &str,
        pointer: AssociationPointer,
    ) -> AppResult<Association> {
        let _operation = self.begin(generation)?;
        let catalog = self.association_host();
        let host = AssociationHost::new(&catalog.endpoint.root, self.generation, Some(&catalog))
            .map_err(agent_error)?;
        let association = self
            .registry
            .associate(run_id, Some(&host), pointer)
            .map_err(agent_error)?;
        self.material_changed();
        Ok(association)
    }

    fn rollback_launched(
        &self,
        generation: u64,
        run_id: &str,
        terminal_id: &str,
    ) -> AppResult<bool> {
        let _operation = self.begin(generation)?;
        let changed = self.registry.rollback_launched(run_id, terminal_id);
        if changed {
            self.material_changed();
        }
        Ok(changed)
    }
}

impl LinkedAgentRuntimeHooks for AgentRuntime {
    fn suppress_runtime_event(&self, generation: u64) -> AppResult<AgentRuntimeEventSuppression> {
        let _operation = self.begin(generation)?;
        Ok(AgentRuntimeEventSuppression::new())
    }

    fn fence_admission(&self, generation: u64) -> AppResult<AgentAdmissionFence> {
        let _operation = self.begin(generation)?;
        Ok(AgentAdmissionFence(Some(self.registry.fence_admission())))
    }

    fn rollback_linked_launched(
        &self,
        generation: u64,
        run_id: &str,
        terminal_id: &str,
    ) -> AppResult<bool> {
        let _operation = self.begin(generation)?;
        let changed = self.registry.rollback_linked_launched(run_id, terminal_id);
        if changed {
            self.material_changed();
        }
        Ok(changed)
    }

    fn rollback_linked_terminal(&self, generation: u64, terminal_id: &str) -> AppResult<usize> {
        let _operation = self.begin(generation)?;
        let changed = self.registry.rollback_linked_terminal(terminal_id);
        if changed != 0 {
            self.material_changed();
        }
        Ok(changed)
    }

    fn has_linked_terminal(&self, generation: u64, terminal_id: &str) -> AppResult<bool> {
        let _operation = self.begin(generation)?;
        Ok(self.registry.has_linked_terminal(terminal_id))
    }

    fn revoke_terminal_event_tokens(&self, generation: u64, terminal_id: &str) -> AppResult<bool> {
        let _operation = self.begin(generation)?;
        let changed = self
            .registry
            .revoke_launched_event_token_for_terminal(terminal_id);
        if changed {
            self.material_changed();
        }
        Ok(changed)
    }

    fn record_terminal_activity(
        &self,
        generation: u64,
        terminal_id: &str,
        activity_at: Timestamp,
    ) -> AppResult<AgentMutationOutcome> {
        let _operation = self.begin(generation)?;
        let outcome = self
            .registry
            .record_terminal_activity_at_outcome(terminal_id, activity_at);
        if outcome.changed {
            self.material_changed();
        }
        Ok(outcome.into())
    }

    fn record_terminal_exit(
        &self,
        generation: u64,
        terminal_id: &str,
        code: i32,
        result: &str,
    ) -> AppResult<AgentMutationOutcome> {
        let _operation = self.begin(generation)?;
        let outcome = self
            .registry
            .record_terminal_exit_outcome(terminal_id, code, result);
        if outcome.changed {
            self.material_changed();
        }
        Ok(outcome.into())
    }

    fn prepare_linked_association(
        &self,
        generation: u64,
        terminal_id: &str,
        terminal_previous: Option<&Association>,
        terminal_next: &Association,
        pointer: AssociationPointer,
    ) -> AppResult<Option<LinkedAgentAssociationChange>> {
        let _operation = self.begin(generation)?;
        let catalog = self.association_host();
        let host = AssociationHost::new(&catalog.endpoint.root, self.generation, Some(&catalog))
            .map_err(agent_error)?;
        self.registry
            .prepare_linked_terminal_association_change(
                terminal_id,
                terminal_previous,
                terminal_next,
                Some(&host),
                pointer,
            )
            .map(|change| change.map(LinkedAgentAssociationChange))
            .map_err(agent_error)
    }

    fn commit_linked_association(
        &self,
        generation: u64,
        change: &LinkedAgentAssociationChange,
    ) -> AppResult<()> {
        let _operation = self.begin(generation)?;
        self.registry
            .commit_linked_association_change(&change.0)
            .map_err(agent_error)?;
        self.material_changed();
        Ok(())
    }

    fn rollback_linked_association(
        &self,
        generation: u64,
        change: &LinkedAgentAssociationChange,
    ) -> AppResult<()> {
        let _operation = self.begin(generation)?;
        self.registry
            .rollback_linked_association_change(&change.0)
            .map_err(agent_error)?;
        self.material_changed();
        Ok(())
    }
}

impl Drop for AgentRuntime {
    fn drop(&mut self) {
        let _ = AgentRuntimeService::shutdown(self);
    }
}

/// Reopening project-store adapter. Each method verifies the exact activation
/// binding and drops the store before it returns.
#[derive(Clone)]
pub struct ProjectCoordinationStore {
    endpoint: ProjectEndpoint,
    writer_version: String,
    workspace_generation: u64,
}

impl ProjectCoordinationStore {
    #[must_use]
    pub const fn new(
        endpoint: ProjectEndpoint,
        writer_version: String,
        workspace_generation: u64,
    ) -> Self {
        Self {
            endpoint,
            writer_version,
            workspace_generation,
        }
    }

    fn with_store<R>(&self, use_store: impl FnOnce(&ProjectStore) -> AppResult<R>) -> AppResult<R> {
        let store = ProjectStore::open_existing(
            &self.endpoint.database,
            &self.endpoint.binding,
            &self.writer_version,
        )?;
        let result = use_store(&store);
        drop(store);
        result
    }
}

impl AssociationCatalog for ProjectCoordinationStore {
    fn validate_plan(&self, plan_id: u64) -> Result<(), String> {
        self.with_store(|store| store.plan(plan_id).map(|_| ()).map_err(AppError::from))
            .map_err(|_| "project plan is unavailable".to_owned())
    }

    fn task_plan(&self, task_id: u64) -> Result<u64, String> {
        self.with_store(|store| {
            store
                .task(task_id)
                .map(|task| task.plan_id)
                .map_err(AppError::from)
        })
        .map_err(|_| "project task is unavailable".to_owned())
    }
}

impl CoordinationStore for ProjectCoordinationStore {
    fn current_association(
        &self,
        live_id: &str,
        association: &Association,
    ) -> Option<RuntimeAssociation> {
        if association.generation != self.workspace_generation
            || association.live_id != live_id
            || association.revision == 0
        {
            return None;
        }
        let canonical = std::fs::canonicalize(&self.endpoint.root).ok()?;
        if Path::new(&association.project_root) != canonical {
            return None;
        }
        let host = AssociationHost::new(canonical, self.workspace_generation, Some(self)).ok()?;
        let target = host
            .validate(AssociationPointer {
                version: association.version,
                plan_id: association.target.plan_id,
                task_id: association.target.task_id,
            })
            .ok()?;
        (target == association.target).then_some(RuntimeAssociation {
            plan_id: target.plan_id,
            task_id: target.task_id,
            revision: association.revision,
        })
    }

    fn linked_commit_shas(&self) -> Vec<String> {
        self.with_store(|store| Ok(store.commits()?.into_iter().map(|item| item.sha).collect()))
            .unwrap_or_default()
    }

    fn linked_commit_shas_until(
        &self,
        deadline: Instant,
    ) -> Result<Vec<String>, CoordinationError> {
        self.with_store(|store| Ok(store.commit_shas_until(deadline)?))
            .map_err(|error| CoordinationError::Message(error.to_string()))
    }

    fn tracking_started_at(&self) -> Timestamp {
        self.with_store(|store| Ok(core_timestamp(store.meta()?.created_at)))
            .unwrap_or(Timestamp::ZERO)
    }

    fn plan_title(&self, plan_id: u64) -> Result<Option<String>, CoordinationError> {
        self.with_store(|store| Ok(Some(store.plan(plan_id)?.title)))
            .map_err(|_| store_coordination_error())
    }

    fn task_context(
        &self,
        task_id: u64,
    ) -> Result<Option<CoordinationTaskContext>, CoordinationError> {
        self.with_store(|store| {
            let task = store.task(task_id)?;
            Ok(Some(CoordinationTaskContext {
                id: task.id,
                plan_id: task.plan_id,
                title: task.title,
            }))
        })
        .map_err(|_| store_coordination_error())
    }

    fn recent_decisions(
        &self,
        limit: usize,
    ) -> Result<Vec<CoordinationDecision>, CoordinationError> {
        self.with_store(|store| {
            let notes = store.recent_notes(limit.min(STORE_SUGGESTION_LIMIT))?;
            Ok(notes
                .into_iter()
                .filter(|note| note.kind == MemoryKind::Decision)
                .map(|note| CoordinationDecision {
                    id: note.id,
                    target: match note.target {
                        NoteTarget::Plan => CoordinationTarget::Plan,
                        NoteTarget::Task => CoordinationTarget::Task,
                        NoteTarget::Project => CoordinationTarget::Project,
                    },
                    target_id: note.target_id,
                    body: note.body,
                })
                .collect())
        })
        .map_err(|_| store_coordination_error())
    }

    fn open_issues(&self) -> Result<Vec<CoordinationIssue>, CoordinationError> {
        self.with_store(|store| {
            Ok(store
                .issues()?
                .into_iter()
                .filter(|issue| issue.status == IssueStatus::Open)
                .map(|issue| CoordinationIssue {
                    id: issue.id,
                    title: issue.title,
                    task_id: issue.task_id,
                })
                .collect())
        })
        .map_err(|_| store_coordination_error())
    }
}

/// Concrete bounded ptrack-git adapter. Its cancellation token is owned by the
/// runtime and cancelled during teardown after the coordinator is closed.
pub struct PtrackCoordinationGit {
    service: RepositoryService,
    cancellation: CancellationToken,
}

impl Default for PtrackCoordinationGit {
    fn default() -> Self {
        Self::new()
    }
}

impl PtrackCoordinationGit {
    #[must_use]
    pub fn new() -> Self {
        Self {
            service: RepositoryService::new(),
            cancellation: CancellationToken::new(),
        }
    }
}

impl CoordinationGit for PtrackCoordinationGit {
    fn inspect_worktree(
        &self,
        project_root: &Path,
        root: &Path,
    ) -> Result<WorktreeIdentity, CoordinationError> {
        let value = self
            .service
            .inspect_worktree(&self.cancellation, project_root, root)
            .map_err(|_| git_coordination_error())?;
        Ok(WorktreeIdentity {
            root: value.root,
            git_dir: value.git_dir,
            common_git_dir: value.common_git_dir,
            branch: value.branch,
            head: value.head,
            linked: value.linked,
        })
    }

    fn snapshot(&self, root: &Path) -> Result<CoordinationGitSnapshot, CoordinationError> {
        let value = self
            .service
            .capture(&self.cancellation, root)
            .map_err(|_| git_coordination_error())?;
        map_git_snapshot(value)
    }
}

pub(crate) fn map_git_snapshot(
    value: Snapshot,
) -> Result<CoordinationGitSnapshot, CoordinationError> {
    if value.state == RepositoryState::NotRepository {
        return Ok(CoordinationGitSnapshot::default());
    }
    let ahead = bounded_usize(value.status.ahead)?;
    let behind = bounded_usize(value.status.behind)?;
    let (recent_commits, invalid_recent_timestamp) =
        map_git_commits(value.recent_commits.unwrap_or_default());
    let (unpushed_commits, invalid_unpushed_timestamp) =
        map_git_commits(value.unpushed_commits.unwrap_or_default());
    Ok(CoordinationGitSnapshot {
        root: value.root,
        git_dir: value.git_dir,
        common_git_dir: value.common_git_dir,
        branch: value.status.branch.clone(),
        head: value.status.oid,
        bare: value.bare,
        upstream: value.status.upstream,
        detached: value.status.detached,
        initial: value.status.initial,
        status: ptrack_agent::AgentWorkflowStatus {
            staged: value.status.staged,
            unstaged: value.status.unstaged,
            untracked: value.status.untracked,
            conflicted: value.status.conflicted,
            ahead,
            behind,
        },
        changed_paths: value.status.changed_paths.unwrap_or_default(),
        untracked_paths: value.status.untracked_paths.unwrap_or_default(),
        changed_more: value.status.changed_path_bounds.more,
        untracked_more: value.status.untracked_path_bounds.more,
        branches: value
            .local_branches
            .unwrap_or_default()
            .into_iter()
            .map(|branch| GitBranch {
                name: branch.name,
                head: branch.oid,
            })
            .collect(),
        worktrees: value
            .worktrees
            .unwrap_or_default()
            .into_iter()
            .map(|worktree| ExistingWorktree {
                root: worktree.root,
                branch: worktree.branch,
                head: worktree.head,
            })
            .collect(),
        worktree_bounds: BoundedSnapshot {
            shown: value.worktree_bounds.shown,
            total: value.worktree_bounds.total,
            more: value.worktree_bounds.more,
        },
        worktrees_incomplete: value.worktrees_incomplete,
        recent_commits,
        unpushed_commits,
        recent_commits_incomplete: value.recent_commits_truncated || invalid_recent_timestamp,
        unpushed_commits_incomplete: value.unpushed_commits_truncated || invalid_unpushed_timestamp,
        divergence: value
            .divergence
            .map(|divergence| {
                Ok(GitDivergence {
                    upstream: divergence.upstream,
                    ahead: bounded_usize(divergence.ahead)?,
                    behind: bounded_usize(divergence.behind)?,
                })
            })
            .transpose()?,
    })
}

fn map_git_commits(values: Vec<ptrack_git::Commit>) -> (Vec<GitCommit>, bool) {
    let mut incomplete = false;
    let commits = values
        .into_iter()
        .filter_map(|commit| {
            if let Ok(committed_at) = Timestamp::parse(&commit.date) {
                Some(GitCommit {
                    sha: commit.sha,
                    committed_at,
                })
            } else {
                incomplete = true;
                None
            }
        })
        .collect();
    (commits, incomplete)
}

fn bounded_usize(value: i64) -> Result<usize, CoordinationError> {
    usize::try_from(value).map_err(|_| git_coordination_error())
}

fn core_timestamp(value: ptrack_core::Timestamp) -> Timestamp {
    value
        .unix_nanoseconds()
        .map_or(Timestamp::ZERO, Timestamp::from_unix_nanoseconds)
}

fn validated_endpoint(endpoint: &ProjectEndpoint) -> AppResult<ProjectEndpoint> {
    let root = std::fs::canonicalize(&endpoint.root)?;
    let expected_database = root.join(".ptrack").join("ptrack.redb");
    if endpoint.root != root
        || endpoint.database != expected_database
        || std::fs::canonicalize(&endpoint.database)? != endpoint.database
        || endpoint.binding.generation == 0
        || endpoint.binding.kind != StoreKind::Project
        || endpoint.binding.canonical_path != endpoint.database
    {
        return Err(AppError::Message(
            "AgentRun project activation binding is invalid".to_owned(),
        ));
    }
    Ok(endpoint.clone())
}

fn validated_global_attestation(
    global_home: &Path,
    global_database: &Path,
    global_binding: &ActiveBinding,
    project_binding_generation: u64,
) -> AppResult<PathBuf> {
    let clean_home = clean_absolute(global_home).ok_or_else(|| {
        AppError::Message("AgentRun global home must be absolute and lexically clean".to_owned())
    })?;
    let expected_database = clean_home.join("global.redb");
    if global_home != clean_home
        || global_database != expected_database
        || std::fs::canonicalize(global_database)? != global_database
        || global_binding.generation == 0
        || global_binding.generation != project_binding_generation
        || global_binding.kind != StoreKind::Global
        || global_binding.canonical_path != global_database
    {
        return Err(AppError::Message(
            "AgentRun global activation binding is invalid".to_owned(),
        ));
    }
    Ok(clean_home)
}

fn clean_absolute(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() || has_windows_dot_segment(path) {
        return None;
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                clean.pop();
            }
            _ => clean.push(component.as_os_str()),
        }
    }
    (clean == path).then_some(clean)
}

#[cfg(windows)]
fn has_windows_dot_segment(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt as _;

    const DOT: u16 = b'.' as u16;
    const FORWARD_SLASH: u16 = b'/' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    let encoded: Vec<_> = path.as_os_str().encode_wide().collect();
    encoded
        .split(|unit| matches!(*unit, FORWARD_SLASH | BACKSLASH))
        .any(|segment| segment == [DOT] || segment == [DOT, DOT])
}

#[cfg(not(windows))]
fn has_windows_dot_segment(_path: &Path) -> bool {
    false
}

fn positive_timeout(value: Duration, fallback: Duration) -> Duration {
    if value.is_zero() { fallback } else { value }
}

fn increment_saturating(value: &AtomicU64) {
    let _ = value.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        Some(current.saturating_add(1))
    });
}

fn stale_generation(expected: u64, active: u64) -> AppError {
    AppError::Message(format!(
        "stale workspace generation: expected {expected}, active {active}"
    ))
}

fn map_coordination<T>(result: Result<T, CoordinationError>) -> AppResult<T> {
    result.map_err(agent_error)
}

fn agent_error(error: impl fmt::Display) -> AppError {
    AppError::Message(error.to_string())
}

fn store_coordination_error() -> CoordinationError {
    CoordinationError::Message("project store lookup failed".to_owned())
}

fn git_coordination_error() -> CoordinationError {
    CoordinationError::Message("Git repository inspection failed".to_owned())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn wait<'a, T>(wake: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    wake.wait(guard)
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
