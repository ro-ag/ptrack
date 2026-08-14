use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ptrack_agent::{Association, AssociationTarget, CoordinationSession, CoordinationSessions};
use ptrack_capability::{BrokerConfig, BrokerServer, BrokerServerConfig};
use ptrack_core::{ProjectRef, ProjectSnapshot};
use ptrack_store::{
    ActiveBinding, ActiveGeneration, ActiveGenerationProject, CutoverLease, CutoverLockMode,
    GlobalStore, ProjectStore, StoreKind, acquire_cutover_lock, install_active_generation,
    load_active_generation, open_private_path, protect_private_directory, protect_private_file,
    sync_private_directory, validate_active_generation,
};
use ptrack_terminal::{
    Manager, ProfileKind, discover_profiles, load_profile_config_if_exists, merge_profiles,
    profile_config_path,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{
    AgentRuntime, AgentRuntimeConfig, AppError, AppResult, ApplicationPort, BoundDesktopWorkspace,
    CapabilityCancellation, CapabilityMcpOutcome, DesktopAgentRuntime, DesktopEventSink,
    DesktopTerminalEventSink, DesktopWorkspace, DesktopWorkspaceFactory, GuideAction, HookAction,
    HookResult, InitRequest, InitResult, LocalApplication, Mutation, MutationResult, ProcessOutput,
    ProductionTerminalIdentityAuthority, ProjectEndpoint, RecentProjectsProvider,
    TerminalAgentAuthority, TerminalEventSink, TerminalIdentityAuthority, TerminalRuntime,
    TerminalRuntimeConfig, WorkspaceBindings, WorkspaceProject,
};

const RECOVERY_REQUIRED: &str = "runtime recovery is required";
const BOOTSTRAP_PLAN: &str = "bootstrap.json";
const BOOTSTRAP_LIMIT: u64 = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct BootstrapPlan {
    format: String,
    version: String,
    previous_marker: Option<ActiveGeneration>,
    target_marker: ActiveGeneration,
    project_root: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeBindingState {
    Uninitialized,
    Active,
}

/// Process-owned active generation. The retained shared lease prevents an
/// offline activation or rollback while any caller can still write stores.
pub struct ActiveRuntime {
    home: PathBuf,
    marker: ActiveGeneration,
    writer_version: String,
    _lease: CutoverLease,
}

impl ActiveRuntime {
    /// Loads and attests the sole active-generation marker.
    ///
    /// # Errors
    /// Returns a recovery-required error for an unsafe marker, lock, or store.
    pub fn load(
        global_home: impl AsRef<Path>,
        writer_version: impl Into<String>,
    ) -> AppResult<Option<Arc<Self>>> {
        let writer_version = writer_version.into();
        let global_home = global_home.as_ref();
        if !global_home.exists() {
            return Ok(None);
        }
        let home = fs::canonicalize(global_home).map_err(recovery)?;
        let lease = acquire_cutover_lock(&home, CutoverLockMode::Shared).map_err(recovery)?;
        let Some(marker) = load_active_generation(&home, &lease).map_err(recovery)? else {
            return Ok(None);
        };
        validate_active_generation(&home, &marker, &writer_version).map_err(recovery)?;
        Ok(Some(Arc::new(Self {
            home,
            marker,
            writer_version,
            _lease: lease,
        })))
    }

    #[must_use]
    pub fn state(&self) -> RuntimeBindingState {
        RuntimeBindingState::Active
    }

    #[must_use]
    pub fn global_home(&self) -> &Path {
        &self.home
    }

    #[must_use]
    pub const fn marker(&self) -> &ActiveGeneration {
        &self.marker
    }

    /// Resolves the deepest marker-mapped ancestor of `current`.
    ///
    /// # Errors
    /// Returns a filesystem, marker, or binding error.
    pub fn bindings_for(&self, current: &Path) -> AppResult<WorkspaceBindings> {
        let current = fs::canonicalize(current)?;
        let project = self
            .marker
            .projects
            .iter()
            .filter(|project| current.starts_with(Path::new(&project.root)))
            .max_by_key(|project| Path::new(&project.root).components().count())
            .map(|project| self.endpoint(project))
            .transpose()?;
        self.bindings(current, project)
    }

    /// Resolves only an exact canonical project root.
    ///
    /// # Errors
    /// Returns no-project or a filesystem/binding error.
    pub fn bindings_for_exact_root(&self, root: &Path) -> AppResult<WorkspaceBindings> {
        let root = fs::canonicalize(root)?;
        let project = self
            .marker
            .projects
            .iter()
            .find(|project| Path::new(&project.root) == root)
            .ok_or(AppError::NoProject)?;
        self.bindings(root, Some(self.endpoint(project)?))
    }

    /// Returns global-only bindings under the retained generation lease.
    ///
    /// # Errors
    /// Returns a filesystem or binding error.
    pub fn global_bindings(&self, current: &Path) -> AppResult<WorkspaceBindings> {
        self.bindings(fs::canonicalize(current)?, None)
    }

    fn endpoint(&self, project: &ActiveGenerationProject) -> AppResult<ProjectEndpoint> {
        Ok(ProjectEndpoint {
            root: PathBuf::from(&project.root),
            database: PathBuf::from(&project.path),
            binding: self.marker.project_binding(project)?,
        })
    }

    fn bindings(
        &self,
        current_dir: PathBuf,
        project: Option<ProjectEndpoint>,
    ) -> AppResult<WorkspaceBindings> {
        Ok(WorkspaceBindings {
            current_dir,
            project,
            global_database: PathBuf::from(&self.marker.global.path),
            global_binding: self.marker.global_binding()?,
            global_home: self.home.clone(),
            writer_version: self.writer_version.clone(),
        })
    }
}

/// Lazily resolves production bindings only when a data command is executed.
/// Help, version, completion, and launch parsing never touch the marker.
pub struct RoutedApplication {
    global_home: PathBuf,
    current_dir: PathBuf,
    writer_version: String,
    active: Option<Arc<ActiveRuntime>>,
}

impl RoutedApplication {
    #[must_use]
    pub fn new(
        global_home: PathBuf,
        current_dir: PathBuf,
        writer_version: impl Into<String>,
    ) -> Self {
        Self {
            global_home,
            current_dir,
            writer_version: writer_version.into(),
            active: None,
        }
    }

    /// Lazily loads the process authority.
    ///
    /// # Errors
    /// Returns recovery-required for malformed or unsafe active state.
    pub fn active_runtime(&mut self) -> AppResult<Option<Arc<ActiveRuntime>>> {
        if self.active.is_none() {
            self.active = ActiveRuntime::load(&self.global_home, &self.writer_version)?;
        }
        Ok(self.active.clone())
    }

    /// Resolves current-directory project bindings.
    ///
    /// # Errors
    /// Returns uninitialized, no-project, or recovery-required.
    pub fn bindings(&mut self) -> AppResult<WorkspaceBindings> {
        self.active_runtime()?
            .ok_or_else(uninitialized)?
            .bindings_for(&self.current_dir)
    }

    fn local(&mut self) -> AppResult<LocalApplication> {
        Ok(LocalApplication::new(self.bindings()?))
    }

    fn local_global(&mut self) -> AppResult<LocalApplication> {
        let active = self.active_runtime()?.ok_or_else(uninitialized)?;
        Ok(LocalApplication::new(
            active.global_bindings(&self.current_dir)?,
        ))
    }

    fn bootstrap(&mut self, request: &InitRequest) -> AppResult<bool> {
        ensure_private_home(&self.global_home)?;
        let home = fs::canonicalize(&self.global_home)?;
        let lease = acquire_cutover_lock(&home, CutoverLockMode::Exclusive).map_err(recovery)?;
        let existing = load_active_generation(&home, &lease).map_err(recovery)?;
        if let Some(marker) = &existing {
            validate_active_generation(&home, marker, &self.writer_version).map_err(recovery)?;
        }
        let requested = request.root.as_deref().unwrap_or(&self.current_dir);
        let root = fs::canonicalize(requested)?;
        let plan_path = home.join("runtime").join(BOOTSTRAP_PLAN);
        let plan = if plan_path.exists() {
            let plan = read_bootstrap_plan(&plan_path)?;
            validate_bootstrap_plan(&home, &root, &plan, &self.writer_version)?;
            plan
        } else if existing.as_ref().is_some_and(|marker| {
            marker
                .projects
                .iter()
                .any(|project| Path::new(&project.root) == root)
        }) {
            drop(lease);
            self.active = ActiveRuntime::load(&home, &self.writer_version)?;
            return Ok(false);
        } else {
            let plan = new_bootstrap_plan(&home, &root, existing.clone())?;
            publish_bootstrap_plan(&plan_path, &plan)?;
            plan
        };
        if existing.as_ref() == Some(&plan.target_marker) {
            validate_active_generation(&home, &plan.target_marker, &self.writer_version)
                .map_err(recovery)?;
            clear_bootstrap_plan(&plan_path)?;
            drop(lease);
            self.active = ActiveRuntime::load(&home, &self.writer_version)?;
            return Ok(false);
        }
        if existing != plan.previous_marker {
            return Err(recovery(
                "bootstrap plan does not match the active-generation marker",
            ));
        }
        ensure_bootstrap_stores(&home, &plan, &self.writer_version)?;
        install_active_generation(&home, &lease, &plan.target_marker, &self.writer_version)
            .map_err(recovery)?;
        clear_bootstrap_plan(&plan_path)?;
        drop(lease);
        self.active = ActiveRuntime::load(&home, &self.writer_version)?;
        Ok(true)
    }
}

impl ApplicationPort for RoutedApplication {
    fn initialize(&mut self, request: InitRequest) -> AppResult<InitResult> {
        let initialized_root = fs::canonicalize(
            request
                .root
                .as_deref()
                .unwrap_or(self.current_dir.as_path()),
        )?;
        let created = self.bootstrap(&request)?;
        self.current_dir = initialized_root;
        let mut result = self.local()?.initialize(request)?;
        if created {
            result.already_initialized = false;
        }
        Ok(result)
    }

    fn snapshot(&mut self) -> AppResult<ProjectSnapshot> {
        self.local()?.snapshot()
    }

    fn mutate(&mut self, mutation: Mutation) -> AppResult<MutationResult> {
        self.local()?.mutate(mutation)
    }

    fn projects(&mut self) -> AppResult<Vec<ProjectRef>> {
        self.local_global()?.projects()
    }

    fn backup(&mut self) -> AppResult<PathBuf> {
        self.local()?.backup()
    }

    fn guide(&mut self, action: GuideAction) -> AppResult<(String, Vec<PathBuf>)> {
        self.local()?.guide(action)
    }

    fn hook(&mut self, action: HookAction) -> AppResult<HookResult> {
        self.local()?.hook(action)
    }

    fn git_show(&mut self, reference: &str, stat: bool) -> AppResult<ProcessOutput> {
        self.local()?.git_show(reference, stat)
    }

    fn capability_call(&mut self, tool: &str, arguments: &str) -> AppResult<Vec<u8>> {
        self.local()?.capability_call(tool, arguments)
    }

    fn capability_mcp(
        &mut self,
        input: Box<dyn Read + Send>,
        output: &mut dyn Write,
        cancellation: &CapabilityCancellation,
    ) -> AppResult<CapabilityMcpOutcome> {
        self.local()?.capability_mcp(input, output, cancellation)
    }
}

pub struct ProductionRecentProjects {
    runtime: Arc<ActiveRuntime>,
}

impl ProductionRecentProjects {
    #[must_use]
    pub fn new(runtime: Arc<ActiveRuntime>) -> Arc<Self> {
        Arc::new(Self { runtime })
    }
}

impl RecentProjectsProvider for ProductionRecentProjects {
    fn recent_projects(&self) -> AppResult<Vec<Value>> {
        let bindings = self.runtime.global_bindings(self.runtime.global_home())?;
        let store =
            GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding)?;
        let mapped = self
            .runtime
            .marker
            .projects
            .iter()
            .map(|project| project.root.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        Ok(store
            .recent_projects(20)?
            .into_iter()
            .filter(|project| mapped.contains(project.path.as_str()))
            .map(|project| {
                let available = self.runtime.marker.projects.iter().any(|mapped_project| {
                    mapped_project.root == project.path
                        && fs::canonicalize(&project.path)
                            .is_ok_and(|path| path == Path::new(&mapped_project.root))
                        && self
                            .runtime
                            .marker
                            .project_binding(mapped_project)
                            .is_ok_and(|binding| {
                                ProjectStore::open_existing(
                                    &mapped_project.path,
                                    &binding,
                                    &self.runtime.writer_version,
                                )
                                .is_ok()
                            })
                });
                json!({
                    "name": project.name,
                    "path": project.path,
                    "lastSeen": format_timestamp(project.last_seen),
                    "available": available
                })
            })
            .collect())
    }
}

pub struct ProductionDesktopWorkspaceFactory {
    runtime: Arc<ActiveRuntime>,
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
            runtime,
            events,
            async_runtime,
            initial_plan,
        }))
    }
}

impl DesktopWorkspaceFactory for ProductionDesktopWorkspaceFactory {
    fn build(&self, root: &Path, generation: u64) -> AppResult<Arc<dyn DesktopWorkspace>> {
        let bindings = self.runtime.bindings_for_exact_root(root)?;
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
        let server = Arc::new(BrokerServer::start(BrokerServerConfig {
            global_home: bindings.global_home.clone(),
            broker: BrokerConfig {
                project_root: endpoint.root.clone(),
                database: endpoint.database.clone(),
                binding: endpoint.binding.clone(),
                writer_version: bindings.writer_version.clone(),
                generation,
            },
        })?);
        let terminal_agent: Arc<dyn TerminalAgentAuthority> = agent.clone();
        let identity: Arc<dyn TerminalIdentityAuthority> =
            Arc::new(ProductionTerminalIdentityAuthority::new(
                Some(Arc::clone(server.broker())),
                Some(terminal_agent),
            ));
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
            Some(Arc::clone(server.broker())),
        );
        Ok(Arc::new(ProductionDesktopWorkspace {
            inner,
            server,
            _runtime: Arc::clone(&self.runtime),
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
    server: Arc<BrokerServer>,
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

    fn shutdown(&self) -> AppResult<()> {
        let inner = self.inner.shutdown();
        let server = self.server.shutdown().map_err(AppError::from);
        match (inner, server) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(first), Err(second)) => Err(AppError::Message(format!("{first}\n{second}"))),
        }
    }
}

/// Resolves the fixed global home without touching it.
///
/// # Errors
/// Returns an error when no platform home or current directory is available.
pub fn resolve_global_home() -> AppResult<PathBuf> {
    let configured = std::env::var_os("PTRACK_HOME").filter(|value| !value.is_empty());
    let home = configured.or_else(|| {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|value| PathBuf::from(value).join(".ptrack").into_os_string())
    });
    let home = home
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Message("p-track home is unavailable".to_owned()))?;
    if home.is_absolute() {
        Ok(home)
    } else {
        Ok(std::env::current_dir()?.join(home))
    }
}

fn recovery(error: impl std::fmt::Display) -> AppError {
    AppError::Message(format!("{RECOVERY_REQUIRED}: {error}"))
}

fn uninitialized() -> AppError {
    AppError::Message("p-track runtime is not initialized (run 'ptrack init')".to_owned())
}

fn ensure_private_home(path: &Path) -> AppResult<()> {
    if !path.exists() {
        fs::create_dir_all(path)?;
        protect_private_directory(path)?;
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(recovery("global home is not a real directory"));
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> AppResult<()> {
    if !path.exists() {
        fs::create_dir(path)?;
        protect_private_directory(path)?;
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(recovery("project storage directory is unsafe"));
    }
    Ok(())
}

fn new_bootstrap_plan(
    home: &Path,
    root: &Path,
    previous_marker: Option<ActiveGeneration>,
) -> AppResult<BootstrapPlan> {
    let generation = previous_marker
        .as_ref()
        .map(ActiveGeneration::generation_number)
        .transpose()?
        .unwrap_or(random_nonzero_u64()?);
    let global_path = home.join("global.redb");
    if previous_marker.is_none() && path_is_present(&home.join("global.db"))? {
        return Err(recovery(
            "legacy global.db requires the offline migration workflow",
        ));
    }
    let global_database_id = previous_marker
        .as_ref()
        .map_or_else(random_id, |marker| Ok(marker.global.database_id.clone()))?;
    if previous_marker.is_none() && global_path.exists() {
        return Err(recovery(
            "an unpublished Rust global database requires recovery",
        ));
    }
    let project_directory = root.join(".ptrack");
    if path_is_present(&project_directory.join("ptrack.db"))? {
        return Err(recovery(
            "legacy .ptrack/ptrack.db requires the offline migration workflow",
        ));
    }
    ensure_private_directory(&project_directory)?;
    let project_path = project_directory.join("ptrack.redb");
    if project_path.exists() {
        return Err(recovery(
            "an unmapped Rust project database requires recovery",
        ));
    }
    let mut projects = previous_marker
        .as_ref()
        .map(|marker| marker.projects.clone())
        .unwrap_or_default();
    projects.push(ActiveGenerationProject {
        root: root.to_string_lossy().into_owned(),
        database_id: random_id()?,
        path: project_path.to_string_lossy().into_owned(),
    });
    projects.sort_by(|left, right| left.root.cmp(&right.root));
    let target_marker =
        ActiveGeneration::new(generation, global_database_id, &global_path, projects)?;
    Ok(BootstrapPlan {
        format: "ptrack-bootstrap-plan".to_owned(),
        version: "1".to_owned(),
        previous_marker,
        target_marker,
        project_root: root.to_string_lossy().into_owned(),
    })
}

fn path_is_present(path: &Path) -> AppResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn validate_bootstrap_plan(
    home: &Path,
    root: &Path,
    plan: &BootstrapPlan,
    writer_version: &str,
) -> AppResult<()> {
    if plan.format != "ptrack-bootstrap-plan"
        || plan.version != "1"
        || Path::new(&plan.project_root) != root
        || Path::new(&plan.target_marker.global.path) != home.join("global.redb")
    {
        return Err(recovery("bootstrap plan is inconsistent"));
    }
    let generation = plan.target_marker.generation_number()?;
    let reconstructed = ActiveGeneration::new(
        generation,
        plan.target_marker.global.database_id.clone(),
        Path::new(&plan.target_marker.global.path),
        plan.target_marker.projects.clone(),
    )?;
    if reconstructed != plan.target_marker {
        return Err(recovery("bootstrap target marker is invalid"));
    }
    let project = plan
        .target_marker
        .projects
        .iter()
        .find(|project| project.root == plan.project_root)
        .ok_or_else(|| recovery("bootstrap project is missing"))?;
    if Path::new(&project.path) != root.join(".ptrack/ptrack.redb") {
        return Err(recovery("bootstrap project path is invalid"));
    }
    let mut expected_projects = plan
        .previous_marker
        .as_ref()
        .map(|marker| marker.projects.clone())
        .unwrap_or_default();
    expected_projects.push(project.clone());
    expected_projects.sort_by(|left, right| left.root.cmp(&right.root));
    if plan.target_marker.projects != expected_projects {
        return Err(recovery(
            "bootstrap target is not one exact project addition",
        ));
    }
    if let Some(previous) = &plan.previous_marker {
        validate_active_generation(home, previous, writer_version).map_err(recovery)?;
        if previous.generation != plan.target_marker.generation
            || previous.global != plan.target_marker.global
        {
            return Err(recovery("bootstrap changed the existing generation"));
        }
    }
    Ok(())
}

fn ensure_bootstrap_stores(
    home: &Path,
    plan: &BootstrapPlan,
    writer_version: &str,
) -> AppResult<()> {
    let generation = plan.target_marker.generation_number()?;
    if plan.previous_marker.is_none() {
        let binding = binding_for_new(
            generation,
            plan.target_marker.global.database_id.clone(),
            StoreKind::Global,
            Path::new(&plan.target_marker.global.path),
        )?;
        if Path::new(&plan.target_marker.global.path).exists() {
            let store = GlobalStore::open_existing(&plan.target_marker.global.path, &binding)
                .map_err(recovery)?;
            if store.application_writes().map_err(recovery)? {
                return Err(recovery("unpublished global store has application writes"));
            }
        } else {
            drop(
                GlobalStore::create_new(&plan.target_marker.global.path, binding)
                    .map_err(recovery)?,
            );
        }
    }
    let project = plan
        .target_marker
        .projects
        .iter()
        .find(|project| project.root == plan.project_root)
        .ok_or_else(|| recovery("bootstrap project is missing"))?;
    ensure_private_directory(
        Path::new(&project.path)
            .parent()
            .ok_or_else(|| recovery("bootstrap project path has no parent"))?,
    )?;
    let binding = binding_for_new(
        generation,
        project.database_id.clone(),
        StoreKind::Project,
        Path::new(&project.path),
    )?;
    if Path::new(&project.path).exists() {
        let store = ProjectStore::open_existing(&project.path, &binding, writer_version)
            .map_err(recovery)?;
        if store.application_writes().map_err(recovery)? {
            return Err(recovery("unpublished project store has application writes"));
        }
    } else {
        drop(ProjectStore::create_new(&project.path, binding, writer_version).map_err(recovery)?);
    }
    if Path::new(&plan.target_marker.global.path) != home.join("global.redb") {
        return Err(recovery("bootstrap global path changed"));
    }
    Ok(())
}

fn read_bootstrap_plan(path: &Path) -> AppResult<BootstrapPlan> {
    let file = open_private_path(path, false, false).map_err(recovery)?;
    let length = file.metadata()?.len();
    if length == 0 || length > BOOTSTRAP_LIMIT {
        return Err(recovery("bootstrap plan size is invalid"));
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(length).map_err(|_| recovery("bootstrap plan is too large"))?,
    );
    file.take(BOOTSTRAP_LIMIT + 1).read_to_end(&mut bytes)?;
    let plan: BootstrapPlan =
        serde_json::from_slice(&bytes).map_err(|_| recovery("bootstrap plan is invalid"))?;
    if canonical_bootstrap_bytes(&plan)? != bytes {
        return Err(recovery("bootstrap plan is not canonical"));
    }
    Ok(plan)
}

fn publish_bootstrap_plan(path: &Path, plan: &BootstrapPlan) -> AppResult<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    protect_private_file(path).map_err(recovery)?;
    file.write_all(&canonical_bootstrap_bytes(plan)?)?;
    file.sync_all()?;
    drop(file);
    sync_private_directory(path.parent().expect("bootstrap plan has parent")).map_err(recovery)
}

fn clear_bootstrap_plan(path: &Path) -> AppResult<()> {
    open_private_path(path, false, true).map_err(recovery)?;
    fs::remove_file(path)?;
    sync_private_directory(path.parent().expect("bootstrap plan has parent")).map_err(recovery)
}

fn canonical_bootstrap_bytes(plan: &BootstrapPlan) -> AppResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec(plan)
        .map_err(|error| AppError::Message(format!("{RECOVERY_REQUIRED}: {error}")))?;
    bytes.push(b'\n');
    if bytes.len() as u64 > BOOTSTRAP_LIMIT {
        return Err(recovery("bootstrap plan exceeds the fixed limit"));
    }
    Ok(bytes)
}

fn binding_for_new(
    generation: u64,
    database_id: String,
    kind: StoreKind,
    path: &Path,
) -> AppResult<ActiveBinding> {
    let parent = path
        .parent()
        .ok_or_else(|| recovery("database path has no parent"))?
        .canonicalize()?;
    Ok(ActiveBinding {
        generation,
        database_id,
        kind,
        canonical_path: parent.join(
            path.file_name()
                .ok_or_else(|| recovery("database path has no name"))?,
        ),
    })
}

fn random_id() -> AppResult<String> {
    let mut raw = [0_u8; 16];
    getrandom::fill(&mut raw)
        .map_err(|_| AppError::Message("runtime identity could not be created".to_owned()))?;
    Ok(URL_SAFE_NO_PAD.encode(raw))
}

fn random_nonzero_u64() -> AppResult<u64> {
    loop {
        let mut raw = [0_u8; 8];
        getrandom::fill(&mut raw)
            .map_err(|_| AppError::Message("runtime generation could not be created".to_owned()))?;
        let value = u64::from_le_bytes(raw);
        if value != 0 {
            return Ok(value);
        }
    }
}

fn format_timestamp(value: ptrack_core::Timestamp) -> String {
    let ptrack_core::Timestamp::Fixed {
        seconds,
        nanoseconds,
        ..
    } = value
    else {
        return "0001-01-01T00:00:00Z".to_owned();
    };
    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .and_then(|value| value.replace_nanosecond(nanoseconds).ok())
        .and_then(|value| value.format(&Rfc3339).ok())
        .unwrap_or_default()
}
