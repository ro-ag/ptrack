use std::collections::BTreeMap;
use std::fs::{self, OpenOptions, TryLockError};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ptrack_agent::{Association, AssociationTarget, CoordinationSession, CoordinationSessions};
use ptrack_capability::{BrokerConfig, BrokerServer, BrokerServerConfig};
#[cfg(unix)]
use ptrack_core::upsert_guide;
use ptrack_core::{ProjectRef, ProjectSnapshot};
use ptrack_store::{
    ActiveBinding, ActiveGeneration, ActiveGenerationProject, ActorIdentity, CutoverLease,
    CutoverLockMode, GlobalStore, PinnedProjectDirectory, PrivatePathIdentity,
    ProjectRegistryCasResult, ProjectStore, StoreError, StoreKind, acquire_cutover_lock,
    install_active_generation, load_active_generation, open_private_path,
    protect_private_directory, protect_private_file, replace_private_file, sha256_digest,
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

#[cfg(unix)]
use crate::ProjectGuideFilePreviewV1;
use crate::{
    AgentRuntime, AgentRuntimeConfig, AppError, AppResult, ApplicationPort, BoundDesktopWorkspace,
    CapabilityCancellation, CapabilityMcpOutcome, DesktopAgentRuntime, DesktopEventSink,
    DesktopInitializationService, DesktopRuntime, DesktopRuntimeConfig, DesktopTerminalEventSink,
    DesktopUpdateEventSink, DesktopUpdateService, DesktopWorkspace, DesktopWorkspaceFactory,
    ForgetRecentProjectResultV1, GuideAction, HookAction, HookResult, InitRequest, InitResult,
    InitializationCheckpointV1, InitializationOutcomeV1, InitializationStatusV1,
    InitializeProjectRequestV1, LocalApplication, Mutation, MutationResult,
    PendingInitializationV1, PlanLifecycleOutcome, PlanLifecycleRequest, ProcessOutput,
    ProductionTerminalIdentityAuthority, ProjectEndpoint, ProjectGuideChoiceV1,
    ProjectGuideFileActionV1, ProjectGuidePreviewRequestV1, ProjectGuidePreviewV1,
    ProjectTargetKindV1, ProjectTargetValidationV1, RecentProjectAvailabilityV1,
    RecentProjectOpenAuthorizationV1, RecentProjectRegistryCommitV1, RecentProjectRegistryStatusV1,
    RecentProjectResolutionV1, RecentProjectV1, RecentProjectsProvider, RecentProjectsV1,
    RelocateRequest, RelocateResult, ResolvedRecentProjectV1, TerminalAgentAuthority,
    TerminalEventSink, TerminalIdentityAuthority, TerminalRuntime, TerminalRuntimeConfig,
    UnavailableUpdateService, UpdateEventSink, UpdateRuntime, UpdateState, WorkspaceBindings,
    WorkspaceProject,
};

const RECOVERY_REQUIRED: &str = "runtime recovery is required";
const BOOTSTRAP_PLAN: &str = "bootstrap.json";
const BOOTSTRAP_LIMIT: u64 = 1024 * 1024;
const DESKTOP_INITIALIZATION: &str = "desktop-initialization.json";
const DESKTOP_INITIALIZATION_LOCK: &str = "desktop-initialization.lock";
const DESKTOP_INITIALIZATION_LIMIT: u64 = 64 * 1024;
const DESKTOP_INITIALIZATION_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const GUIDE_FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
const GUIDE_FILE_LIMIT: u64 = 32 * 1024;
const GUIDE_OUTPUT_LIMIT: usize = 64 * 1024;
const GUIDE_DIFF_LIMIT: usize = 64 * 1024;
const GUIDE_DIFF_LINE_LIMIT: usize = 4_096;
const GUIDE_PREVIEW_LIMIT: usize = 8;
const GUIDE_PREVIEW_STALE: &str = "project-guide-preview-stale";
const GUIDE_PARTIALLY_APPLIED: &str = "project-guide-partially-applied";
const RECENT_CONFIRMATION_LIMIT: usize = 64;
const RECENT_CONFIRMATION_TTL: Duration = Duration::from_secs(120);
const RECENT_LISTING_TTL: Duration = Duration::from_secs(600);
const RECENT_ID_BYTES: usize = 43;
const RECENT_PATH_LIMIT: usize = 16 * 1024;
#[cfg(not(unix))]
const GUIDE_UNAVAILABLE: &str = "Project guidance is not available on this platform yet";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct BootstrapPlan {
    format: String,
    version: String,
    operation_id: Option<String>,
    previous_marker: Option<ActiveGeneration>,
    target_marker: ActiveGeneration,
    project_root: String,
    project_root_identity: PrivatePathIdentity,
    project_directory_identity: PrivatePathIdentity,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DesktopInitializationJournal {
    format: String,
    version: String,
    status: InitializationStatusV1,
    goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guide: Option<DesktopGuideManifest>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DesktopGuideManifest {
    version: String,
    choice: ProjectGuideChoiceV1,
    operation_id: String,
    canonical_root: String,
    preview_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    root_identity: Option<PrivatePathIdentity>,
    template_digest: String,
    files: Vec<DesktopGuideFileManifest>,
}

impl DesktopGuideManifest {
    fn skip(
        operation_id: String,
        canonical_root: String,
        root_identity: PrivatePathIdentity,
    ) -> Self {
        Self {
            version: "1".to_owned(),
            choice: ProjectGuideChoiceV1::Skip,
            operation_id,
            canonical_root,
            preview_token: String::new(),
            root_identity: Some(root_identity),
            template_digest: String::new(),
            files: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DesktopGuideFileManifest {
    name: String,
    action: ProjectGuideFileActionV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_identity: Option<PrivatePathIdentity>,
    base_digest: String,
    output_digest: String,
    mode: u32,
}

#[derive(Clone, Debug)]
struct GuideFileSnapshot {
    identity: PrivatePathIdentity,
    digest: String,
    content: String,
    mode: u32,
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
    /// When validation fails because listed project roots were deleted, the
    /// missing projects are pruned from the marker under the exclusive
    /// cutover lock (the replaced marker is backed up beside it) and the
    /// load is retried once.
    ///
    /// # Errors
    /// Returns a recovery-required error for an unsafe marker, lock, or store.
    pub fn load(
        global_home: impl AsRef<Path>,
        writer_version: impl Into<String>,
    ) -> AppResult<Option<Arc<Self>>> {
        let writer_version = writer_version.into();
        let global_home = global_home.as_ref();
        match Self::attempt(global_home, &writer_version) {
            Err(error) => {
                if prune_missing_marker_projects(global_home, &writer_version).unwrap_or(false) {
                    Self::attempt(global_home, &writer_version)
                } else {
                    Err(error)
                }
            }
            loaded => loaded,
        }
    }

    fn attempt(global_home: &Path, writer_version: &str) -> AppResult<Option<Arc<Self>>> {
        if !global_home.exists() {
            return Ok(None);
        }
        let home = fs::canonicalize(global_home).map_err(recovery)?;
        let lease = acquire_cutover_lock(&home, CutoverLockMode::Shared).map_err(recovery)?;
        if path_is_present(&home.join("runtime").join(BOOTSTRAP_PLAN))? {
            return Err(recovery(
                "bootstrap recovery must complete before runtime load",
            ));
        }
        let Some(marker) = load_active_generation(&home, &lease).map_err(recovery)? else {
            return Ok(None);
        };
        validate_active_generation(&home, &marker, writer_version).map_err(recovery)?;
        Ok(Some(Arc::new(Self {
            home,
            marker,
            writer_version: writer_version.to_owned(),
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
        let (plan, pinned_project) = if plan_path.exists() {
            let plan = read_bootstrap_plan(&plan_path)?;
            validate_bootstrap_plan(&home, &root, &plan, &self.writer_version)?;
            let pinned = PinnedProjectDirectory::prepare_expected_identities(
                &root,
                plan.project_root_identity,
                plan.project_directory_identity,
            )
            .map_err(recovery)?;
            (plan, pinned)
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
            let project_root_identity =
                PinnedProjectDirectory::identify_root(&root).map_err(recovery)?;
            validate_new_bootstrap_target(&home, &root, existing.as_ref())?;
            let pinned = PinnedProjectDirectory::prepare_new_expected(&root, project_root_identity)
                .map_err(recovery)?;
            let plan = new_bootstrap_plan(
                &home,
                &root,
                project_root_identity,
                pinned.directory_identity(),
                existing.clone(),
                None,
            )?;
            publish_bootstrap_plan(&plan_path, &plan)?;
            (plan, pinned)
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
        ensure_bootstrap_stores(&home, &plan, &self.writer_version, Some(&pinned_project))?;
        install_active_generation(&home, &lease, &plan.target_marker, &self.writer_version)
            .map_err(recovery)?;
        clear_bootstrap_plan(&plan_path)?;
        drop(lease);
        self.active = ActiveRuntime::load(&home, &self.writer_version)?;
        Ok(true)
    }

    /// Re-registers a project store whose folder was physically moved.
    ///
    /// Fail-closed on every mismatch: the healthy marker is the base, the
    /// store's recorded binding must belong to the current generation with an
    /// unused database ID, and the storage layer refuses a copied store. The
    /// manifest rewrite lands before the marker install, so a crash between
    /// the two resumes here: an already-rebound store at an unregistered root
    /// skips straight to the marker publication.
    fn relocate_project(&mut self, request: &RelocateRequest) -> AppResult<RelocateResult> {
        self.active = None;
        let home = fs::canonicalize(&self.global_home).map_err(recovery)?;
        let lease = match acquire_cutover_lock(&home, CutoverLockMode::Exclusive) {
            Ok(lease) => lease,
            Err(error) if error.to_string().contains("cutover lock is unavailable") => {
                return Err(AppError::Message(
                    "another p-track process is running; quit it and run 'ptrack relocate' again"
                        .to_owned(),
                ));
            }
            Err(error) => return Err(recovery(error)),
        };
        if path_is_present(&home.join("runtime").join(BOOTSTRAP_PLAN))? {
            return Err(recovery(
                "bootstrap recovery must complete before relocation",
            ));
        }
        let marker = load_active_generation(&home, &lease)
            .map_err(recovery)?
            .ok_or_else(uninitialized)?;
        let requested = request.root.as_deref().unwrap_or(&self.current_dir);
        let root = fs::canonicalize(requested)?;
        let root_text = root
            .to_str()
            .ok_or_else(|| recovery("project root is not valid UTF-8"))?
            .to_owned();
        if marker
            .projects
            .iter()
            .any(|project| Path::new(&project.root) == root)
        {
            return Err(AppError::Message(
                "project is already registered at this location".to_owned(),
            ));
        }
        require_relocation_target(&root, &home)?;
        let database = root.join(".ptrack").join("ptrack.redb");
        if !path_is_present(&database)? {
            return Err(AppError::Message(
                "no project store found at this location".to_owned(),
            ));
        }
        // A symlinked `.ptrack` (or database file) would make the rebound
        // manifest record the resolved path while the marker records the
        // literal one, wedging the store between the two. Refuse up front.
        if fs::canonicalize(&database)? != database {
            return Err(recovery("project storage is unsafe"));
        }
        let database_text = database
            .to_str()
            .ok_or_else(|| recovery("project database path is not valid UTF-8"))?
            .to_owned();
        let recorded = ProjectStore::peek_binding(&database)
            .map_err(recovery)?
            .ok_or_else(|| recovery("the project store is not activated"))?;
        if recorded.generation != marker.generation_number().map_err(recovery)? {
            return Err(recovery(
                "the project store belongs to another runtime generation",
            ));
        }
        if recorded.database_id == marker.global.database_id {
            return Err(recovery(
                "the store's database ID is already bound in the active runtime",
            ));
        }
        let (mut projects, dropped_other) = relocation_marker_projects(&marker, &recorded)?;
        if dropped_other {
            // Match the startup self-heal: never publish a marker that drops
            // an unrelated project without a recoverable backup.
            backup_marker(&home)?;
        }
        // Where the store lived before this run's rebind — the authoritative
        // old root for the recents cleanup, surviving marker pruning.
        let previous_root = (recorded.canonical_path != database)
            .then(|| recorded.canonical_path.parent().and_then(Path::parent))
            .flatten()
            .map(Path::to_path_buf);
        if recorded.canonical_path != database {
            ProjectStore::rebind_moved(&database, &recorded).map_err(recovery)?;
        }
        projects.push(ActiveGenerationProject {
            root: root_text,
            database_id: recorded.database_id,
            path: database_text,
        });
        projects.sort_by(|left, right| left.root.cmp(&right.root));
        let target = ActiveGeneration {
            projects,
            ..marker.clone()
        };
        install_active_generation(&home, &lease, &target, &self.writer_version)
            .map_err(recovery)?;
        drop(lease);
        self.active = ActiveRuntime::load(&home, &self.writer_version)?;
        // Best-effort recents cleanup: move the registry row from the old
        // root to the new one. A stale row is cosmetic, never fatal.
        if let Some(previous_root) = previous_root
            && let Some(runtime) = &self.active
            && let Ok(bindings) = runtime.global_bindings(&root)
            && let Ok(global) =
                GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding)
            && let Ok(Some(expected)) = global.project(&previous_root)
        {
            let _ = global.relocate_project_if_matches(&expected, project_name(&root), &root);
        }
        Ok(RelocateResult { root })
    }
}

impl ApplicationPort for RoutedApplication {
    fn relocate(&mut self, request: RelocateRequest) -> AppResult<RelocateResult> {
        self.relocate_project(&request)
    }

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

    fn plan_lifecycle(&mut self, request: PlanLifecycleRequest) -> AppResult<PlanLifecycleOutcome> {
        self.local()?.plan_lifecycle(request)
    }

    fn projects(&mut self) -> AppResult<Vec<ProjectRef>> {
        self.local_global()?.projects()
    }

    fn identity(&mut self) -> AppResult<Option<ActorIdentity>> {
        self.local_global()?.identity()
    }

    fn set_identity(&mut self, name: &str) -> AppResult<ActorIdentity> {
        self.local_global()?.set_identity(name)
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
    confirmations: Mutex<BTreeMap<String, RecentLocationConfirmation>>,
    completed: Mutex<BTreeMap<String, CompletedRecentOpen>>,
    listed: Mutex<BTreeMap<String, ListedRecentEntry>>,
}

#[derive(Clone)]
struct RecentLocationConfirmation {
    source: ProjectRef,
    source_base: String,
    target_root: String,
    target_database: String,
    target_database_id: String,
    generation: String,
    expires_at: Instant,
}

#[derive(Clone)]
struct CompletedRecentOpen {
    authorization: RecentProjectOpenAuthorizationV1,
    commit: RecentProjectRegistryCommitV1,
    expires_at: Instant,
}

#[derive(Clone)]
struct ListedRecentEntry {
    project: ProjectRef,
    base: String,
    expires_at: Instant,
}

impl ProductionRecentProjects {
    #[must_use]
    pub fn new(runtime: Arc<ActiveRuntime>) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            confirmations: Mutex::new(BTreeMap::new()),
            completed: Mutex::new(BTreeMap::new()),
            listed: Mutex::new(BTreeMap::new()),
        })
    }

    fn global_store(&self) -> AppResult<GlobalStore> {
        let bindings = self
            .runtime
            .global_bindings(self.runtime.global_home())
            .map_err(|_| recent_projects_unavailable())?;
        GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding)
            .map_err(|_| recent_projects_unavailable())
    }

    fn registry_entry(&self, entry_id: &str, base: &str) -> AppResult<ProjectRef> {
        validate_recent_id(entry_id)?;
        validate_recent_id(base)?;
        let listed = lock(&self.listed)
            .get(entry_id)
            .filter(|listed| listed.base == base && Instant::now() <= listed.expires_at)
            .cloned()
            .ok_or_else(recent_entry_stale)?;
        let project = self
            .global_store()?
            .project(&listed.project.path)
            .map_err(|_| recent_projects_unavailable())?
            .ok_or_else(recent_entry_stale)?;
        if recent_entry_base(&self.runtime, &project) != base || project != listed.project {
            return Err(recent_entry_stale());
        }
        Ok(project)
    }

    fn resolved_candidate(&self, candidate: &Path) -> AppResult<ProjectEndpoint> {
        let candidate = candidate.to_str().ok_or_else(recent_project_changed)?;
        if candidate.is_empty() || candidate.len() > RECENT_PATH_LIMIT {
            return Err(recent_project_changed());
        }
        let canonical = fs::canonicalize(candidate).map_err(|error| sanitize_recent_io(&error))?;
        if !canonical.is_dir() {
            return Err(recent_project_changed());
        }
        let bindings = self
            .runtime
            .bindings_for(&canonical)
            .map_err(sanitize_recent_app_error)?;
        let endpoint = bindings.project.ok_or_else(recent_project_changed)?;
        ProjectStore::open_existing(
            &endpoint.database,
            &endpoint.binding,
            &self.runtime.writer_version,
        )
        .map_err(sanitize_recent_store_error)?;
        Ok(endpoint)
    }

    fn exact_candidate(&self, candidate: &Path) -> AppResult<(PathBuf, ProjectEndpoint)> {
        let endpoint = self.resolved_candidate(candidate)?;
        let canonical = fs::canonicalize(candidate).map_err(|error| sanitize_recent_io(&error))?;
        if canonical != endpoint.root {
            return Err(recent_project_changed());
        }
        Ok((canonical, endpoint))
    }

    fn authorization_for(
        &self,
        entry_id: &str,
        base: &str,
        canonical_root: &Path,
        relocation_confirmation_token: &str,
    ) -> AppResult<RecentProjectOpenAuthorizationV1> {
        validate_recent_id(entry_id)?;
        validate_recent_id(base)?;
        let (canonical, endpoint) = self.exact_candidate(canonical_root)?;
        if canonical != canonical_root {
            return Err(recent_project_changed());
        }
        let canonical_root = canonical
            .to_str()
            .ok_or_else(recent_project_changed)?
            .to_owned();
        let replay_key = recent_open_key(
            entry_id,
            base,
            &canonical_root,
            relocation_confirmation_token,
        );
        if let Some(completed) = lock(&self.completed).get(&replay_key).cloned()
            && Instant::now() <= completed.expires_at
        {
            let mut authorization = completed.authorization;
            authorization.already_completed = true;
            return Ok(authorization);
        }
        let source = self.registry_entry(entry_id, base)?;
        if canonical_root == source.path {
            if !relocation_confirmation_token.is_empty() {
                return Err(recent_confirmation_invalid());
            }
        } else {
            validate_recent_id(relocation_confirmation_token)?;
            let confirmation = lock(&self.confirmations)
                .get(relocation_confirmation_token)
                .cloned()
                .ok_or_else(recent_confirmation_invalid)?;
            if confirmation.source != source
                || confirmation.source_base != base
                || confirmation.target_root != canonical_root
                || confirmation.target_database
                    != endpoint
                        .database
                        .to_str()
                        .ok_or_else(recent_project_changed)?
                || confirmation.target_database_id != endpoint.binding.database_id
                || confirmation.generation != self.runtime.marker.generation
                || Instant::now() > confirmation.expires_at
            {
                return Err(recent_confirmation_invalid());
            }
        }
        Ok(RecentProjectOpenAuthorizationV1 {
            entry_id: entry_id.to_owned(),
            base: base.to_owned(),
            canonical_root,
            name: project_name(&endpoint.root),
            relocation_confirmation_token: relocation_confirmation_token.to_owned(),
            already_completed: false,
        })
    }
}

impl RecentProjectsProvider for ProductionRecentProjects {
    fn recent_projects(&self) -> AppResult<Vec<Value>> {
        Ok(self
            .recent_projects_v1()?
            .projects
            .into_iter()
            .map(|project| {
                json!({
                    "name": project.name,
                    "path": project.canonical_path,
                    "lastSeen": project.last_opened_at,
                    "available": project.availability == RecentProjectAvailabilityV1::Available
                })
            })
            .collect())
    }

    fn recent_projects_v1(&self) -> AppResult<RecentProjectsV1> {
        let store = self.global_store()?;
        let registered = store
            .recent_projects(20)
            .map_err(|_| recent_projects_unavailable())?;
        let projects = registered
            .iter()
            .map(|project| {
                if project.name.is_empty()
                    || project.name.len() > 4_096
                    || project.path.is_empty()
                    || project.path.len() > RECENT_PATH_LIMIT
                {
                    return Err(recent_projects_unavailable());
                }
                Ok(RecentProjectV1 {
                    entry_id: recent_entry_id(project),
                    base: recent_entry_base(&self.runtime, project),
                    name: project.name.clone(),
                    canonical_path: project.path.clone(),
                    last_opened_at: format_timestamp(project.last_seen),
                    availability: recent_availability(&self.runtime, project),
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        let expires_at = Instant::now() + RECENT_LISTING_TTL;
        let mut listed = lock(&self.listed);
        listed.retain(|_, entry| Instant::now() <= entry.expires_at);
        for project in registered {
            let entry_id = recent_entry_id(&project);
            listed.insert(
                entry_id,
                ListedRecentEntry {
                    base: recent_entry_base(&self.runtime, &project),
                    project,
                    expires_at,
                },
            );
        }
        while listed.len() > RECENT_CONFIRMATION_LIMIT {
            let Some(oldest) = listed
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(entry_id, _)| entry_id.clone())
            else {
                break;
            };
            listed.remove(&oldest);
        }
        Ok(RecentProjectsV1 { projects })
    }

    fn resolve_recent_project(
        &self,
        entry_id: &str,
        base: &str,
        candidate: &Path,
    ) -> AppResult<ResolvedRecentProjectV1> {
        let source = self.registry_entry(entry_id, base)?;
        let endpoint = self.resolved_candidate(candidate)?;
        let canonical_root = endpoint
            .root
            .to_str()
            .ok_or_else(recent_project_changed)?
            .to_owned();
        let (resolution, confirmation_token) = if canonical_root == source.path {
            (RecentProjectResolutionV1::Ready, String::new())
        } else {
            let token = random_operation_id().map_err(|_| {
                AppError::Message("recent-project confirmation is unavailable".to_owned())
            })?;
            let confirmation = RecentLocationConfirmation {
                source,
                source_base: base.to_owned(),
                target_root: canonical_root.clone(),
                target_database: endpoint
                    .database
                    .to_str()
                    .ok_or_else(recent_project_changed)?
                    .to_owned(),
                target_database_id: endpoint.binding.database_id.clone(),
                generation: self.runtime.marker.generation.clone(),
                expires_at: Instant::now() + RECENT_CONFIRMATION_TTL,
            };
            let mut confirmations = lock(&self.confirmations);
            confirmations.retain(|_, value| Instant::now() <= value.expires_at);
            if confirmations.len() >= RECENT_CONFIRMATION_LIMIT
                && let Some(oldest) = confirmations
                    .iter()
                    .min_by_key(|(_, value)| value.expires_at)
                    .map(|(token, _)| token.clone())
            {
                confirmations.remove(&oldest);
            }
            confirmations.insert(token.clone(), confirmation);
            (RecentProjectResolutionV1::ConfirmationRequired, token)
        };
        Ok(ResolvedRecentProjectV1 {
            entry_id: entry_id.to_owned(),
            base: base.to_owned(),
            canonical_root,
            name: project_name(&endpoint.root),
            resolution,
            confirmation_token,
        })
    }

    fn authorize_recent_project_open(
        &self,
        entry_id: &str,
        base: &str,
        canonical_root: &Path,
        relocation_confirmation_token: &str,
    ) -> AppResult<RecentProjectOpenAuthorizationV1> {
        self.authorization_for(
            entry_id,
            base,
            canonical_root,
            relocation_confirmation_token,
        )
    }

    fn finish_recent_project_open(
        &self,
        authorization: &RecentProjectOpenAuthorizationV1,
    ) -> AppResult<RecentProjectRegistryCommitV1> {
        let replay_key = recent_open_key(
            &authorization.entry_id,
            &authorization.base,
            &authorization.canonical_root,
            &authorization.relocation_confirmation_token,
        );
        if authorization.already_completed
            && let Some(completed) = lock(&self.completed).get(&replay_key).cloned()
            && Instant::now() <= completed.expires_at
        {
            return Ok(completed.commit);
        }
        let commit = (|| {
            let confirmed = self.authorization_for(
                &authorization.entry_id,
                &authorization.base,
                Path::new(&authorization.canonical_root),
                &authorization.relocation_confirmation_token,
            )?;
            let expected = self.registry_entry(&authorization.entry_id, &authorization.base)?;
            let same_path = confirmed.canonical_root == expected.path;
            let result = self
                .global_store()?
                .relocate_project_if_matches(
                    &expected,
                    &authorization.name,
                    &authorization.canonical_root,
                )
                .map_err(|_| recent_projects_unavailable())?;
            Ok(match result {
                ProjectRegistryCasResult::Applied(project) => RecentProjectRegistryCommitV1 {
                    base: recent_entry_base(&self.runtime, &project),
                    status: if same_path {
                        RecentProjectRegistryStatusV1::Unchanged
                    } else {
                        RecentProjectRegistryStatusV1::Relocated
                    },
                },
                ProjectRegistryCasResult::Absent | ProjectRegistryCasResult::Stale => {
                    RecentProjectRegistryCommitV1 {
                        base: authorization.base.clone(),
                        status: RecentProjectRegistryStatusV1::Stale,
                    }
                }
            })
        })()
        .unwrap_or_else(|_: AppError| RecentProjectRegistryCommitV1 {
            base: authorization.base.clone(),
            status: RecentProjectRegistryStatusV1::Stale,
        });
        if !authorization.relocation_confirmation_token.is_empty() {
            lock(&self.confirmations).remove(&authorization.relocation_confirmation_token);
        }
        let mut completed = lock(&self.completed);
        completed.retain(|_, value| Instant::now() <= value.expires_at);
        if completed.len() >= RECENT_CONFIRMATION_LIMIT
            && let Some(oldest) = completed
                .iter()
                .min_by_key(|(_, value)| value.expires_at)
                .map(|(key, _)| key.clone())
        {
            completed.remove(&oldest);
        }
        completed.insert(
            replay_key,
            CompletedRecentOpen {
                authorization: authorization.clone(),
                commit: commit.clone(),
                expires_at: Instant::now() + RECENT_CONFIRMATION_TTL,
            },
        );
        Ok(commit)
    }

    fn forget_recent_project(
        &self,
        entry_id: &str,
        base: &str,
    ) -> AppResult<ForgetRecentProjectResultV1> {
        validate_recent_id(entry_id)?;
        validate_recent_id(base)?;
        let listed = lock(&self.listed)
            .get(entry_id)
            .filter(|listed| listed.base == base && Instant::now() <= listed.expires_at)
            .cloned()
            .ok_or_else(recent_entry_stale)?;
        let store = self.global_store()?;
        let current = store
            .project(&listed.project.path)
            .map_err(|_| recent_projects_unavailable())?;
        let Some(current) = current else {
            return Ok(ForgetRecentProjectResultV1 {
                entry_id: entry_id.to_owned(),
                registry_base: base.to_owned(),
                forgotten: true,
            });
        };
        if recent_entry_base(&self.runtime, &current) != base || current != listed.project {
            return Err(recent_entry_stale());
        }
        match store
            .forget_project_if_matches(&current)
            .map_err(|_| recent_projects_unavailable())?
        {
            ProjectRegistryCasResult::Applied(_) | ProjectRegistryCasResult::Absent => {
                Ok(ForgetRecentProjectResultV1 {
                    entry_id: entry_id.to_owned(),
                    registry_base: base.to_owned(),
                    forgotten: true,
                })
            }
            ProjectRegistryCasResult::Stale => Err(recent_entry_stale()),
        }
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

struct ProductionDesktopAuthorityState {
    runtime: Option<Arc<ActiveRuntime>>,
    factory: Option<Arc<ProductionDesktopWorkspaceFactory>>,
    recents: Option<Arc<ProductionRecentProjects>>,
    updates: Arc<dyn DesktopUpdateService>,
    initialization: Option<InitializationStatusV1>,
    initialization_goal: Option<String>,
    initialization_guide: Option<DesktopGuideManifest>,
    guide_previews: BTreeMap<String, DesktopGuideManifest>,
}

type DesktopAuthorityComponents = (
    Option<Arc<ProductionDesktopWorkspaceFactory>>,
    Option<Arc<ProductionRecentProjects>>,
    Arc<dyn DesktopUpdateService>,
);

/// Replaceable desktop authority shared by workspace, recents, updates, and
/// first-run initialization. Keeping the indirection here lets initialization
/// drop every shared cutover lease before the exclusive bootstrap transition.
pub struct ProductionDesktopAuthority {
    global_home: PathBuf,
    writer_version: String,
    events: Option<Arc<dyn DesktopEventSink>>,
    update_events: Option<Arc<dyn UpdateEventSink>>,
    initial_plan: u64,
    state: Mutex<ProductionDesktopAuthorityState>,
}

impl ProductionDesktopAuthority {
    /// Loads the current authority without creating an uninitialized home.
    ///
    /// # Errors
    /// Returns recovery-required for unsafe active state or when a production
    /// workspace/update runtime cannot be constructed.
    pub fn load(
        global_home: PathBuf,
        writer_version: impl Into<String>,
        events: Option<Arc<dyn DesktopEventSink>>,
        update_events: Option<Arc<dyn UpdateEventSink>>,
        initial_plan: u64,
    ) -> AppResult<Arc<Self>> {
        let writer_version = writer_version.into();
        let mut initialization = if global_home.exists() {
            read_desktop_initialization(&global_home)?
        } else {
            None
        };
        if let Some(journal) = &mut initialization {
            #[cfg(test)]
            run_startup_initialization_inference_hook();
            reconcile_startup_initialization(&global_home, &writer_version, journal)?;
        }
        let interrupted_bootstrap = initialization
            .as_ref()
            .is_some_and(|journal| journal.status.outcome != InitializationOutcomeV1::Complete)
            && path_is_present(&global_home.join("runtime").join(BOOTSTRAP_PLAN))?;
        let incomplete_initialization = initialization
            .as_ref()
            .is_some_and(|journal| journal.status.outcome != InitializationOutcomeV1::Complete);
        let runtime = if interrupted_bootstrap {
            None
        } else {
            match ActiveRuntime::load(&global_home, &writer_version) {
                Ok(runtime) => runtime,
                Err(_) if incomplete_initialization => None,
                Err(error) => return Err(error),
            }
        };
        let (initialization, initialization_goal, initialization_guide) = initialization
            .map_or_else(
                || (None, None, None),
                |journal| (Some(journal.status), Some(journal.goal), journal.guide),
            );
        let components = Self::components(
            runtime.as_ref(),
            events.clone(),
            update_events.clone(),
            initial_plan,
            &writer_version,
        );
        let (factory, recents, updates) = match components {
            Ok(components) => components,
            Err(_) if incomplete_initialization => (
                None,
                None,
                UnavailableUpdateService::new(&writer_version) as Arc<dyn DesktopUpdateService>,
            ),
            Err(error) => return Err(error),
        };
        Ok(Arc::new(Self {
            global_home,
            writer_version,
            events,
            update_events,
            initial_plan,
            state: Mutex::new(ProductionDesktopAuthorityState {
                runtime,
                factory,
                recents,
                updates,
                initialization,
                initialization_goal,
                initialization_guide,
                guide_previews: BTreeMap::new(),
            }),
        }))
    }

    /// Returns the currently loaded process authority, if initialized.
    #[must_use]
    pub fn active_runtime(&self) -> Option<Arc<ActiveRuntime>> {
        lock(&self.state).runtime.clone()
    }

    /// Returns a startup runtime only when no initialization recovery is pending.
    #[must_use]
    pub fn initial_workspace_runtime(&self) -> Option<Arc<ActiveRuntime>> {
        let state = lock(&self.state);
        let recovering = state
            .initialization
            .as_ref()
            .is_some_and(|status| status.outcome != InitializationOutcomeV1::Complete);
        if recovering || state.factory.is_none() {
            None
        } else {
            state.runtime.clone()
        }
    }

    fn components(
        runtime: Option<&Arc<ActiveRuntime>>,
        events: Option<Arc<dyn DesktopEventSink>>,
        update_events: Option<Arc<dyn UpdateEventSink>>,
        initial_plan: u64,
        writer_version: &str,
    ) -> AppResult<DesktopAuthorityComponents> {
        let Some(runtime) = runtime else {
            return Ok((None, None, UnavailableUpdateService::new(writer_version)));
        };
        let factory =
            ProductionDesktopWorkspaceFactory::new(Arc::clone(runtime), events, initial_plan)?;
        let recents = ProductionRecentProjects::new(Arc::clone(runtime));
        let bindings = runtime.global_bindings(runtime.global_home())?;
        let updates = UpdateRuntime::for_bindings(&bindings, update_events)
            .map_err(AppError::Message)? as Arc<dyn DesktopUpdateService>;
        Ok((Some(factory), Some(recents), updates))
    }

    fn recovery_validation(
        canonical_root: &str,
        reason: impl Into<String>,
    ) -> ProjectTargetValidationV1 {
        ProjectTargetValidationV1 {
            kind: ProjectTargetKindV1::RecoveryRequired,
            canonical_root: canonical_root.to_owned(),
            operation_id: String::new(),
            reason: reason.into(),
            initialization: None,
            goal: None,
            guide_choice: None,
        }
    }

    #[cfg(not(unix))]
    fn guide_unavailable() -> ProjectGuidePreviewV1 {
        ProjectGuidePreviewV1 {
            available: false,
            message: GUIDE_UNAVAILABLE.to_owned(),
            preview_token: String::new(),
            files: Vec::new(),
        }
    }

    #[cfg(unix)]
    fn preview_guide_inner(
        &self,
        request: &ProjectGuidePreviewRequestV1,
    ) -> AppResult<ProjectGuidePreviewV1> {
        validate_operation_id(&request.operation_id)?;
        let validation = self.validate_target_inner(Path::new(&request.root))?;
        if validation.kind != ProjectTargetKindV1::New
            || validation.operation_id != request.operation_id
            || validation.canonical_root != request.root
        {
            return Err(AppError::Message(
                "project guide preview target is stale or unsafe".to_owned(),
            ));
        }
        let root = Path::new(&request.root);
        let root_identity = PinnedProjectDirectory::identify_root(root).map_err(recovery)?;
        let template = read_guide_template(&self.global_home)?;
        let template_digest = content_digest(template.as_bytes());
        let preview_token = random_operation_id()?;
        let mut files = Vec::with_capacity(GUIDE_FILES.len());
        let mut manifests = Vec::with_capacity(GUIDE_FILES.len());
        let guide_root = PinnedGuideRoot::capture(root, root_identity)?;
        for name in GUIDE_FILES {
            let base = guide_root.read(name)?;
            let base_content = base
                .as_ref()
                .map_or("", |snapshot| snapshot.content.as_str());
            let (output, changed) = upsert_guide(base_content, &template);
            if output.len() > GUIDE_OUTPUT_LIMIT {
                return Err(AppError::Message(
                    "project guide proposed content exceeds its byte limit".to_owned(),
                ));
            }
            let action = if !changed {
                ProjectGuideFileActionV1::NoChange
            } else if base.is_some() {
                ProjectGuideFileActionV1::Update
            } else {
                ProjectGuideFileActionV1::Create
            };
            let diff = guide_diff(name, base_content, &output, action)?;
            let (additions, deletions) = guide_line_counts(base_content, &output, action);
            if additions > GUIDE_DIFF_LINE_LIMIT || deletions > GUIDE_DIFF_LINE_LIMIT {
                return Err(AppError::Message(
                    "project guide preview line count exceeds its limit".to_owned(),
                ));
            }
            files.push(ProjectGuideFilePreviewV1 {
                path: name.to_owned(),
                action,
                additions,
                deletions,
                diff,
            });
            manifests.push(DesktopGuideFileManifest {
                name: name.to_owned(),
                action,
                base_identity: base.as_ref().map(|snapshot| snapshot.identity),
                base_digest: base
                    .as_ref()
                    .map_or_else(String::new, |snapshot| snapshot.digest.clone()),
                output_digest: content_digest(output.as_bytes()),
                mode: base.as_ref().map_or(0o644, |snapshot| snapshot.mode),
            });
        }
        guide_root.verify()?;
        let manifest = DesktopGuideManifest {
            version: "1".to_owned(),
            choice: ProjectGuideChoiceV1::Install,
            operation_id: request.operation_id.clone(),
            canonical_root: request.root.clone(),
            preview_token: preview_token.clone(),
            root_identity: Some(root_identity),
            template_digest,
            files: manifests,
        };
        validate_guide_manifest(&manifest)?;
        let mut state = lock(&self.state);
        state
            .guide_previews
            .retain(|_, preview| preview.operation_id != request.operation_id);
        while state.guide_previews.len() >= GUIDE_PREVIEW_LIMIT {
            let Some(oldest) = state.guide_previews.keys().next().cloned() else {
                break;
            };
            state.guide_previews.remove(&oldest);
        }
        state.guide_previews.insert(preview_token.clone(), manifest);
        Ok(ProjectGuidePreviewV1 {
            available: true,
            message: String::new(),
            preview_token,
            files,
        })
    }

    #[allow(clippy::too_many_lines)] // Consent replacement rules are one fail-closed state table.
    fn bind_guide_manifest(
        &self,
        request: &InitializeProjectRequestV1,
        ready: &InitializationStatusV1,
    ) -> AppResult<DesktopGuideManifest> {
        let mut state = lock(&self.state);
        let existing = state.initialization_guide.clone();
        let immutable_root_identity = existing.as_ref().and_then(|guide| guide.root_identity);
        let selected_root_identity =
            || PinnedProjectDirectory::identify_root(Path::new(&request.root)).map_err(recovery);
        let postcommit_refresh_allowed = ready.checkpoint
            == InitializationCheckpointV1::ProjectCommitted
            && ready.outcome == InitializationOutcomeV1::RecoveryRequired
            && matches!(
                ready.error_kind.as_str(),
                GUIDE_PREVIEW_STALE | GUIDE_PARTIALLY_APPLIED
            );
        let stale_skip_allowed = stale_guide_skip_allowed(ready);
        let precommit_refresh_allowed = ready.checkpoint == InitializationCheckpointV1::None
            && ready.outcome == InitializationOutcomeV1::Ready
            && ready.error_kind == GUIDE_PREVIEW_STALE;
        let selected = match (existing, request.guide_choice) {
            (None, ProjectGuideChoiceV1::Skip) => DesktopGuideManifest::skip(
                request.operation_id.clone(),
                request.root.clone(),
                selected_root_identity()?,
            ),
            (None, ProjectGuideChoiceV1::Install) => state
                .guide_previews
                .remove(&request.guide_preview_token)
                .ok_or_else(|| AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))?,
            (Some(existing), choice)
                if existing.choice == choice
                    && existing.preview_token == request.guide_preview_token =>
            {
                existing
            }
            (Some(existing), ProjectGuideChoiceV1::Install)
                if existing.choice == ProjectGuideChoiceV1::Skip =>
            {
                return Err(AppError::Message(
                    "skipped project guidance cannot be upgraded for this operation".to_owned(),
                ));
            }
            (Some(existing), ProjectGuideChoiceV1::Skip) if stale_skip_allowed => {
                if ready.checkpoint == InitializationCheckpointV1::ProjectCommitted
                    && guide_manifest_has_applied_output(&existing)?
                {
                    let status = InitializationStatusV1 {
                        error_kind: GUIDE_PARTIALLY_APPLIED.to_owned(),
                        ..ready.clone()
                    };
                    drop(state);
                    self.record_initialization_status(status, &request.goal)?;
                    return Err(AppError::Message(GUIDE_PARTIALLY_APPLIED.to_owned()));
                }
                DesktopGuideManifest::skip(
                    request.operation_id.clone(),
                    request.root.clone(),
                    selected_root_identity()?,
                )
            }
            (Some(_), ProjectGuideChoiceV1::Install) if postcommit_refresh_allowed => state
                .guide_previews
                .remove(&request.guide_preview_token)
                .ok_or_else(|| AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))?,
            (Some(_), ProjectGuideChoiceV1::Install) if precommit_refresh_allowed => state
                .guide_previews
                .remove(&request.guide_preview_token)
                .ok_or_else(|| AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))?,
            (Some(existing), ProjectGuideChoiceV1::Install)
                if existing.choice == ProjectGuideChoiceV1::Install
                    && !matches!(
                        ready.checkpoint,
                        InitializationCheckpointV1::GuideApplied
                            | InitializationCheckpointV1::DesktopBound
                    ) =>
            {
                state
                    .guide_previews
                    .remove(&request.guide_preview_token)
                    .ok_or_else(|| AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))?
            }
            (Some(_), _) => {
                return Err(AppError::Message(
                    "project guide choice does not match its durable request".to_owned(),
                ));
            }
        };
        validate_guide_manifest(&selected)?;
        if immutable_root_identity.is_some() && selected.root_identity != immutable_root_identity {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        if selected.operation_id != request.operation_id
            || selected.canonical_root != request.root
            || selected.choice != request.guide_choice
        {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        Ok(selected)
    }

    #[allow(clippy::too_many_lines)] // Classification order is the fail-closed safety contract.
    fn validate_target_inner(&self, selected: &Path) -> AppResult<ProjectTargetValidationV1> {
        let canonical = fs::canonicalize(selected)?;
        let canonical_text = canonical.to_str().ok_or_else(|| {
            AppError::Message("the selected target path is not valid UTF-8".to_owned())
        })?;
        if !canonical.is_dir() {
            return Ok(Self::recovery_validation(
                canonical_text,
                "the selected target is not a directory",
            ));
        }
        let (mut runtime, initialization, initialization_goal, initialization_guide) = {
            let state = lock(&self.state);
            (
                state.runtime.clone(),
                state.initialization.clone(),
                state.initialization_goal.clone(),
                state.initialization_guide.clone(),
            )
        };
        if runtime.is_none()
            && initialization.as_ref().is_some_and(|status| {
                initialization_checkpoint_rank(status.checkpoint)
                    >= initialization_checkpoint_rank(InitializationCheckpointV1::RuntimeCommitted)
            })
        {
            runtime = ActiveRuntime::load(&self.global_home, &self.writer_version)
                .ok()
                .flatten();
        }
        let resumable = initialization.as_ref().filter(|status| {
            status.outcome != InitializationOutcomeV1::Complete
                && status.canonical_root == canonical_text
        });
        let resume_metadata =
            |status: &InitializationStatusV1| match (&initialization_goal, &initialization_guide) {
                (Some(goal), Some(guide)) => {
                    (Some(status.clone()), Some(goal.clone()), Some(guide.choice))
                }
                _ => (None, None, None),
            };
        if let Some(status) = &resumable {
            let plan_path = self.global_home.join("runtime").join(BOOTSTRAP_PLAN);
            if path_is_present(&plan_path)? {
                let plan = read_bootstrap_plan(&plan_path)?;
                let home = fs::canonicalize(&self.global_home)?;
                validate_bootstrap_plan(&home, &canonical, &plan, &self.writer_version)?;
                if plan.operation_id.as_deref() != Some(status.operation_id.as_str()) {
                    return Ok(Self::recovery_validation(
                        canonical_text,
                        "bootstrap storage is not bound to this initialization operation",
                    ));
                }
                let (initialization, goal, guide_choice) = resume_metadata(status);
                return Ok(ProjectTargetValidationV1 {
                    kind: ProjectTargetKindV1::New,
                    canonical_root: status.canonical_root.clone(),
                    operation_id: status.operation_id.clone(),
                    reason: String::new(),
                    initialization,
                    goal,
                    guide_choice,
                });
            }
            if status.checkpoint != InitializationCheckpointV1::None {
                if runtime.as_ref().is_some_and(|runtime| {
                    runtime
                        .bindings_for_exact_root(&canonical)
                        .is_ok_and(|bindings| bindings.project.is_some())
                }) {
                    let (initialization, goal, guide_choice) = resume_metadata(status);
                    return Ok(ProjectTargetValidationV1 {
                        kind: ProjectTargetKindV1::New,
                        canonical_root: status.canonical_root.clone(),
                        operation_id: status.operation_id.clone(),
                        reason: String::new(),
                        initialization,
                        goal,
                        guide_choice,
                    });
                }
                return Ok(Self::recovery_validation(
                    canonical_text,
                    "the interrupted initialization cannot be resumed safely",
                ));
            }
            if selected_project_directory_present(&canonical) {
                return Ok(Self::recovery_validation(
                    canonical_text,
                    "interrupted project storage requires recovery",
                ));
            }
        } else if initialization.as_ref().is_some_and(|status| {
            status.outcome != InitializationOutcomeV1::Complete
                && status.checkpoint != InitializationCheckpointV1::None
        }) {
            return Ok(Self::recovery_validation(
                canonical_text,
                "another project has an incomplete initialization",
            ));
        }
        if let Some(runtime) = runtime
            && let Some(project) = runtime.bindings_for(&canonical)?.project
        {
            return Ok(ProjectTargetValidationV1 {
                kind: ProjectTargetKindV1::Existing,
                canonical_root: project
                    .root
                    .to_str()
                    .ok_or_else(|| recovery("registered project root is not valid UTF-8"))?
                    .to_owned(),
                operation_id: String::new(),
                reason: String::new(),
                initialization: None,
                goal: None,
                guide_choice: None,
            });
        }
        let global_homes = global_home_exemptions(&self.global_home);
        // A home directory — p-track's or the user's own — is refused by
        // name rather than read as a foreign project store.
        if let Some(refusal) = home_project_refusal(&canonical, &global_homes) {
            return Ok(Self::recovery_validation(canonical_text, refusal));
        }
        for (depth, ancestor) in canonical.ancestors().enumerate() {
            let storage = ancestor.join(".ptrack");
            // Depth 0 is the selected root itself, which never gets the exemption:
            // its own `.ptrack` must not be the global home.
            if depth > 0 && is_global_home(&storage, &global_homes) {
                continue;
            }
            if path_is_present(&storage.join("ptrack.redb"))? {
                // A store at the selected root itself is the moved-project
                // shape. The hint never opens the file: relocation fail-closes
                // on anything that is not a genuinely moved store, and this
                // walk must stay a read-only classification.
                let reason = if depth == 0 {
                    "an unregistered project store requires recovery; a moved project can be re-registered by quitting p-track and running 'ptrack relocate' in the project folder"
                } else {
                    "an unregistered project store requires recovery"
                };
                return Ok(Self::recovery_validation(canonical_text, reason));
            }
            if let Ok(metadata) = fs::symlink_metadata(&storage) {
                return Ok(Self::recovery_validation(
                    canonical_text,
                    if metadata.file_type().is_symlink() || !metadata.is_dir() {
                        "project storage is unsafe"
                    } else {
                        "preexisting project storage requires recovery"
                    },
                ));
            }
        }
        // A bound runtime is the proof the global state is healthy. With no
        // bound runtime, the database's presence means a store the runtime
        // refused, and a leftover bootstrap plan is an interrupted bootstrap
        // either way.
        if (lock(&self.state).runtime.is_none()
            && path_is_present(&self.global_home.join("global.redb"))?)
            || path_is_present(&self.global_home.join("runtime").join(BOOTSTRAP_PLAN))?
        {
            return Ok(Self::recovery_validation(
                canonical_text,
                "global runtime state requires recovery",
            ));
        }
        let (initialization, goal, guide_choice) =
            resumable.map_or((None, None, None), resume_metadata);
        Ok(ProjectTargetValidationV1 {
            kind: ProjectTargetKindV1::New,
            canonical_root: canonical_text.to_owned(),
            operation_id: resumable.map_or_else(random_operation_id, |status| {
                Ok(status.operation_id.clone())
            })?,
            reason: String::new(),
            initialization,
            goal,
            guide_choice,
        })
    }

    fn record_initialization_status(
        &self,
        status: InitializationStatusV1,
        goal: &str,
    ) -> AppResult<InitializationStatusV1> {
        let guide = lock(&self.state).initialization_guide.clone();
        publish_desktop_initialization(&self.global_home, &status, goal, guide.as_ref())?;
        let mut state = lock(&self.state);
        state.initialization = Some(status.clone());
        state.initialization_goal = Some(goal.to_owned());
        Ok(status)
    }

    fn record_recovery_if_owned(
        &self,
        status: &InitializationStatusV1,
        goal: &str,
    ) -> AppResult<Option<DesktopInitializationJournal>> {
        if !path_is_present(&self.global_home.join("runtime"))? {
            return Ok(None);
        }
        with_desktop_initialization_lock(&self.global_home, || {
            let Some(journal) = read_desktop_initialization(&self.global_home)? else {
                return Ok(None);
            };
            if journal.status.operation_id != status.operation_id || journal.goal != goal {
                return Ok(None);
            }
            validate_desktop_initialization_transition(&journal.status, status)?;
            publish_desktop_initialization_locked(
                &self.global_home,
                status,
                goal,
                journal.guide.as_ref(),
            )?;
            Ok(Some(DesktopInitializationJournal {
                format: journal.format,
                version: journal.version,
                status: status.clone(),
                goal: goal.to_owned(),
                guide: journal.guide,
            }))
        })
    }

    fn reconcile_recovery_status(
        &self,
        proposed: InitializationStatusV1,
        goal: &str,
    ) -> (InitializationStatusV1, String, Option<DesktopGuideManifest>) {
        if let Ok(Some(journal)) = self.record_recovery_if_owned(&proposed, goal) {
            return (journal.status, journal.goal, journal.guide);
        }
        let local_guide = lock(&self.state).initialization_guide.clone();
        match read_desktop_initialization(&self.global_home) {
            Ok(Some(journal)) => (journal.status, journal.goal, journal.guide),
            Ok(None) | Err(_) => (proposed, goal.to_owned(), local_guide),
        }
    }

    #[allow(clippy::too_many_lines)] // One exclusive-lease transaction preserves checkpoint order.
    fn commit_initialization(
        &self,
        request: &InitializeProjectRequestV1,
        ready: &InitializationStatusV1,
    ) -> AppResult<InitializationStatusV1> {
        let guide = lock(&self.state)
            .initialization_guide
            .clone()
            .ok_or_else(|| recovery("project guide choice is not bound to initialization"))?;
        let root = PathBuf::from(&ready.canonical_root);
        let root_identity = directory_identity(&root)?;
        let pinned_root_identity =
            PinnedProjectDirectory::identify_root(&root).map_err(recovery)?;
        if ready.checkpoint == InitializationCheckpointV1::None {
            require_new_project_storage_absent(&root, &self.global_home)?;
        }
        ensure_private_home(&self.global_home)?;
        let home = fs::canonicalize(&self.global_home)?;
        let lease = acquire_cutover_lock(&home, CutoverLockMode::Exclusive).map_err(recovery)?;
        if let Some(journal) = read_desktop_initialization(&home)? {
            if journal.status.outcome != InitializationOutcomeV1::Complete
                && journal.status.operation_id != request.operation_id
                && journal.status.checkpoint != InitializationCheckpointV1::None
            {
                return Err(recovery("another initialization operation is incomplete"));
            }
            if journal.status.operation_id == request.operation_id && journal.goal != request.goal {
                return Err(recovery(
                    "initialization goal does not match the durable operation",
                ));
            }
        }
        let existing = load_active_generation(&home, &lease).map_err(recovery)?;
        if let Some(marker) = &existing {
            validate_active_generation(&home, marker, &self.writer_version).map_err(recovery)?;
        }
        if ready.checkpoint == InitializationCheckpointV1::None {
            if existing.as_ref().is_some_and(|marker| {
                marker
                    .projects
                    .iter()
                    .any(|project| root.starts_with(Path::new(&project.root)))
            }) {
                return Err(recovery("selected project root is already registered"));
            }
            require_new_project_storage_absent(&root, &self.global_home)?;
            if existing.is_none() && path_is_present(&home.join("global.redb"))? {
                return Err(recovery("global runtime state changed before commit"));
            }
        }
        if ready.checkpoint != InitializationCheckpointV1::GuideApplied {
            #[cfg(test)]
            run_guide_before_commit_hook();
            validate_guide_before_commit(&home, &guide)?;
        }
        let started = InitializationStatusV1 {
            operation_id: request.operation_id.clone(),
            canonical_root: ready.canonical_root.clone(),
            checkpoint: ready.checkpoint,
            outcome: InitializationOutcomeV1::InProgress,
            error_kind: String::new(),
        };
        self.record_initialization_status(started, &request.goal)?;
        #[cfg(test)]
        run_initialization_after_started_hook();
        let plan_path = home.join("runtime").join(BOOTSTRAP_PLAN);
        let (existing_plan, pinned_project) = if path_is_present(&plan_path)? {
            let plan = read_bootstrap_plan(&plan_path)?;
            validate_bootstrap_plan(&home, &root, &plan, &self.writer_version)?;
            if plan.operation_id.as_deref() != Some(request.operation_id.as_str()) {
                return Err(recovery(
                    "bootstrap plan is not bound to this initialization operation",
                ));
            }
            let pinned = PinnedProjectDirectory::prepare_expected_identities(
                &root,
                plan.project_root_identity,
                plan.project_directory_identity,
            )
            .map_err(recovery)?;
            (Some(plan), pinned)
        } else if matches!(
            ready.checkpoint,
            InitializationCheckpointV1::RuntimeCommitted
                | InitializationCheckpointV1::ProjectCommitted
                | InitializationCheckpointV1::GuideApplied
        ) {
            let expected_root = guide
                .root_identity
                .ok_or_else(|| recovery("project guide root identity is missing"))?;
            (
                None,
                PinnedProjectDirectory::prepare_expected(&root, expected_root).map_err(recovery)?,
            )
        } else {
            (
                None,
                PinnedProjectDirectory::prepare_new_expected(&root, pinned_root_identity)
                    .map_err(recovery)?,
            )
        };
        if ready.checkpoint == InitializationCheckpointV1::GuideApplied {
            let marker = existing
                .as_ref()
                .ok_or_else(|| recovery("committed initialization marker is missing"))?;
            if existing_plan
                .as_ref()
                .is_some_and(|plan| plan.target_marker != *marker)
            {
                return Err(recovery(
                    "bootstrap plan does not match the committed marker",
                ));
            }
            pinned_project.verify().map_err(recovery)?;
            if existing_plan.is_some() {
                clear_bootstrap_plan(&plan_path)?;
            }
            return Ok(ready.clone());
        }
        if ready.checkpoint == InitializationCheckpointV1::ProjectCommitted {
            let marker = existing
                .as_ref()
                .ok_or_else(|| recovery("committed initialization marker is missing"))?;
            if existing_plan
                .as_ref()
                .is_some_and(|plan| plan.target_marker != *marker)
            {
                return Err(recovery(
                    "bootstrap plan does not match the committed marker",
                ));
            }
            let generation = marker.generation_number()?;
            let project = marker
                .projects
                .iter()
                .find(|project| project.root == ready.canonical_root)
                .ok_or_else(|| recovery("committed initialization project is missing"))?;
            let binding = binding_for_new(
                generation,
                project.database_id.clone(),
                StoreKind::Project,
                Path::new(&project.path),
            )?;
            require_directory_identity(&root, root_identity)?;
            pinned_project.verify().map_err(recovery)?;
            let committed_store =
                ProjectStore::open_existing_pinned(&pinned_project, &binding, &self.writer_version)
                    .map_err(recovery)?;
            let committed_goal = committed_store.meta().map_err(recovery)?.goal;
            drop(committed_store);
            pinned_project.verify().map_err(recovery)?;
            if committed_goal != request.goal {
                return Err(recovery("committed initialization goal changed"));
            }
            if existing_plan.is_some() {
                clear_bootstrap_plan(&plan_path)?;
            }
            apply_guide_manifest(&home, &guide, &pinned_project)?;
            return self.record_initialization_status(
                InitializationStatusV1 {
                    checkpoint: InitializationCheckpointV1::GuideApplied,
                    outcome: InitializationOutcomeV1::InProgress,
                    error_kind: String::new(),
                    ..ready.clone()
                },
                &request.goal,
            );
        }
        let plan = if let Some(plan) = existing_plan {
            Some(plan)
        } else if existing.as_ref().is_some_and(|marker| {
            marker
                .projects
                .iter()
                .any(|project| project.root == ready.canonical_root)
        }) && ready.checkpoint != InitializationCheckpointV1::None
        {
            None
        } else {
            require_directory_identity(&root, root_identity)?;
            let plan = new_bootstrap_plan(
                &home,
                &root,
                pinned_project.root_identity(),
                pinned_project.directory_identity(),
                existing.clone(),
                Some(request.operation_id.clone()),
            )?;
            publish_bootstrap_plan(&plan_path, &plan)?;
            #[cfg(test)]
            run_initialization_after_bootstrap_plan_hook();
            Some(plan)
        };

        let prepared = InitializationStatusV1 {
            operation_id: request.operation_id.clone(),
            canonical_root: ready.canonical_root.clone(),
            checkpoint: InitializationCheckpointV1::Prepared,
            outcome: InitializationOutcomeV1::InProgress,
            error_kind: String::new(),
        };
        if plan.is_some() {
            self.record_initialization_status(prepared, &request.goal)?;
        }

        let marker = if let Some(plan) = &plan {
            if existing.as_ref() != Some(&plan.target_marker) {
                if existing != plan.previous_marker {
                    return Err(recovery(
                        "bootstrap plan does not match the active-generation marker",
                    ));
                }
                require_directory_identity(&root, root_identity)?;
                ensure_bootstrap_stores(&home, plan, &self.writer_version, Some(&pinned_project))?;
                install_active_generation(&home, &lease, &plan.target_marker, &self.writer_version)
                    .map_err(recovery)?;
                pinned_project.verify().map_err(recovery)?;
            }
            plan.target_marker.clone()
        } else {
            existing.ok_or_else(|| recovery("committed initialization marker is missing"))?
        };

        let runtime_committed = InitializationStatusV1 {
            operation_id: request.operation_id.clone(),
            canonical_root: ready.canonical_root.clone(),
            checkpoint: InitializationCheckpointV1::RuntimeCommitted,
            outcome: InitializationOutcomeV1::InProgress,
            error_kind: String::new(),
        };
        self.record_initialization_status(runtime_committed, &request.goal)?;

        let generation = marker.generation_number()?;
        let project = marker
            .projects
            .iter()
            .find(|project| project.root == ready.canonical_root)
            .ok_or_else(|| recovery("committed initialization project is missing"))?;
        let project_binding = binding_for_new(
            generation,
            project.database_id.clone(),
            StoreKind::Project,
            Path::new(&project.path),
        )?;
        require_directory_identity(&root, root_identity)?;
        pinned_project.verify().map_err(recovery)?;
        let project_store = ProjectStore::open_existing_pinned(
            &pinned_project,
            &project_binding,
            &self.writer_version,
        )
        .map_err(recovery)?;
        project_store
            .set_goal(request.goal.clone())
            .map_err(recovery)?;
        drop(project_store);
        pinned_project.verify().map_err(recovery)?;

        let global_binding = binding_for_new(
            generation,
            marker.global.database_id.clone(),
            StoreKind::Global,
            Path::new(&marker.global.path),
        )?;
        if let Ok(global_store) = GlobalStore::open_existing(&marker.global.path, &global_binding) {
            let name = root
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            let _ = global_store.register_project(name, &root);
        }
        if plan.is_some() {
            clear_bootstrap_plan(&plan_path)?;
        }
        let project_committed = InitializationStatusV1 {
            operation_id: request.operation_id.clone(),
            canonical_root: ready.canonical_root.clone(),
            checkpoint: InitializationCheckpointV1::ProjectCommitted,
            outcome: InitializationOutcomeV1::InProgress,
            error_kind: String::new(),
        };
        self.record_initialization_status(project_committed, &request.goal)?;
        apply_guide_manifest(&home, &guide, &pinned_project)?;
        self.record_initialization_status(
            InitializationStatusV1 {
                operation_id: request.operation_id.clone(),
                canonical_root: ready.canonical_root.clone(),
                checkpoint: InitializationCheckpointV1::GuideApplied,
                outcome: InitializationOutcomeV1::InProgress,
                error_kind: String::new(),
            },
            &request.goal,
        )
    }

    fn install_reloaded_authority(&self, initialization: InitializationStatusV1) -> AppResult<()> {
        let runtime = ActiveRuntime::load(&self.global_home, &self.writer_version)?;
        let (factory, recents, updates) = Self::components(
            runtime.as_ref(),
            self.events.clone(),
            self.update_events.clone(),
            self.initial_plan,
            &self.writer_version,
        )?;
        updates.start().map_err(AppError::Message)?;
        let mut state = lock(&self.state);
        state.runtime = runtime;
        state.factory = factory;
        state.recents = recents;
        state.updates = updates;
        state.initialization = Some(initialization);
        Ok(())
    }

    fn derive_initialization_checkpoint(
        &self,
        root: &Path,
        goal: &str,
    ) -> InitializationCheckpointV1 {
        if let Ok(Some(runtime)) = ActiveRuntime::load(&self.global_home, &self.writer_version) {
            if let Ok(bindings) = runtime.bindings_for_exact_root(root)
                && let Some(endpoint) = bindings.project
            {
                let committed = ProjectStore::open_existing(
                    &endpoint.database,
                    &endpoint.binding,
                    &self.writer_version,
                )
                .and_then(|store| store.meta())
                .is_ok_and(|meta| meta.goal == goal);
                return if committed {
                    InitializationCheckpointV1::ProjectCommitted
                } else {
                    InitializationCheckpointV1::RuntimeCommitted
                };
            }
            return if selected_project_storage_present(root) {
                InitializationCheckpointV1::RuntimeCommitted
            } else if selected_project_directory_present(root) {
                InitializationCheckpointV1::Prepared
            } else {
                InitializationCheckpointV1::None
            };
        }
        if path_is_present(&self.global_home.join("runtime").join(BOOTSTRAP_PLAN)).unwrap_or(false)
        {
            InitializationCheckpointV1::Prepared
        } else if selected_project_storage_present(root) {
            InitializationCheckpointV1::RuntimeCommitted
        } else if selected_project_directory_present(root) {
            InitializationCheckpointV1::Prepared
        } else {
            InitializationCheckpointV1::None
        }
    }

    fn refresh_durable_initialization(&self) -> AppResult<()> {
        let durable_required = lock(&self.state)
            .initialization
            .as_ref()
            .is_some_and(|status| {
                status.checkpoint != InitializationCheckpointV1::None
                    || status.outcome == InitializationOutcomeV1::Complete
            });
        let journal_path = self
            .global_home
            .join("runtime")
            .join(DESKTOP_INITIALIZATION);
        if !self.global_home.exists() || !path_is_present(&journal_path)? {
            return if durable_required {
                Err(recovery("desktop initialization status disappeared"))
            } else {
                Ok(())
            };
        }
        let journal = with_desktop_initialization_lock(&self.global_home, || {
            read_desktop_initialization(&self.global_home)
        })?;
        let journal =
            journal.ok_or_else(|| recovery("desktop initialization status disappeared"))?;
        let reload_components = journal.status.outcome == InitializationOutcomeV1::Complete
            && lock(&self.state).factory.is_none();
        if reload_components {
            self.install_reloaded_authority(journal.status.clone())?;
        }
        let mut state = lock(&self.state);
        state.initialization = Some(journal.status);
        state.initialization_goal = Some(journal.goal);
        state.initialization_guide = journal.guide;
        Ok(())
    }
}

/// Builds the production desktop runtime used by the native shell and
/// headless smoke tests from one authority graph.
///
/// # Errors
/// Returns authority, binding, or initial workspace construction failures.
pub fn production_desktop_runtime(
    global_home: PathBuf,
    writer_version: impl Into<String>,
    current: &Path,
    events: Option<Arc<dyn DesktopEventSink>>,
    initial_plan: u64,
) -> AppResult<Arc<DesktopRuntime>> {
    let writer_version = writer_version.into();
    let update_events = events
        .as_ref()
        .map(|sink| DesktopUpdateEventSink::new(Arc::clone(sink)) as Arc<dyn UpdateEventSink>);
    let authority = ProductionDesktopAuthority::load(
        global_home,
        writer_version.clone(),
        events.clone(),
        update_events,
        initial_plan,
    )?;
    let mut config = DesktopRuntimeConfig::unavailable(writer_version);
    config.factory = authority.clone();
    config.recent_projects = authority.clone();
    config.initialization = authority.clone();
    config.update_service = authority.clone();
    if let Some(runtime) = authority.initial_workspace_runtime() {
        match runtime.bindings_for(current) {
            Ok(bindings) => {
                if let Some(project) = bindings.project {
                    config.initial_workspace = Some(authority.build(&project.root, 1)?);
                }
            }
            Err(AppError::NoProject) => {}
            Err(error) => return Err(error),
        }
    }
    config.event_sink = events;
    Ok(DesktopRuntime::new(config))
}

impl DesktopWorkspaceFactory for ProductionDesktopAuthority {
    fn build(&self, root: &Path, generation: u64) -> AppResult<Arc<dyn DesktopWorkspace>> {
        let factory = lock(&self.state)
            .factory
            .clone()
            .ok_or_else(uninitialized)?;
        factory.build(root, generation)
    }
}

impl RecentProjectsProvider for ProductionDesktopAuthority {
    fn recent_projects(&self) -> AppResult<Vec<Value>> {
        let recents = lock(&self.state).recents.clone();
        recents.map_or_else(|| Ok(Vec::new()), |recents| recents.recent_projects())
    }

    fn recent_projects_v1(&self) -> AppResult<RecentProjectsV1> {
        let recents = lock(&self.state).recents.clone();
        recents.map_or_else(
            || {
                Ok(RecentProjectsV1 {
                    projects: Vec::new(),
                })
            },
            |recents| recents.recent_projects_v1(),
        )
    }

    fn resolve_recent_project(
        &self,
        entry_id: &str,
        base: &str,
        candidate: &Path,
    ) -> AppResult<ResolvedRecentProjectV1> {
        lock(&self.state)
            .recents
            .clone()
            .ok_or_else(uninitialized)?
            .resolve_recent_project(entry_id, base, candidate)
    }

    fn authorize_recent_project_open(
        &self,
        entry_id: &str,
        base: &str,
        canonical_root: &Path,
        relocation_confirmation_token: &str,
    ) -> AppResult<RecentProjectOpenAuthorizationV1> {
        lock(&self.state)
            .recents
            .clone()
            .ok_or_else(uninitialized)?
            .authorize_recent_project_open(
                entry_id,
                base,
                canonical_root,
                relocation_confirmation_token,
            )
    }

    fn finish_recent_project_open(
        &self,
        authorization: &RecentProjectOpenAuthorizationV1,
    ) -> AppResult<RecentProjectRegistryCommitV1> {
        lock(&self.state)
            .recents
            .clone()
            .ok_or_else(uninitialized)?
            .finish_recent_project_open(authorization)
    }

    fn forget_recent_project(
        &self,
        entry_id: &str,
        base: &str,
    ) -> AppResult<ForgetRecentProjectResultV1> {
        lock(&self.state)
            .recents
            .clone()
            .ok_or_else(uninitialized)?
            .forget_recent_project(entry_id, base)
    }
}

impl DesktopInitializationService for ProductionDesktopAuthority {
    fn validate_target(&self, selected: &Path) -> AppResult<ProjectTargetValidationV1> {
        let validation = self.validate_target_inner(selected)?;
        if validation.kind == ProjectTargetKindV1::New {
            let mut state = lock(&self.state);
            if state
                .initialization
                .as_ref()
                .is_none_or(|status| status.operation_id != validation.operation_id)
            {
                state.initialization = Some(InitializationStatusV1 {
                    operation_id: validation.operation_id.clone(),
                    canonical_root: validation.canonical_root.clone(),
                    checkpoint: InitializationCheckpointV1::None,
                    outcome: InitializationOutcomeV1::Ready,
                    error_kind: String::new(),
                });
                state.initialization_goal = None;
            }
        }
        Ok(validation)
    }

    fn preview_guide(
        &self,
        request: &ProjectGuidePreviewRequestV1,
    ) -> AppResult<ProjectGuidePreviewV1> {
        #[cfg(unix)]
        {
            self.preview_guide_inner(request)
        }
        #[cfg(not(unix))]
        {
            let _ = request;
            Ok(Self::guide_unavailable())
        }
    }

    #[allow(clippy::too_many_lines)] // Quiesce, restore, and recovery paths share one authority swap.
    fn initialize(
        &self,
        request: &InitializeProjectRequestV1,
    ) -> AppResult<InitializationStatusV1> {
        validate_operation_id(&request.operation_id)?;
        let (ready, bound_goal, bound_guide) = {
            let state = lock(&self.state);
            let ready = state
                .initialization
                .clone()
                .filter(|status| status.operation_id == request.operation_id)
                .ok_or_else(|| {
                    AppError::Message("initialization operation is unknown".to_owned())
                })?;
            (
                ready,
                state.initialization_goal.clone(),
                state.initialization_guide.clone(),
            )
        };
        if bound_goal
            .as_deref()
            .is_some_and(|goal| goal != request.goal)
        {
            return Err(AppError::Message(
                "initialization operation goal does not match its durable request".to_owned(),
            ));
        }
        if ready.checkpoint == InitializationCheckpointV1::DesktopBound {
            let guide = bound_guide
                .ok_or_else(|| recovery("completed initialization guide manifest is missing"))?;
            if ready.outcome != InitializationOutcomeV1::Complete
                || ready.canonical_root != request.root
                || guide.operation_id != request.operation_id
                || guide.canonical_root != request.root
                || guide.choice != request.guide_choice
                || guide.preview_token != request.guide_preview_token
            {
                return Err(AppError::Message(
                    "initialization operation request does not match its durable request"
                        .to_owned(),
                ));
            }
            return Ok(ready);
        }
        if ready.checkpoint != InitializationCheckpointV1::GuideApplied {
            match request.guide_choice {
                ProjectGuideChoiceV1::Skip if !request.guide_preview_token.is_empty() => {
                    return Err(AppError::Message(
                        "skipping project guidance requires an empty preview token".to_owned(),
                    ));
                }
                ProjectGuideChoiceV1::Install
                    if validate_operation_id(&request.guide_preview_token).is_err() =>
                {
                    return Err(AppError::Message(
                        "installing project guidance requires a valid preview token".to_owned(),
                    ));
                }
                ProjectGuideChoiceV1::Skip | ProjectGuideChoiceV1::Install => {}
            }
            #[cfg(not(unix))]
            if request.guide_choice == ProjectGuideChoiceV1::Install {
                return Err(AppError::Message("project-guide-unavailable".to_owned()));
            }
        }
        let validation = match self.validate_target_inner(Path::new(&request.root)) {
            Ok(validation) => validation,
            Err(error) => {
                let status = InitializationStatusV1 {
                    operation_id: request.operation_id.clone(),
                    canonical_root: request.root.clone(),
                    checkpoint: ready.checkpoint,
                    outcome: if ready.checkpoint == InitializationCheckpointV1::None {
                        InitializationOutcomeV1::Ready
                    } else {
                        InitializationOutcomeV1::RecoveryRequired
                    },
                    error_kind: initialization_error_kind(&error).to_owned(),
                };
                if ready.checkpoint == InitializationCheckpointV1::None {
                    let mut state = lock(&self.state);
                    if state
                        .initialization
                        .as_ref()
                        .is_some_and(|current| current.operation_id == request.operation_id)
                    {
                        state.initialization = Some(status.clone());
                    }
                } else {
                    self.record_initialization_status(status.clone(), &request.goal)?;
                }
                return Err(AppError::Message(status.error_kind));
            }
        };
        if validation.kind != ProjectTargetKindV1::New
            || validation.canonical_root != request.root
            || ready.canonical_root != request.root
        {
            if ready.checkpoint == InitializationCheckpointV1::None {
                let mut state = lock(&self.state);
                if state
                    .initialization
                    .as_ref()
                    .is_some_and(|status| status.operation_id == request.operation_id)
                {
                    state.initialization = None;
                    state.initialization_goal = None;
                }
            }
            return Err(AppError::Message(
                "project initialization request is stale or unsafe".to_owned(),
            ));
        }
        let guide = if ready.checkpoint == InitializationCheckpointV1::GuideApplied {
            lock(&self.state)
                .initialization_guide
                .clone()
                .ok_or_else(|| recovery("applied project guide manifest is missing"))?
        } else {
            self.bind_guide_manifest(request, &ready)?
        };
        if ready.checkpoint != InitializationCheckpointV1::GuideApplied
            && let Err(error) = validate_guide_before_commit(&self.global_home, &guide)
        {
            let status = InitializationStatusV1 {
                operation_id: request.operation_id.clone(),
                canonical_root: request.root.clone(),
                checkpoint: ready.checkpoint,
                outcome: if ready.checkpoint == InitializationCheckpointV1::None {
                    InitializationOutcomeV1::Ready
                } else {
                    InitializationOutcomeV1::RecoveryRequired
                },
                error_kind: GUIDE_PREVIEW_STALE.to_owned(),
            };
            let durable = path_is_present(
                &self
                    .global_home
                    .join("runtime")
                    .join(DESKTOP_INITIALIZATION),
            )?;
            if durable {
                self.record_initialization_status(status, &request.goal)?;
            } else {
                lock(&self.state).initialization = Some(status);
            }
            return Err(error);
        }
        lock(&self.state).initialization_guide = Some(guide);
        let started = InitializationStatusV1 {
            operation_id: request.operation_id.clone(),
            canonical_root: validation.canonical_root.clone(),
            checkpoint: ready.checkpoint,
            outcome: InitializationOutcomeV1::InProgress,
            error_kind: String::new(),
        };
        let (old_runtime, old_factory, old_recents, old_updates) = {
            let mut state = lock(&self.state);
            state.initialization = Some(started);
            state.initialization_goal = Some(request.goal.clone());
            (
                state.runtime.take(),
                state.factory.take(),
                state.recents.take(),
                std::mem::replace(
                    &mut state.updates,
                    UnavailableUpdateService::new(&self.writer_version),
                ),
            )
        };
        if old_updates.shutdown().is_err() {
            let status = InitializationStatusV1 {
                operation_id: request.operation_id.clone(),
                canonical_root: request.root.clone(),
                checkpoint: ready.checkpoint,
                outcome: if ready.checkpoint == InitializationCheckpointV1::None {
                    InitializationOutcomeV1::Ready
                } else {
                    InitializationOutcomeV1::RecoveryRequired
                },
                error_kind: "authority-shutdown-failed".to_owned(),
            };
            let mut state = lock(&self.state);
            state.runtime = old_runtime;
            state.factory = old_factory;
            state.recents = old_recents;
            state.updates = Arc::clone(&old_updates);
            drop(state);
            let (status, goal, guide) = self.reconcile_recovery_status(status, &request.goal);
            let mut state = lock(&self.state);
            state.initialization = Some(status);
            state.initialization_goal = Some(goal);
            state.initialization_guide = guide;
            return Err(AppError::Message(
                "desktop runtime authority could not be quiesced".to_owned(),
            ));
        }
        drop(old_updates);
        drop(old_recents);
        drop(old_factory);
        drop(old_runtime);

        let initialized = (|| -> AppResult<InitializationStatusV1> {
            #[cfg(test)]
            run_initialization_before_commit_hook();
            let status = self.commit_initialization(request, &ready)?;
            self.install_reloaded_authority(status.clone())?;
            Ok(status)
        })();
        if let Err(error) = &initialized {
            let checkpoint =
                self.derive_initialization_checkpoint(Path::new(&request.root), &request.goal);
            let status = InitializationStatusV1 {
                operation_id: request.operation_id.clone(),
                canonical_root: request.root.clone(),
                checkpoint,
                outcome: if checkpoint == InitializationCheckpointV1::None {
                    InitializationOutcomeV1::Ready
                } else {
                    InitializationOutcomeV1::RecoveryRequired
                },
                error_kind: initialization_error_kind(error).to_owned(),
            };
            let (status, goal, guide) = self.reconcile_recovery_status(status, &request.goal);
            let reload_failed = self.install_reloaded_authority(status.clone()).is_err();
            let mut state = lock(&self.state);
            if reload_failed {
                state.initialization = Some(status);
            }
            state.initialization_goal = Some(goal);
            state.initialization_guide = guide;
        }
        initialized
    }

    fn status(&self, operation_id: &str) -> AppResult<InitializationStatusV1> {
        validate_operation_id(operation_id)?;
        self.refresh_durable_initialization()
            .map_err(|_| AppError::Message("initialization status is unavailable".to_owned()))?;
        if let Some(status) = lock(&self.state)
            .initialization
            .clone()
            .filter(|status| status.operation_id == operation_id)
        {
            return Ok(status);
        }
        Err(AppError::Message(
            "initialization operation is unknown".to_owned(),
        ))
    }

    fn pending(&self) -> AppResult<PendingInitializationV1> {
        self.refresh_durable_initialization()
            .map_err(|_| AppError::Message("initialization status is unavailable".to_owned()))?;
        let status = lock(&self.state).initialization.clone();
        let Some(status) = status.filter(|status| {
            status.outcome != InitializationOutcomeV1::Complete
                && status.checkpoint != InitializationCheckpointV1::None
        }) else {
            return Ok(PendingInitializationV1 {
                pending: false,
                initialization: None,
                validation: None,
            });
        };
        let validation = self
            .validate_target_inner(Path::new(&status.canonical_root))
            .unwrap_or_else(|_| {
                Self::recovery_validation(
                    &status.canonical_root,
                    "the interrupted initialization target is unavailable",
                )
            });
        Ok(PendingInitializationV1 {
            pending: true,
            initialization: Some(status),
            validation: Some(validation),
        })
    }

    fn completed_initialization(&self) -> AppResult<Option<InitializationStatusV1>> {
        self.refresh_durable_initialization()
            .map_err(|_| AppError::Message("initialization status is unavailable".to_owned()))?;
        Ok(lock(&self.state)
            .initialization
            .clone()
            .filter(|status| status.outcome == InitializationOutcomeV1::Complete))
    }

    fn mark_desktop_bound(&self, operation_id: &str) -> AppResult<InitializationStatusV1> {
        validate_operation_id(operation_id)?;
        let (status, goal, guide) = {
            let state = lock(&self.state);
            let status = state
                .initialization
                .as_ref()
                .filter(|status| status.operation_id == operation_id)
                .ok_or_else(|| {
                    AppError::Message("initialization operation is unknown".to_owned())
                })?;
            if status.checkpoint == InitializationCheckpointV1::DesktopBound {
                return Ok(status.clone());
            }
            if status.checkpoint != InitializationCheckpointV1::GuideApplied {
                return Err(recovery(
                    "desktop binding requires the guide decision to be applied",
                ));
            }
            let status = InitializationStatusV1 {
                checkpoint: InitializationCheckpointV1::DesktopBound,
                outcome: InitializationOutcomeV1::Complete,
                error_kind: String::new(),
                ..status.clone()
            };
            let goal = state.initialization_goal.clone().ok_or_else(|| {
                AppError::Message("initialization operation goal is unavailable".to_owned())
            })?;
            (status, goal, state.initialization_guide.clone())
        };
        publish_desktop_initialization(&self.global_home, &status, &goal, guide.as_ref())?;
        lock(&self.state).initialization = Some(status.clone());
        Ok(status)
    }
}

impl DesktopUpdateService for ProductionDesktopAuthority {
    fn start(&self) -> Result<(), String> {
        lock(&self.state).updates.clone().start()
    }

    fn state(&self) -> UpdateState {
        lock(&self.state).updates.clone().state()
    }

    fn set_automatic_checks(&self, enabled: bool) -> Result<UpdateState, String> {
        lock(&self.state)
            .updates
            .clone()
            .set_automatic_checks(enabled)
    }

    fn check_for_updates(&self) -> Result<UpdateState, String> {
        lock(&self.state).updates.clone().check_for_updates()
    }

    fn download_update(&self, expected_version: &str) -> Result<UpdateState, String> {
        lock(&self.state)
            .updates
            .clone()
            .download_update(expected_version)
    }

    fn apply_update(&self, expected_version: &str) -> Result<UpdateState, String> {
        lock(&self.state)
            .updates
            .clone()
            .apply_update(expected_version)
    }

    fn cancel_operation(&self) -> UpdateState {
        lock(&self.state).updates.clone().cancel_operation()
    }

    fn shutdown(&self) -> Result<(), String> {
        lock(&self.state).updates.clone().shutdown()
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

/// What a launch opens before the window is shown.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StartupProjectV1 {
    /// Open this root immediately.
    Open(PathBuf),
    /// Land on Welcome. A path is the recent entry to preselect, so the user
    /// confirms its relocation themselves.
    Welcome(Option<PathBuf>),
}

/// Decides what a launch opens.
///
/// An explicit context always wins. A path named on the command line is an
/// explicit instruction, and so is a working directory that is itself a bound
/// project: a terminal launch from inside a project opens that project, which
/// is both the pre-existing behavior and the only reading a terminal user
/// expects. Auto-open applies only when neither is present — the Finder and
/// Dock launch — and then demands proof: the opt-in, a recorded root, and a
/// `resolve_recent_project` answer that is both `Available` and `Ready`. A
/// `ConfirmationRequired` answer is the relocated-project case and never
/// auto-opens; it preselects the entry on Welcome instead, because confirming
/// a move is the user's call, not the launcher's.
#[must_use]
pub fn startup_project(
    cli_path: Option<PathBuf>,
    working_directory_project: Option<PathBuf>,
    restore_last_project: bool,
    last_project_root: Option<&str>,
    resolved: Option<(RecentProjectAvailabilityV1, RecentProjectResolutionV1)>,
) -> StartupProjectV1 {
    if let Some(path) = cli_path.or(working_directory_project) {
        return StartupProjectV1::Open(path);
    }
    let Some(root) = last_project_root.filter(|_| restore_last_project) else {
        return StartupProjectV1::Welcome(None);
    };
    match resolved {
        Some((RecentProjectAvailabilityV1::Available, RecentProjectResolutionV1::Ready)) => {
            StartupProjectV1::Open(PathBuf::from(root))
        }
        Some((
            RecentProjectAvailabilityV1::Available,
            RecentProjectResolutionV1::ConfirmationRequired,
        )) => StartupProjectV1::Welcome(Some(PathBuf::from(root))),
        _ => StartupProjectV1::Welcome(None),
    }
}

/// Decides what a launch opens from the working directory and the stored
/// startup preference.
///
/// Resolves the working directory through the same binding machinery the
/// runtime uses, so a launch from inside a project opens it without ever
/// reading the opt-in. Only a working directory that is no project reaches the
/// `startup` section, whose recorded root is then proven through the same
/// `resolve_recent_project` path the Welcome list uses, so an auto-open is
/// held to exactly the evidence a manual reopen is. Every failure — no
/// runtime, no store, no matching recents entry — lands on Welcome, because a
/// launcher that cannot prove a project has no business opening one.
#[must_use]
pub fn resolved_startup_project(
    global_home: &Path,
    writer_version: &str,
    cli_path: Option<PathBuf>,
    current_dir: &Path,
) -> StartupProjectV1 {
    if cli_path.is_some() {
        return startup_project(cli_path, None, false, None, None);
    }
    let Ok(Some(runtime)) = ActiveRuntime::load(global_home, writer_version) else {
        return StartupProjectV1::Welcome(None);
    };
    let working_directory_project = runtime
        .bindings_for(current_dir)
        .ok()
        .and_then(|bindings| bindings.project)
        .map(|project| project.root);
    if working_directory_project.is_some() {
        return startup_project(None, working_directory_project, false, None, None);
    }
    let Ok(bindings) = runtime.global_bindings(runtime.global_home()) else {
        return StartupProjectV1::Welcome(None);
    };
    let Ok(store) = GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding)
    else {
        return StartupProjectV1::Welcome(None);
    };
    let startup = crate::preferences::preferences(&store).preferences.startup;
    let Some(root) = startup
        .last_project_root
        .clone()
        .filter(|_| startup.restore_last_project)
    else {
        return StartupProjectV1::Welcome(None);
    };
    let recents = ProductionRecentProjects::new(runtime);
    let resolved = recents
        .recent_projects_v1()
        .ok()
        .and_then(|listed| {
            listed
                .projects
                .into_iter()
                .find(|entry| entry.canonical_path == root)
        })
        .and_then(|entry| {
            recents
                .resolve_recent_project(&entry.entry_id, &entry.base, Path::new(&root))
                .ok()
                .map(|resolved| (entry.availability, resolved.resolution))
        });
    startup_project(
        None,
        None,
        startup.restore_last_project,
        Some(&root),
        resolved,
    )
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

/// Prunes marker projects whose root directories no longer exist so one
/// deleted project cannot block every startup. Returns true only when a
/// pruned marker was published; any other outcome leaves the caller's
/// original fail-closed error in force. The exclusive lock is non-blocking,
/// so a live process holding the shared lease makes this return an error
/// rather than wait.
fn prune_missing_marker_projects(global_home: &Path, writer_version: &str) -> AppResult<bool> {
    if !global_home.exists() {
        return Ok(false);
    }
    let home = fs::canonicalize(global_home).map_err(recovery)?;
    if path_is_present(&home.join("runtime").join(BOOTSTRAP_PLAN))? {
        return Ok(false);
    }
    let lease = acquire_cutover_lock(&home, CutoverLockMode::Exclusive).map_err(recovery)?;
    let Some(marker) = load_active_generation(&home, &lease).map_err(recovery)? else {
        return Ok(false);
    };
    let kept: Vec<ActiveGenerationProject> = marker
        .projects
        .iter()
        .filter(|project| path_is_present(Path::new(&project.root)).unwrap_or(true))
        .cloned()
        .collect();
    if kept.len() == marker.projects.len() {
        return Ok(false);
    }
    backup_marker(&home)?;
    let pruned = ActiveGeneration {
        projects: kept,
        ..marker
    };
    install_active_generation(&home, &lease, &pruned, writer_version).map_err(recovery)?;
    Ok(true)
}

fn backup_marker(home: &Path) -> AppResult<()> {
    let marker = home.join("runtime").join("active-generation.json");
    let backup = home.join("runtime").join(format!(
        "active-generation.json.pruned-{}",
        OffsetDateTime::now_utc().unix_timestamp()
    ));
    fs::copy(&marker, &backup)?;
    protect_private_file(&backup).map_err(recovery)?;
    Ok(())
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
    project_root_identity: PrivatePathIdentity,
    project_directory_identity: PrivatePathIdentity,
    previous_marker: Option<ActiveGeneration>,
    operation_id: Option<String>,
) -> AppResult<BootstrapPlan> {
    if let Some(operation_id) = &operation_id {
        validate_operation_id(operation_id)?;
    }
    validate_new_bootstrap_target(home, root, previous_marker.as_ref())?;
    let generation = previous_marker
        .as_ref()
        .map(ActiveGeneration::generation_number)
        .transpose()?
        .unwrap_or(random_nonzero_u64()?);
    let global_path = home.join("global.redb");
    let global_database_id = previous_marker
        .as_ref()
        .map_or_else(random_id, |marker| Ok(marker.global.database_id.clone()))?;
    let project_directory = root.join(".ptrack");
    let project_path = project_directory.join("ptrack.redb");
    let mut projects = previous_marker
        .as_ref()
        .map(|marker| marker.projects.clone())
        .unwrap_or_default();
    let project_root = root
        .to_str()
        .ok_or_else(|| recovery("project root must be valid UTF-8"))?;
    let project_database = project_path
        .to_str()
        .ok_or_else(|| recovery("project database path must be valid UTF-8"))?;
    projects.push(ActiveGenerationProject {
        root: project_root.to_owned(),
        database_id: random_id()?,
        path: project_database.to_owned(),
    });
    projects.sort_by(|left, right| left.root.cmp(&right.root));
    let target_marker =
        ActiveGeneration::new(generation, global_database_id, &global_path, projects)?;
    Ok(BootstrapPlan {
        format: "ptrack-bootstrap-plan".to_owned(),
        version: "2".to_owned(),
        operation_id,
        previous_marker,
        target_marker,
        project_root: project_root.to_owned(),
        project_root_identity,
        project_directory_identity,
    })
}

/// Why a root can never be a project, or `None` for an ordinary candidate.
///
/// Two roots are refused by name rather than left to downstream checks, which
/// would misreport them as recovery cases or colliding database destinations:
/// a root whose `.ptrack` IS the global home ("`ptrack init` in `~`"), and the
/// OS user home itself — even when `PTRACK_HOME` points somewhere else, a home
/// directory initialized as a project would sweep every repository under it
/// into one workspace.
fn home_project_refusal(root: &Path, global_homes: &[PathBuf]) -> Option<&'static str> {
    if is_global_home(&root.join(".ptrack"), global_homes) {
        return Some("the p-track home directory cannot be a project");
    }
    let user_home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .and_then(|home| fs::canonicalize(PathBuf::from(home)).ok());
    if user_home.is_some_and(|home_dir| same_path(root, &home_dir)) {
        return Some("the user home directory cannot be a project");
    }
    None
}

fn validate_new_bootstrap_target(
    home: &Path,
    root: &Path,
    previous_marker: Option<&ActiveGeneration>,
) -> AppResult<()> {
    if let Some(refusal) = home_project_refusal(root, &global_home_exemptions(home)) {
        return Err(AppError::Message(refusal.to_owned()));
    }
    if previous_marker.is_none() && path_is_present(&home.join("global.redb"))? {
        return Err(recovery(
            "an unpublished Rust global database requires recovery",
        ));
    }
    let project_directory = root.join(".ptrack");
    if path_is_present(&project_directory.join("ptrack.redb"))? {
        return Err(recovery(
            "an unmapped Rust project database requires recovery",
        ));
    }
    Ok(())
}

fn path_is_present(path: &Path) -> AppResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// The same target guards initialization enforces, applied to a relocation
/// root: never the p-track or user home, and never a root nested inside
/// another project's storage tree (which would make deepest-ancestor binding
/// resolution reroute commands under this subtree).
fn require_relocation_target(root: &Path, home: &Path) -> AppResult<()> {
    let global_homes = global_home_exemptions(home);
    if let Some(refusal) = home_project_refusal(root, &global_homes) {
        return Err(AppError::Message(refusal.to_owned()));
    }
    for ancestor in root.ancestors().skip(1) {
        let storage = ancestor.join(".ptrack");
        if is_global_home(&storage, &global_homes) {
            continue;
        }
        if path_is_present(&storage)? {
            return Err(AppError::Message(
                "cannot relocate into a folder nested inside another project".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Splits a marker for a relocation: keeps live projects, drops the moved
/// store's stale registration and any project whose root vanished — exactly
/// as the startup self-heal would prune them; the marker install revalidates
/// every kept destination. A store still present at its registered location
/// is a copy, not a move, and refuses relocation. Returns the kept entries
/// and whether an unrelated project was dropped.
fn relocation_marker_projects(
    marker: &ActiveGeneration,
    recorded: &ActiveBinding,
) -> AppResult<(Vec<ActiveGenerationProject>, bool)> {
    let mut projects: Vec<ActiveGenerationProject> = Vec::new();
    let mut dropped_other = false;
    for project in &marker.projects {
        if project.database_id == recorded.database_id {
            if path_is_present(Path::new(&project.path))? {
                return Err(recovery(
                    "a store with this database ID still exists at its registered location; a copied store cannot be relocated",
                ));
            }
            // The stale registration of this store's previous location.
            continue;
        }
        if path_is_present(Path::new(&project.root))? {
            projects.push(project.clone());
        } else {
            dropped_other = true;
        }
    }
    Ok((projects, dropped_other))
}

fn require_new_project_storage_absent(root: &Path, global_home: &Path) -> AppResult<()> {
    let global_homes = global_home_exemptions(global_home);
    for (depth, ancestor) in root.ancestors().enumerate() {
        let storage = ancestor.join(".ptrack");
        // Depth 0 is the selected root itself, which never gets the exemption:
        // its own `.ptrack` must not be the global home.
        if depth > 0 && is_global_home(&storage, &global_homes) {
            continue;
        }
        if path_is_present(&storage)? {
            return Err(recovery(
                "selected project storage changed before initialization",
            ));
        }
    }
    Ok(())
}

/// Global homes an ancestor walk must not mistake for project storage: the home
/// this authority runs on and the one the environment resolves. Production passes
/// the resolved home, so the two coincide; tests and embedders supply their own.
///
/// The default global home is `<user home>/.ptrack`, an ancestor `.ptrack` of every
/// project under the user home, so without the exemption the common case classifies
/// as recovery-required instead of new.
fn global_home_exemptions(global_home: &Path) -> [PathBuf; 2] {
    let resolved = resolve_global_home().unwrap_or_else(|_| global_home.to_owned());
    [
        comparable_global_home(global_home),
        comparable_global_home(&resolved),
    ]
}

/// Reports whether an ancestor's `.ptrack` names one of the global homes.
fn is_global_home(storage: &Path, global_homes: &[PathBuf]) -> bool {
    global_homes.iter().any(|home| same_path(storage, home))
}

/// Rewrites a global home into the shape an ancestor walk produces, so the two can
/// be compared without following a symlink at the final component. Falls back to the
/// path as given when the home or its parent does not exist.
fn comparable_global_home(global_home: &Path) -> PathBuf {
    match (global_home.parent(), global_home.file_name()) {
        (Some(parent), Some(name)) => fs::canonicalize(parent)
            .map_or_else(|_| global_home.to_owned(), |parent| parent.join(name)),
        _ => global_home.to_owned(),
    }
}

/// Compares two paths case-insensitively where the platform file systems are.
fn same_path(left: &Path, right: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        left.as_os_str().eq_ignore_ascii_case(right.as_os_str())
    } else {
        left == right
    }
}

fn selected_project_storage_present(root: &Path) -> bool {
    path_is_present(&root.join(".ptrack/ptrack.redb")).unwrap_or(true)
}

fn selected_project_directory_present(root: &Path) -> bool {
    path_is_present(&root.join(".ptrack")).unwrap_or(true)
}

fn content_digest(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(sha256_digest(bytes))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn validate_guide_manifest(manifest: &DesktopGuideManifest) -> AppResult<()> {
    validate_operation_id(&manifest.operation_id)?;
    if manifest.version != "1"
        || manifest.canonical_root.is_empty()
        || manifest.canonical_root.len() > 4_096
        || !Path::new(&manifest.canonical_root).is_absolute()
    {
        return Err(recovery("desktop initialization guide is invalid"));
    }
    match manifest.choice {
        ProjectGuideChoiceV1::Skip => {
            if !manifest.preview_token.is_empty()
                || manifest.root_identity.is_none()
                || !manifest.template_digest.is_empty()
                || !manifest.files.is_empty()
            {
                return Err(recovery("skipped project guide manifest is invalid"));
            }
        }
        ProjectGuideChoiceV1::Install => {
            validate_operation_id(&manifest.preview_token)?;
            if manifest.root_identity.is_none()
                || !valid_digest(&manifest.template_digest)
                || manifest.files.len() != GUIDE_FILES.len()
            {
                return Err(recovery("project guide manifest is incomplete"));
            }
            for (file, expected_name) in manifest.files.iter().zip(GUIDE_FILES) {
                if file.name != expected_name
                    || !valid_digest(&file.output_digest)
                    || file.mode > 0o7777
                    || match file.action {
                        ProjectGuideFileActionV1::Create => {
                            file.base_identity.is_some() || !file.base_digest.is_empty()
                        }
                        ProjectGuideFileActionV1::Update | ProjectGuideFileActionV1::NoChange => {
                            file.base_identity.is_none() || !valid_digest(&file.base_digest)
                        }
                    }
                {
                    return Err(recovery("project guide file manifest is invalid"));
                }
            }
        }
    }
    Ok(())
}

fn read_guide_template(home: &Path) -> AppResult<String> {
    let path = home.join("guide.md");
    if !path_is_present(&path)? {
        return Ok(String::new());
    }
    let file = open_private_path(&path, false, false).map_err(recovery)?;
    let length = file.metadata()?.len();
    if length > GUIDE_FILE_LIMIT {
        return Err(AppError::Message(
            "project guide template exceeds its byte limit".to_owned(),
        ));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(length).unwrap_or_default());
    file.take(GUIDE_FILE_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > GUIDE_FILE_LIMIT {
        return Err(AppError::Message(
            "project guide template exceeds its byte limit".to_owned(),
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| AppError::Message("project guide template is not valid UTF-8".to_owned()))
}

fn validate_guide_before_commit(home: &Path, manifest: &DesktopGuideManifest) -> AppResult<()> {
    validate_guide_manifest(manifest)?;
    if manifest.choice == ProjectGuideChoiceV1::Skip {
        let current = PinnedProjectDirectory::identify_root(Path::new(&manifest.canonical_root))
            .map_err(recovery)?;
        return if Some(current) == manifest.root_identity {
            Ok(())
        } else {
            Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))
        };
    }
    #[cfg(not(unix))]
    {
        let _ = home;
        return Err(AppError::Message("project-guide-unavailable".to_owned()));
    }
    #[cfg(unix)]
    {
        let root_identity = manifest
            .root_identity
            .ok_or_else(|| recovery("project guide root identity is missing"))?;
        let root = PinnedGuideRoot::capture(Path::new(&manifest.canonical_root), root_identity)?;
        let template = read_guide_template(home)?;
        if content_digest(template.as_bytes()) != manifest.template_digest {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        for file in &manifest.files {
            validate_guide_file_state(&root, file, &template)?;
        }
        root.verify()
    }
}

fn apply_guide_manifest(
    home: &Path,
    manifest: &DesktopGuideManifest,
    pinned: &PinnedProjectDirectory,
) -> AppResult<()> {
    if manifest.choice == ProjectGuideChoiceV1::Skip {
        pinned.verify().map_err(recovery)?;
        return if Some(pinned.root_identity()) == manifest.root_identity {
            Ok(())
        } else {
            Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))
        };
    }
    #[cfg(not(unix))]
    {
        let _ = (home, pinned);
        return Err(AppError::Message("project-guide-unavailable".to_owned()));
    }
    #[cfg(unix)]
    {
        let root_identity = manifest
            .root_identity
            .ok_or_else(|| recovery("project guide root identity is missing"))?;
        if pinned.root_identity() != root_identity {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        let guide_root = PinnedGuideRoot::from_pinned(pinned, root_identity)?;
        let template = read_guide_template(home)?;
        if content_digest(template.as_bytes()) != manifest.template_digest {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        for file in &manifest.files {
            if let Err(error) = validate_guide_file_state(&guide_root, file, &template) {
                return if guide_root_has_applied_output(&guide_root, &manifest.files)? {
                    Err(AppError::Message(GUIDE_PARTIALLY_APPLIED.to_owned()))
                } else {
                    Err(error)
                };
            }
        }
        for file in &manifest.files {
            let applied = (|| -> AppResult<()> {
                let current = guide_root.read(&file.name)?;
                if current
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.digest == file.output_digest)
                {
                    return Ok(());
                }
                require_guide_base(current.as_ref(), file)?;
                let base = current
                    .as_ref()
                    .map_or("", |snapshot| snapshot.content.as_str());
                let (output, _) = upsert_guide(base, &template);
                if output.len() > GUIDE_OUTPUT_LIMIT
                    || content_digest(output.as_bytes()) != file.output_digest
                {
                    return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
                }
                guide_root.publish(file, &output)
            })();
            if let Err(error) = applied {
                return if guide_root_has_applied_output(&guide_root, &manifest.files)? {
                    Err(AppError::Message(GUIDE_PARTIALLY_APPLIED.to_owned()))
                } else {
                    Err(error)
                };
            }
        }
        guide_root.verify()
    }
}

#[cfg(unix)]
fn guide_root_has_applied_output(
    root: &PinnedGuideRoot<'_>,
    files: &[DesktopGuideFileManifest],
) -> AppResult<bool> {
    for file in files {
        if file.action != ProjectGuideFileActionV1::NoChange
            && root
                .read(&file.name)?
                .is_some_and(|snapshot| snapshot.digest == file.output_digest)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn guide_manifest_has_applied_output(manifest: &DesktopGuideManifest) -> AppResult<bool> {
    if manifest.choice != ProjectGuideChoiceV1::Install {
        return Ok(false);
    }
    #[cfg(not(unix))]
    {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        let root_identity = manifest
            .root_identity
            .ok_or_else(|| recovery("project guide root identity is missing"))?;
        let root = PinnedGuideRoot::capture(Path::new(&manifest.canonical_root), root_identity)?;
        for file in &manifest.files {
            if file.action != ProjectGuideFileActionV1::NoChange
                && root
                    .read(&file.name)?
                    .is_some_and(|snapshot| snapshot.digest == file.output_digest)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(unix)]
pub(crate) fn install_project_guide_pinned(
    pinned: &PinnedProjectDirectory,
    extra: &str,
) -> AppResult<Vec<&'static str>> {
    let root = PinnedGuideRoot::from_pinned(pinned, pinned.root_identity())?;
    let mut written = Vec::new();
    for name in GUIDE_FILES {
        let base = root.read(name)?;
        let base_content = base
            .as_ref()
            .map_or("", |snapshot| snapshot.content.as_str());
        let (output, changed) = upsert_guide(base_content, extra);
        if !changed {
            continue;
        }
        if output.len() > GUIDE_OUTPUT_LIMIT {
            return Err(AppError::Message(
                "project guide proposed content exceeds its byte limit".to_owned(),
            ));
        }
        let manifest = DesktopGuideFileManifest {
            name: name.to_owned(),
            action: if base.is_some() {
                ProjectGuideFileActionV1::Update
            } else {
                ProjectGuideFileActionV1::Create
            },
            base_identity: base.as_ref().map(|snapshot| snapshot.identity),
            base_digest: base
                .as_ref()
                .map_or_else(String::new, |snapshot| snapshot.digest.clone()),
            output_digest: content_digest(output.as_bytes()),
            mode: base.as_ref().map_or(0o644, |snapshot| snapshot.mode),
        };
        root.publish(&manifest, &output)?;
        written.push(name);
    }
    root.verify()?;
    pinned.verify().map_err(recovery)?;
    Ok(written)
}

#[cfg(not(unix))]
pub(crate) fn install_project_guide_pinned(
    _pinned: &PinnedProjectDirectory,
    _extra: &str,
) -> AppResult<Vec<&'static str>> {
    Err(AppError::Message("project-guide-unavailable".to_owned()))
}

#[cfg(unix)]
fn validate_guide_file_state(
    root: &PinnedGuideRoot<'_>,
    file: &DesktopGuideFileManifest,
    template: &str,
) -> AppResult<()> {
    let current = root.read(&file.name)?;
    if current
        .as_ref()
        .is_some_and(|snapshot| snapshot.digest == file.output_digest)
    {
        return Ok(());
    }
    require_guide_base(current.as_ref(), file)?;
    let base = current
        .as_ref()
        .map_or("", |snapshot| snapshot.content.as_str());
    let (output, _) = upsert_guide(base, template);
    if output.len() > GUIDE_OUTPUT_LIMIT || content_digest(output.as_bytes()) != file.output_digest
    {
        return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
    }
    Ok(())
}

fn require_guide_base(
    current: Option<&GuideFileSnapshot>,
    file: &DesktopGuideFileManifest,
) -> AppResult<()> {
    let matches = match (current, file.base_identity) {
        (None, None) => file.base_digest.is_empty(),
        (Some(current), Some(identity)) => {
            current.identity == identity
                && current.digest == file.base_digest
                && current.mode == file.mode
        }
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))
    }
}

fn guide_line_counts(base: &str, output: &str, action: ProjectGuideFileActionV1) -> (usize, usize) {
    match action {
        ProjectGuideFileActionV1::Create => (output.lines().count(), 0),
        ProjectGuideFileActionV1::Update => (output.lines().count(), base.lines().count()),
        ProjectGuideFileActionV1::NoChange => (0, 0),
    }
}

fn guide_diff(
    name: &str,
    base: &str,
    output: &str,
    action: ProjectGuideFileActionV1,
) -> AppResult<String> {
    if action == ProjectGuideFileActionV1::NoChange {
        return Ok(String::new());
    }
    let old_name = if action == ProjectGuideFileActionV1::Create {
        "/dev/null"
    } else {
        name
    };
    let mut diff = format!(
        "--- {old_name}\n+++ {name}\n@@ -1,{} +1,{} @@\n",
        base.lines().count(),
        output.lines().count()
    );
    for line in base.lines() {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in output.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    if diff.len() > GUIDE_DIFF_LIMIT {
        return Err(AppError::Message(
            "project guide preview diff exceeds its byte limit".to_owned(),
        ));
    }
    Ok(diff)
}

#[cfg(unix)]
struct PinnedGuideRoot<'a> {
    path: Option<PathBuf>,
    pinned: Option<&'a PinnedProjectDirectory>,
    identity: PrivatePathIdentity,
    handle: fs::File,
    staging: Option<fs::File>,
}

#[cfg(unix)]
impl<'a> PinnedGuideRoot<'a> {
    fn capture(path: &Path, expected: PrivatePathIdentity) -> AppResult<Self> {
        use std::os::unix::fs::MetadataExt as _;

        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || fs::canonicalize(path)? != path
        {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        let handle = fs::File::open(path)?;
        let identity = PrivatePathIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        let opened = handle.metadata()?;
        if identity != expected || opened.dev() != identity.device || opened.ino() != identity.inode
        {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        Ok(Self {
            path: Some(path.to_owned()),
            pinned: None,
            identity,
            handle,
            staging: None,
        })
    }

    fn from_pinned(
        pinned: &'a PinnedProjectDirectory,
        expected: PrivatePathIdentity,
    ) -> AppResult<Self> {
        use std::os::unix::fs::MetadataExt as _;

        pinned.verify().map_err(recovery)?;
        if pinned.root_identity() != expected {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        let handle = pinned.try_clone_root_directory().map_err(recovery)?;
        let staging = pinned.try_clone_project_directory().map_err(recovery)?;
        let metadata = handle.metadata()?;
        if metadata.dev() != expected.device || metadata.ino() != expected.inode {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        Ok(Self {
            path: None,
            pinned: Some(pinned),
            identity: expected,
            handle,
            staging: Some(staging),
        })
    }

    fn verify(&self) -> AppResult<()> {
        use std::os::unix::fs::MetadataExt as _;

        let opened = self.handle.metadata()?;
        if opened.dev() != self.identity.device || opened.ino() != self.identity.inode {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        if let Some(pinned) = self.pinned {
            pinned.verify().map_err(recovery)?;
        } else if let Some(path) = &self.path {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || metadata.dev() != self.identity.device
                || metadata.ino() != self.identity.inode
            {
                return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
            }
        }
        Ok(())
    }

    fn read(&self, name: &str) -> AppResult<Option<GuideFileSnapshot>> {
        use rustix::fs::{AtFlags, Mode, OFlags, openat, statat};
        use std::os::unix::fs::MetadataExt as _;

        let before = match statat(&self.handle, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
            Err(_) => return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned())),
        };
        let descriptor = openat(
            &self.handle,
            name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|_| AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))?;
        let file = fs::File::from(descriptor);
        let metadata = file.metadata()?;
        let identity = PrivatePathIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        if !metadata.is_file() || !guide_stat_matches(identity, &before) {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        let mode = metadata.mode() & 0o7777;
        let mut bytes = Vec::with_capacity(
            usize::try_from(metadata.len().min(GUIDE_FILE_LIMIT)).unwrap_or_default(),
        );
        file.take(GUIDE_FILE_LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > GUIDE_FILE_LIMIT {
            return Err(AppError::Message(
                "project guide file exceeds its byte limit".to_owned(),
            ));
        }
        let after = statat(&self.handle, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))?;
        if !guide_stat_matches(identity, &after) {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        let content = String::from_utf8(bytes)
            .map_err(|_| AppError::Message("project guide file is not valid UTF-8".to_owned()))?;
        Ok(Some(GuideFileSnapshot {
            identity,
            digest: content_digest(content.as_bytes()),
            content,
            mode,
        }))
    }

    fn publish(&self, manifest: &DesktopGuideFileManifest, content: &str) -> AppResult<()> {
        use rustix::fs::{
            AtFlags, Mode, OFlags, RenameFlags, openat, renameat, renameat_with, unlinkat,
        };
        use std::os::unix::fs::PermissionsExt as _;

        let staging = self
            .staging
            .as_ref()
            .ok_or_else(|| recovery("project guide staging authority is unavailable"))?;
        let temporary = format!(".guide-{}-{}.tmp", manifest.name, random_id()?);
        let descriptor = openat(
            staging,
            temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(platform_raw_mode(manifest.mode)),
        )
        .map_err(|error| AppError::Io(error.into()))?;
        let mut file = fs::File::from(descriptor);
        let prepared = (|| -> AppResult<()> {
            file.write_all(content.as_bytes())?;
            file.set_permissions(fs::Permissions::from_mode(manifest.mode))?;
            file.sync_all()?;
            Ok(())
        })();
        drop(file);
        if let Err(error) = prepared {
            let _ = unlinkat(staging, temporary.as_str(), AtFlags::empty());
            return Err(error);
        }
        #[cfg(test)]
        run_guide_before_publish_hook();
        let mut staged = true;
        let publication = (|| -> AppResult<()> {
            let current = self.read(&manifest.name)?;
            require_guide_base(current.as_ref(), manifest)?;
            self.verify()?;
            let published = if manifest.base_identity.is_none() {
                renameat_with(
                    staging,
                    temporary.as_str(),
                    &self.handle,
                    manifest.name.as_str(),
                    RenameFlags::NOREPLACE,
                )
            } else {
                renameat(
                    staging,
                    temporary.as_str(),
                    &self.handle,
                    manifest.name.as_str(),
                )
            };
            if published.is_err() {
                return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
            }
            staged = false;
            Ok(())
        })();
        if let Err(error) = publication {
            if staged {
                let _ = unlinkat(staging, temporary.as_str(), AtFlags::empty());
            }
            return Err(error);
        }
        if staged {
            let _ = unlinkat(staging, temporary.as_str(), AtFlags::empty());
            return Err(AppError::Message(
                "project guide publication failed".to_owned(),
            ));
        }
        self.handle.sync_all()?;
        staging.sync_all()?;
        let applied = self.read(&manifest.name)?;
        if applied
            .as_ref()
            .is_none_or(|snapshot| snapshot.digest != manifest.output_digest)
        {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        Ok(())
    }
}

#[cfg(test)]
std::thread_local! {
    static GUIDE_BEFORE_PUBLISH_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static GUIDE_BEFORE_COMMIT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static INITIALIZATION_BEFORE_COMMIT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static INITIALIZATION_AFTER_STARTED_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static STARTUP_INITIALIZATION_INFERENCE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static INITIALIZATION_AFTER_BOOTSTRAP_PLAN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(crate) fn set_guide_before_publish_hook(hook: impl FnOnce() + 'static) {
    GUIDE_BEFORE_PUBLISH_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_guide_before_publish_hook() {
    GUIDE_BEFORE_PUBLISH_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
pub(crate) fn set_guide_before_commit_hook(hook: impl FnOnce() + 'static) {
    GUIDE_BEFORE_COMMIT_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_guide_before_commit_hook() {
    GUIDE_BEFORE_COMMIT_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
pub(crate) fn set_initialization_before_commit_hook(hook: impl FnOnce() + 'static) {
    INITIALIZATION_BEFORE_COMMIT_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_initialization_before_commit_hook() {
    INITIALIZATION_BEFORE_COMMIT_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
pub(crate) fn set_initialization_after_started_hook(hook: impl FnOnce() + 'static) {
    INITIALIZATION_AFTER_STARTED_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_initialization_after_started_hook() {
    INITIALIZATION_AFTER_STARTED_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
pub(crate) fn set_startup_initialization_inference_hook(hook: impl FnOnce() + 'static) {
    STARTUP_INITIALIZATION_INFERENCE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_startup_initialization_inference_hook() {
    STARTUP_INITIALIZATION_INFERENCE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
pub(crate) fn set_initialization_after_bootstrap_plan_hook(hook: impl FnOnce() + 'static) {
    INITIALIZATION_AFTER_BOOTSTRAP_PLAN_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_initialization_after_bootstrap_plan_hook() {
    INITIALIZATION_AFTER_BOOTSTRAP_PLAN_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(unix)]
fn guide_stat_matches(identity: PrivatePathIdentity, stat: &rustix::fs::Stat) -> bool {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let device_matches = stat.st_dev == identity.device;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let device_matches = u64::try_from(stat.st_dev).is_ok_and(|device| device == identity.device);
    device_matches && stat.st_ino == identity.inode
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const fn platform_raw_mode(mode: u32) -> u32 {
    mode
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn platform_raw_mode(mode: u32) -> u16 {
    u16::try_from(mode).expect("mode bits fit the platform raw mode")
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

#[cfg(windows)]
type DirectoryIdentity = PrivatePathIdentity;

#[cfg(unix)]
fn directory_identity(path: &Path) -> AppResult<DirectoryIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() || fs::canonicalize(path)? != path {
        return Err(recovery(
            "selected project root changed before initialization",
        ));
    }
    Ok(DirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(windows)]
fn directory_identity(path: &Path) -> AppResult<DirectoryIdentity> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() || fs::canonicalize(path)? != path {
        return Err(recovery(
            "selected project root changed before initialization",
        ));
    }
    PinnedProjectDirectory::identify_root(path).map_err(recovery)
}

#[cfg(not(any(unix, windows)))]
compile_error!("desktop project initialization requires directory identity support");

fn require_directory_identity(path: &Path, expected: DirectoryIdentity) -> AppResult<()> {
    if directory_identity(path)? == expected {
        Ok(())
    } else {
        Err(recovery(
            "selected project root identity changed before initialization",
        ))
    }
}

fn validate_bootstrap_plan(
    home: &Path,
    root: &Path,
    plan: &BootstrapPlan,
    writer_version: &str,
) -> AppResult<()> {
    validate_bootstrap_plan_intent(home, root, plan, writer_version)?;
    if PinnedProjectDirectory::identify_directory(root).map_err(recovery)?
        != plan.project_directory_identity
    {
        return Err(recovery("bootstrap project directory identity changed"));
    }
    Ok(())
}

fn validate_bootstrap_plan_intent(
    home: &Path,
    root: &Path,
    plan: &BootstrapPlan,
    writer_version: &str,
) -> AppResult<()> {
    if let Some(operation_id) = &plan.operation_id {
        validate_operation_id(operation_id)?;
    }
    if plan.format != "ptrack-bootstrap-plan"
        || plan.version != "2"
        || Path::new(&plan.project_root) != root
        || PinnedProjectDirectory::identify_root(root).map_err(recovery)?
            != plan.project_root_identity
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
    pinned_project: Option<&PinnedProjectDirectory>,
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
    if let Some(pinned) = pinned_project {
        if pinned.database_path() != Path::new(&project.path) {
            return Err(recovery("bootstrap project path changed"));
        }
        pinned.verify().map_err(recovery)?;
    } else {
        ensure_private_directory(
            Path::new(&project.path)
                .parent()
                .ok_or_else(|| recovery("bootstrap project path has no parent"))?,
        )?;
    }
    let binding = binding_for_new(
        generation,
        project.database_id.clone(),
        StoreKind::Project,
        Path::new(&project.path),
    )?;
    if Path::new(&project.path).exists() {
        if let Some(pinned) = pinned_project {
            pinned.verify().map_err(recovery)?;
        }
        let store = if let Some(pinned) = pinned_project {
            ProjectStore::open_existing_pinned(pinned, &binding, writer_version)
        } else {
            ProjectStore::open_existing(&project.path, &binding, writer_version)
        }
        .map_err(recovery)?;
        if store.application_writes().map_err(recovery)? {
            return Err(recovery("unpublished project store has application writes"));
        }
        drop(store);
        if let Some(pinned) = pinned_project {
            pinned.verify().map_err(recovery)?;
        }
    } else if let Some(pinned) = pinned_project {
        drop(ProjectStore::create_new_pinned(pinned, binding, writer_version).map_err(recovery)?);
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

fn read_desktop_initialization(home: &Path) -> AppResult<Option<DesktopInitializationJournal>> {
    let path = home.join("runtime").join(DESKTOP_INITIALIZATION);
    if !path_is_present(&path)? {
        return Ok(None);
    }
    let file = open_private_path(&path, false, false).map_err(recovery)?;
    let length = file.metadata()?.len();
    if length == 0 || length > DESKTOP_INITIALIZATION_LIMIT {
        return Err(recovery("desktop initialization status size is invalid"));
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(length)
            .map_err(|_| recovery("desktop initialization status is too large"))?,
    );
    file.take(DESKTOP_INITIALIZATION_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    let journal: DesktopInitializationJournal = serde_json::from_slice(&bytes)
        .map_err(|_| recovery("desktop initialization status is invalid"))?;
    if journal.format != "ptrack-desktop-initialization"
        || journal.version != "1"
        || canonical_desktop_initialization_bytes(&journal)? != bytes
    {
        return Err(recovery("desktop initialization status is not canonical"));
    }
    validate_operation_id(&journal.status.operation_id)?;
    let status_shape_is_valid = match journal.status.checkpoint {
        InitializationCheckpointV1::None => matches!(
            journal.status.outcome,
            InitializationOutcomeV1::Ready | InitializationOutcomeV1::InProgress
        ),
        InitializationCheckpointV1::Prepared
        | InitializationCheckpointV1::RuntimeCommitted
        | InitializationCheckpointV1::ProjectCommitted
        | InitializationCheckpointV1::GuideApplied => matches!(
            journal.status.outcome,
            InitializationOutcomeV1::InProgress | InitializationOutcomeV1::RecoveryRequired
        ),
        InitializationCheckpointV1::DesktopBound => {
            journal.status.outcome == InitializationOutcomeV1::Complete
        }
    };
    if journal.status.canonical_root.is_empty()
        || journal.status.canonical_root.len() > 4_096
        || !Path::new(&journal.status.canonical_root).is_absolute()
        || journal.status.error_kind.len() > 64
        || journal.goal.is_empty()
        || journal.goal.trim() != journal.goal
        || journal.goal.len() > 4_096
        || !status_shape_is_valid
    {
        return Err(recovery("desktop initialization status fields are invalid"));
    }
    if let Some(guide) = &journal.guide {
        validate_guide_manifest(guide)?;
        if guide.operation_id != journal.status.operation_id
            || guide.canonical_root != journal.status.canonical_root
        {
            return Err(recovery(
                "desktop initialization guide does not match its operation",
            ));
        }
    }
    Ok(Some(journal))
}

fn publish_desktop_initialization(
    home: &Path,
    status: &InitializationStatusV1,
    goal: &str,
    guide: Option<&DesktopGuideManifest>,
) -> AppResult<()> {
    validate_operation_id(&status.operation_id)?;
    if goal.is_empty() || goal.trim() != goal || goal.len() > 4_096 {
        return Err(AppError::Message(
            "initialization goal is invalid".to_owned(),
        ));
    }
    ensure_private_home(home)?;
    let runtime = home.join("runtime");
    ensure_private_directory(&runtime)?;
    with_desktop_initialization_lock(home, || {
        if let Some(existing) = read_desktop_initialization(home)? {
            validate_desktop_initialization_transition(&existing.status, status)?;
            validate_guide_transition(existing.guide.as_ref(), guide, &existing.status, status)?;
        }
        publish_desktop_initialization_locked(home, status, goal, guide)
    })
}

fn reconcile_startup_initialization(
    home: &Path,
    writer_version: &str,
    loaded: &mut DesktopInitializationJournal,
) -> AppResult<()> {
    if loaded.status.checkpoint != InitializationCheckpointV1::None
        || loaded.status.outcome == InitializationOutcomeV1::Complete
    {
        return Ok(());
    }
    let _lease = acquire_cutover_lock(home, CutoverLockMode::Shared).map_err(recovery)?;
    with_desktop_initialization_lock(home, || {
        let Some(mut current) = read_desktop_initialization(home)? else {
            return Err(recovery("desktop initialization status disappeared"));
        };
        if current.status.checkpoint != InitializationCheckpointV1::None
            || current.status.outcome == InitializationOutcomeV1::Complete
        {
            *loaded = current;
            return Ok(());
        }
        let project_storage_present =
            selected_project_directory_present(Path::new(&current.status.canonical_root));
        let bootstrap_present = path_is_present(&home.join("runtime").join(BOOTSTRAP_PLAN))?;
        if bootstrap_present {
            let canonical_home = fs::canonicalize(home)?;
            let plan = read_bootstrap_plan(&home.join("runtime").join(BOOTSTRAP_PLAN))?;
            if plan.operation_id.as_deref() != Some(current.status.operation_id.as_str())
                || plan.project_root != current.status.canonical_root
            {
                return Err(recovery(
                    "bootstrap plan is not bound to its initialization operation",
                ));
            }
            current.status.checkpoint = InitializationCheckpointV1::Prepared;
            current.status.outcome = InitializationOutcomeV1::RecoveryRequired;
            match fs::canonicalize(&current.status.canonical_root) {
                Ok(canonical_root) => {
                    validate_bootstrap_plan_intent(
                        &canonical_home,
                        &canonical_root,
                        &plan,
                        writer_version,
                    )?;
                    "interrupted-bootstrap-plan".clone_into(&mut current.status.error_kind);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    "project-not-found".clone_into(&mut current.status.error_kind);
                }
                Err(_) => "filesystem-error".clone_into(&mut current.status.error_kind),
            }
        } else if project_storage_present {
            current.status.checkpoint = InitializationCheckpointV1::Prepared;
            current.status.outcome = InitializationOutcomeV1::RecoveryRequired;
            "interrupted-project-storage".clone_into(&mut current.status.error_kind);
        } else if current.status.outcome == InitializationOutcomeV1::InProgress
            && !bootstrap_present
        {
            current.status.outcome = InitializationOutcomeV1::Ready;
            "interrupted-before-commit".clone_into(&mut current.status.error_kind);
        } else {
            *loaded = current;
            return Ok(());
        }
        publish_desktop_initialization_locked(
            home,
            &current.status,
            &current.goal,
            current.guide.as_ref(),
        )?;
        *loaded = current;
        Ok(())
    })
}

fn publish_desktop_initialization_locked(
    home: &Path,
    status: &InitializationStatusV1,
    goal: &str,
    guide: Option<&DesktopGuideManifest>,
) -> AppResult<()> {
    let runtime = home.join("runtime");
    let path = runtime.join(DESKTOP_INITIALIZATION);
    if path_is_present(&path)? {
        drop(open_private_path(&path, false, true).map_err(recovery)?);
    }
    let journal = DesktopInitializationJournal {
        format: "ptrack-desktop-initialization".to_owned(),
        version: "1".to_owned(),
        status: status.clone(),
        goal: goal.to_owned(),
        guide: guide.cloned(),
    };
    let bytes = canonical_desktop_initialization_bytes(&journal)?;
    let temporary = runtime.join(format!(".{DESKTOP_INITIALIZATION}.{}.tmp", random_id()?));
    let result = (|| -> AppResult<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        protect_private_file(&temporary).map_err(recovery)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        replace_private_file(&temporary, &path).map_err(recovery)?;
        sync_private_directory(&runtime).map_err(recovery)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn with_desktop_initialization_lock<R>(
    home: &Path,
    operation: impl FnOnce() -> AppResult<R>,
) -> AppResult<R> {
    let path = home.join("runtime").join(DESKTOP_INITIALIZATION_LOCK);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    protect_private_file(&path).map_err(recovery)?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock)
                if started.elapsed() < DESKTOP_INITIALIZATION_LOCK_TIMEOUT =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(TryLockError::WouldBlock) => {
                return Err(recovery("desktop initialization status is busy"));
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    let result = operation();
    let unlock = file.unlock().map_err(AppError::Io);
    match (result, unlock) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(AppError::Message(format!("{first}\n{second}"))),
    }
}

pub(crate) fn validate_desktop_initialization_transition(
    existing: &InitializationStatusV1,
    incoming: &InitializationStatusV1,
) -> AppResult<()> {
    if existing.operation_id == incoming.operation_id {
        if existing.outcome == InitializationOutcomeV1::Complete
            && incoming.outcome != InitializationOutcomeV1::Complete
        {
            return Err(recovery(
                "desktop initialization status cannot regress from complete",
            ));
        }
        if initialization_checkpoint_rank(incoming.checkpoint)
            < initialization_checkpoint_rank(existing.checkpoint)
        {
            return Err(recovery("desktop initialization checkpoint cannot regress"));
        }
    } else if existing.outcome != InitializationOutcomeV1::Complete
        && (existing.checkpoint != InitializationCheckpointV1::None
            || incoming.checkpoint != InitializationCheckpointV1::None)
    {
        return Err(recovery(
            "a different initialization operation cannot replace durable progress",
        ));
    }
    Ok(())
}

const fn initialization_checkpoint_rank(checkpoint: InitializationCheckpointV1) -> u8 {
    match checkpoint {
        InitializationCheckpointV1::None => 0,
        InitializationCheckpointV1::Prepared => 1,
        InitializationCheckpointV1::RuntimeCommitted => 2,
        InitializationCheckpointV1::ProjectCommitted => 3,
        InitializationCheckpointV1::GuideApplied => 4,
        InitializationCheckpointV1::DesktopBound => 5,
    }
}

fn validate_guide_transition(
    existing: Option<&DesktopGuideManifest>,
    incoming: Option<&DesktopGuideManifest>,
    existing_status: &InitializationStatusV1,
    incoming_status: &InitializationStatusV1,
) -> AppResult<()> {
    if existing.is_some_and(|existing| {
        incoming.is_some_and(|incoming| existing.root_identity != incoming.root_identity)
    }) {
        return Err(recovery(
            "desktop initialization project root identity cannot change",
        ));
    }
    match (existing, incoming) {
        (None, _) => Ok(()),
        (Some(_), Some(_)) if existing == incoming => Ok(()),
        (Some(_), None) => Err(recovery(
            "desktop initialization guide consent cannot be removed",
        )),
        (Some(existing), Some(_)) if existing.choice == ProjectGuideChoiceV1::Skip => {
            Err(recovery("skipped project guidance cannot be upgraded"))
        }
        (Some(existing), Some(incoming))
            if existing.choice == ProjectGuideChoiceV1::Install
                && incoming.choice == ProjectGuideChoiceV1::Skip
                && stale_guide_skip_allowed(existing_status)
                && incoming_status.checkpoint == existing_status.checkpoint =>
        {
            Ok(())
        }
        (Some(existing), Some(incoming))
            if existing.choice == ProjectGuideChoiceV1::Install
                && incoming.choice == ProjectGuideChoiceV1::Install
                && !matches!(
                    existing_status.checkpoint,
                    InitializationCheckpointV1::GuideApplied
                        | InitializationCheckpointV1::DesktopBound
                )
                && incoming_status.checkpoint == existing_status.checkpoint =>
        {
            Ok(())
        }
        (Some(existing), Some(incoming))
            if existing.choice == ProjectGuideChoiceV1::Install
                && incoming.choice == ProjectGuideChoiceV1::Install
                && existing_status.checkpoint == InitializationCheckpointV1::None
                && existing_status.outcome == InitializationOutcomeV1::Ready
                && existing_status.error_kind == GUIDE_PREVIEW_STALE
                && incoming_status.checkpoint == InitializationCheckpointV1::None =>
        {
            Ok(())
        }
        (Some(_), Some(_)) => Err(recovery(
            "desktop initialization guide consent is immutable",
        )),
    }
}

pub(crate) fn stale_guide_skip_allowed(status: &InitializationStatusV1) -> bool {
    (status.error_kind == GUIDE_PREVIEW_STALE
        || (status.error_kind == "interrupted-before-commit"
            && status.checkpoint == InitializationCheckpointV1::None
            && status.outcome == InitializationOutcomeV1::Ready))
        && match status.checkpoint {
            InitializationCheckpointV1::None => status.outcome == InitializationOutcomeV1::Ready,
            InitializationCheckpointV1::Prepared
            | InitializationCheckpointV1::RuntimeCommitted
            | InitializationCheckpointV1::ProjectCommitted => {
                status.outcome == InitializationOutcomeV1::RecoveryRequired
            }
            InitializationCheckpointV1::GuideApplied | InitializationCheckpointV1::DesktopBound => {
                false
            }
        }
}

fn canonical_desktop_initialization_bytes(
    journal: &DesktopInitializationJournal,
) -> AppResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec(journal)
        .map_err(|error| AppError::Message(format!("{RECOVERY_REQUIRED}: {error}")))?;
    bytes.push(b'\n');
    if bytes.len() as u64 > DESKTOP_INITIALIZATION_LIMIT {
        return Err(recovery(
            "desktop initialization status exceeds the fixed limit",
        ));
    }
    Ok(bytes)
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

fn random_operation_id() -> AppResult<String> {
    let mut raw = [0_u8; 32];
    getrandom::fill(&mut raw).map_err(|_| {
        AppError::Message("initialization identity could not be created".to_owned())
    })?;
    Ok(URL_SAFE_NO_PAD.encode(raw))
}

fn validate_operation_id(value: &str) -> AppResult<()> {
    if value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err(AppError::Message(
            "initialization operation is invalid".to_owned(),
        ))
    }
}

fn initialization_error_kind(error: &AppError) -> &'static str {
    match error {
        AppError::NoProject => "project-not-found",
        AppError::NotImplemented(_) => "unsupported",
        AppError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => "project-not-found",
        AppError::Io(_) => "filesystem-error",
        AppError::Message(message) if message == GUIDE_PREVIEW_STALE => GUIDE_PREVIEW_STALE,
        AppError::Message(message) if message == GUIDE_PARTIALLY_APPLIED => GUIDE_PARTIALLY_APPLIED,
        AppError::Message(message) if message.starts_with(RECOVERY_REQUIRED) => "recovery-required",
        AppError::Message(message)
            if message.contains("cutover")
                || message.contains("lock")
                || message.contains("busy") =>
        {
            "runtime-busy"
        }
        AppError::Message(_) => "initialization-failed",
    }
}

fn recent_entry_id(project: &ProjectRef) -> String {
    URL_SAFE_NO_PAD.encode(sha256_digest(project.path.as_bytes()))
}

fn recent_open_key(
    entry_id: &str,
    base: &str,
    canonical_root: &str,
    relocation_confirmation_token: &str,
) -> String {
    let mut bytes = Vec::new();
    push_recent_field(&mut bytes, entry_id.as_bytes());
    push_recent_field(&mut bytes, base.as_bytes());
    push_recent_field(&mut bytes, canonical_root.as_bytes());
    push_recent_field(&mut bytes, relocation_confirmation_token.as_bytes());
    URL_SAFE_NO_PAD.encode(sha256_digest(&bytes))
}

fn recent_entry_base(runtime: &ActiveRuntime, project: &ProjectRef) -> String {
    let mut bytes = Vec::new();
    push_recent_field(&mut bytes, runtime.marker.generation.as_bytes());
    push_recent_field(&mut bytes, project.name.as_bytes());
    push_recent_field(&mut bytes, project.path.as_bytes());
    if let Some(mapped) = runtime
        .marker
        .projects
        .iter()
        .find(|mapped| mapped.root == project.path)
    {
        push_recent_field(&mut bytes, mapped.database_id.as_bytes());
        push_recent_field(&mut bytes, mapped.path.as_bytes());
    } else {
        push_recent_field(&mut bytes, b"unmapped");
    }
    match project.last_seen {
        ptrack_core::Timestamp::Zero => bytes.push(0),
        ptrack_core::Timestamp::Fixed {
            seconds,
            nanoseconds,
            offset_seconds,
        } => {
            bytes.push(1);
            bytes.extend_from_slice(&seconds.to_le_bytes());
            bytes.extend_from_slice(&nanoseconds.to_le_bytes());
            bytes.extend_from_slice(&offset_seconds.to_le_bytes());
        }
    }
    URL_SAFE_NO_PAD.encode(sha256_digest(&bytes))
}

fn push_recent_field(bytes: &mut Vec<u8>, field: &[u8]) {
    bytes.extend_from_slice(&u64::try_from(field.len()).unwrap_or(u64::MAX).to_le_bytes());
    bytes.extend_from_slice(field);
}

fn validate_recent_id(value: &str) -> AppResult<()> {
    if value.len() == RECENT_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err(AppError::Message(
            "recent-project identity is invalid".to_owned(),
        ))
    }
}

fn recent_entry_stale() -> AppError {
    AppError::Message("recent-project-entry-stale".to_owned())
}

fn recent_projects_unavailable() -> AppError {
    AppError::Message("recent-projects-unavailable".to_owned())
}

fn recent_confirmation_invalid() -> AppError {
    AppError::Message("recent-project-confirmation-invalid".to_owned())
}

fn recent_project_changed() -> AppError {
    AppError::Message("recent-project-changed".to_owned())
}

fn recent_project_missing() -> AppError {
    AppError::Message("recent-project-folder-not-found".to_owned())
}

fn recent_project_permission() -> AppError {
    AppError::Message("recent-project-permission-required".to_owned())
}

fn sanitize_recent_io(error: &std::io::Error) -> AppError {
    match error.kind() {
        ErrorKind::NotFound => recent_project_missing(),
        ErrorKind::PermissionDenied => recent_project_permission(),
        _ => recent_project_changed(),
    }
}

fn sanitize_recent_store_error(error: StoreError) -> AppError {
    match error {
        StoreError::Io(error) if error.kind() == ErrorKind::PermissionDenied => {
            recent_project_permission()
        }
        _ => recent_project_changed(),
    }
}

fn sanitize_recent_app_error(error: AppError) -> AppError {
    match error {
        AppError::Io(error) => sanitize_recent_io(&error),
        AppError::NoProject | AppError::NotImplemented(_) | AppError::Message(_) => {
            recent_project_changed()
        }
    }
}

fn recent_availability(
    runtime: &ActiveRuntime,
    project: &ProjectRef,
) -> RecentProjectAvailabilityV1 {
    let canonical = match fs::canonicalize(&project.path) {
        Ok(canonical) => canonical,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return RecentProjectAvailabilityV1::Missing;
        }
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            return RecentProjectAvailabilityV1::PermissionRequired;
        }
        Err(_) => return RecentProjectAvailabilityV1::Changed,
    };
    let Some(mapped) = runtime
        .marker
        .projects
        .iter()
        .find(|mapped| mapped.root == project.path)
    else {
        return RecentProjectAvailabilityV1::Changed;
    };
    if canonical != Path::new(&mapped.root) || !canonical.is_dir() {
        return RecentProjectAvailabilityV1::Changed;
    }
    let Ok(binding) = runtime.marker.project_binding(mapped) else {
        return RecentProjectAvailabilityV1::Changed;
    };
    match ProjectStore::open_existing(&mapped.path, &binding, &runtime.writer_version) {
        Ok(_) => RecentProjectAvailabilityV1::Available,
        Err(StoreError::Io(error)) if error.kind() == ErrorKind::PermissionDenied => {
            RecentProjectAvailabilityV1::PermissionRequired
        }
        Err(_) => RecentProjectAvailabilityV1::Changed,
    }
}

fn project_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .to_owned()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
