//! The CLI's lazily bound application: project-local routing, bootstrap of a
//! new project into the live generation, and relocation of a moved store.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ptrack_core::{ProjectRef, ProjectSnapshot};
use ptrack_store::{
    ActiveBinding, ActiveGeneration, ActiveGenerationProject, ActorIdentity, CutoverLease,
    CutoverLockMode, GlobalStore, PinnedProjectDirectory, ProjectStore, acquire_bootstrap_lock,
    acquire_cutover_lock, append_active_generation, install_active_generation,
    load_active_generation, validate_active_generation,
};

use super::bootstrap::{
    BootstrapPlan, clear_bootstrap_plan, ensure_bootstrap_stores, ensure_private_home,
    global_home_exemptions, home_project_refusal, is_global_home, new_bootstrap_plan,
    publish_bootstrap_plan, read_bootstrap_plan, validate_bootstrap_plan,
    validate_new_bootstrap_target,
};
use super::runtime::{ActiveRuntime, backup_marker};
use super::{
    BOOTSTRAP_PLAN, INITIALIZATION_IN_PROGRESS, path_is_present, project_name, recovery,
    uninitialized,
};
use crate::{
    AppError, AppResult, ApplicationPort, GuideAction, HookAction, HookResult, InitRequest,
    InitResult, LocalApplication, Mutation, MutationResult, PlanLifecycleOutcome,
    PlanLifecycleRequest, ProcessOutput, RelocateRequest, RelocateResult, WorkspaceBindings,
};

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
        if let Some(metadata) = crate::local_mode::discover(&self.current_dir)? {
            return crate::local_mode::validate(&metadata, &self.current_dir, &self.writer_version);
        }
        self.active_runtime()?
            .ok_or_else(uninitialized)?
            .bindings_for(&self.current_dir)
    }

    fn local(&mut self) -> AppResult<LocalApplication> {
        if let Some(metadata) = crate::local_mode::discover(&self.current_dir)? {
            let bindings =
                crate::local_mode::validate(&metadata, &self.current_dir, &self.writer_version)?;
            return Ok(LocalApplication::project_only(bindings, metadata));
        }
        Ok(LocalApplication::new(self.bindings()?))
    }

    fn local_global(&mut self) -> AppResult<LocalApplication> {
        self.require_global_mode()?;
        let active = self.active_runtime()?.ok_or_else(uninitialized)?;
        Ok(LocalApplication::new(
            active.global_bindings(&self.current_dir)?,
        ))
    }

    /// Refuse desktop/global entry points while project-local routing is active.
    ///
    /// # Errors
    /// Returns an error for local mode or malformed project metadata.
    pub fn require_global_mode(&self) -> AppResult<()> {
        if crate::local_mode::discover(&self.current_dir)?.is_some() {
            return Err(crate::local_mode::global_refusal());
        }
        Ok(())
    }

    /// Registers a project without disturbing any live p-track process.
    ///
    /// Adding a project appends to the live generation — same generation
    /// number, same global database, same bindings for every project already
    /// listed — so the publication runs under the shared cutover lease that
    /// running apps and sessions also hold, serialized against other
    /// initializers by the bootstrap lease. Only the first generation, which
    /// has no marker to append to, still publishes under the exclusive lease.
    fn bootstrap(&mut self, request: &InitRequest) -> AppResult<bool> {
        ensure_private_home(&self.global_home)?;
        let home = fs::canonicalize(&self.global_home)?;
        let requested = request.root.as_deref().unwrap_or(&self.current_dir);
        let root = fs::canonicalize(requested)?;
        let plan_path = home.join("runtime").join(BOOTSTRAP_PLAN);
        // A root the marker already lists publishes nothing, so it must never
        // wait on a lease that a running app or session holds for its life.
        if !path_is_present(&plan_path)? && self.adopt_listed_project(&home, &root)? {
            return Ok(false);
        }
        // Startup self-heal runs first, while the bootstrap lock this
        // initialization is about to take is still free: a marker listing some
        // other project whose folder was deleted would otherwise fail
        // validation below, and the prune that fixes it could not run from
        // inside this publication. A marker that stays unhealthy still fails
        // closed, with its own message, a few lines down.
        let _ = self.active_runtime();
        let publication = match acquire_bootstrap_lock(&home) {
            Ok(lease) => lease,
            Err(error) if error.to_string().contains("lock is unavailable") => {
                return Err(AppError::Message(INITIALIZATION_IN_PROGRESS.to_owned()));
            }
            Err(error) => return Err(recovery(error)),
        };
        let lease = acquire_cutover_lock(&home, CutoverLockMode::Shared).map_err(recovery)?;
        let Some(previous) = load_active_generation(&home, &lease).map_err(recovery)? else {
            drop(lease);
            return self.bootstrap_first_generation(&home, &root, &plan_path);
        };
        validate_active_generation(&home, &previous, &self.writer_version).map_err(recovery)?;
        let Some((plan, pinned_project)) =
            self.plan_bootstrap(&home, &root, &plan_path, Some(&previous))?
        else {
            drop(lease);
            self.active = ActiveRuntime::load(&home, &self.writer_version)?;
            return Ok(false);
        };
        if previous == plan.target_marker {
            clear_bootstrap_plan(&plan_path)?;
            drop(lease);
            self.active = ActiveRuntime::load(&home, &self.writer_version)?;
            return Ok(false);
        }
        if plan.previous_marker.as_ref() != Some(&previous) {
            return Err(recovery(
                "bootstrap plan does not match the active-generation marker",
            ));
        }
        ensure_bootstrap_stores(&home, &plan, &self.writer_version, Some(&pinned_project))?;
        append_active_generation(
            &home,
            &lease,
            &publication,
            &previous,
            &plan.target_marker,
            &self.writer_version,
        )
        .map_err(recovery)?;
        clear_bootstrap_plan(&plan_path)?;
        drop(lease);
        self.active = ActiveRuntime::load(&home, &self.writer_version)?;
        Ok(true)
    }

    /// Loads the runtime when the marker already lists `root`, reporting
    /// whether initialization has nothing left to publish.
    fn adopt_listed_project(&mut self, home: &Path, root: &Path) -> AppResult<bool> {
        let lease = acquire_cutover_lock(home, CutoverLockMode::Shared).map_err(recovery)?;
        let listed = load_active_generation(home, &lease)
            .map_err(recovery)?
            .is_some_and(|marker| {
                marker
                    .projects
                    .iter()
                    .any(|project| Path::new(&project.root) == root)
            });
        drop(lease);
        if !listed {
            return Ok(false);
        }
        self.active = ActiveRuntime::load(home, &self.writer_version)?;
        Ok(true)
    }

    /// Publishes the very first generation, which creates the global store and
    /// has no live bindings to preserve, under the exclusive cutover lease.
    fn bootstrap_first_generation(
        &mut self,
        home: &Path,
        root: &Path,
        plan_path: &Path,
    ) -> AppResult<bool> {
        let lease = match acquire_cutover_lock(home, CutoverLockMode::Exclusive) {
            Ok(lease) => lease,
            Err(error) if error.to_string().contains("lock is unavailable") => {
                return Err(AppError::Message(
                    "another p-track process holds the runtime lease; close p-track apps/sessions and retry"
                        .to_owned(),
                ));
            }
            Err(error) => return Err(recovery(error)),
        };
        let existing = load_active_generation(home, &lease).map_err(recovery)?;
        if let Some(marker) = &existing {
            validate_active_generation(home, marker, &self.writer_version).map_err(recovery)?;
        }
        let Some((plan, pinned_project)) =
            self.plan_bootstrap(home, root, plan_path, existing.as_ref())?
        else {
            drop(lease);
            self.active = ActiveRuntime::load(home, &self.writer_version)?;
            return Ok(false);
        };
        if existing.as_ref() == Some(&plan.target_marker) {
            clear_bootstrap_plan(plan_path)?;
            drop(lease);
            self.active = ActiveRuntime::load(home, &self.writer_version)?;
            return Ok(false);
        }
        if existing != plan.previous_marker {
            return Err(recovery(
                "bootstrap plan does not match the active-generation marker",
            ));
        }
        ensure_bootstrap_stores(home, &plan, &self.writer_version, Some(&pinned_project))?;
        install_active_generation(home, &lease, &plan.target_marker, &self.writer_version)
            .map_err(recovery)?;
        clear_bootstrap_plan(plan_path)?;
        drop(lease);
        self.active = ActiveRuntime::load(home, &self.writer_version)?;
        Ok(true)
    }

    /// Resolves the plan this initialization must publish: a resumed durable
    /// plan, a freshly published one, or `None` when the marker already lists
    /// the root and nothing has to change.
    fn plan_bootstrap(
        &self,
        home: &Path,
        root: &Path,
        plan_path: &Path,
        existing: Option<&ActiveGeneration>,
    ) -> AppResult<Option<(BootstrapPlan, PinnedProjectDirectory)>> {
        if plan_path.exists() {
            let plan = read_bootstrap_plan(plan_path)?;
            validate_bootstrap_plan(home, root, &plan, &self.writer_version)?;
            let pinned = PinnedProjectDirectory::prepare_expected_identities(
                root,
                plan.project_root_identity,
                plan.project_directory_identity,
            )
            .map_err(recovery)?;
            return Ok(Some((plan, pinned)));
        }
        if existing.is_some_and(|marker| {
            marker
                .projects
                .iter()
                .any(|project| Path::new(&project.root) == root)
        }) {
            return Ok(None);
        }
        let project_root_identity =
            PinnedProjectDirectory::identify_root(root).map_err(recovery)?;
        validate_new_bootstrap_target(home, root, existing)?;
        let pinned = PinnedProjectDirectory::prepare_new_expected(root, project_root_identity)
            .map_err(recovery)?;
        let plan = new_bootstrap_plan(
            home,
            root,
            project_root_identity,
            pinned.directory_identity(),
            existing.cloned(),
            None,
        )?;
        publish_bootstrap_plan(plan_path, &plan)?;
        Ok(Some((plan, pinned)))
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
        let lease = relocation_lease(&home)?;
        if path_is_present(&home.join("runtime").join(BOOTSTRAP_PLAN))? {
            return Err(recovery(
                "bootstrap recovery must complete before relocation",
            ));
        }
        let marker = load_active_generation(&home, &lease)
            .map_err(recovery)?
            .ok_or_else(uninitialized)?;
        let requested = request.root.as_deref().unwrap_or(&self.current_dir);
        let target = RelocationTarget::resolve(requested, &marker, &home)?;
        let recorded = relocation_binding(&target.database, &marker)?;
        let (mut projects, dropped_other) = relocation_marker_projects(&marker, &recorded)?;
        if dropped_other {
            // Match the startup self-heal: never publish a marker that drops
            // an unrelated project without a recoverable backup.
            backup_marker(&home)?;
        }
        // Where the store lived before this run's rebind — the authoritative
        // old root for the recents cleanup, surviving marker pruning.
        let previous_root = (recorded.canonical_path != target.database)
            .then(|| recorded.canonical_path.parent().and_then(Path::parent))
            .flatten()
            .map(Path::to_path_buf);
        if recorded.canonical_path != target.database {
            ProjectStore::rebind_moved(&target.database, &recorded).map_err(recovery)?;
        }
        projects.push(ActiveGenerationProject {
            root: target.root_text,
            database_id: recorded.database_id,
            path: target.database_text,
        });
        projects.sort_by(|left, right| left.root.cmp(&right.root));
        let generation = ActiveGeneration {
            projects,
            ..marker.clone()
        };
        install_active_generation(&home, &lease, &generation, &self.writer_version)
            .map_err(recovery)?;
        drop(lease);
        self.active = ActiveRuntime::load(&home, &self.writer_version)?;
        if let Some(previous_root) = previous_root {
            self.move_recent_entry(&previous_root, &target.root);
        }
        Ok(RelocateResult { root: target.root })
    }

    /// Best-effort recents cleanup after a relocation: move the registry row
    /// from the old root to the new one. A stale row is cosmetic, never fatal.
    fn move_recent_entry(&self, previous_root: &Path, root: &Path) {
        if let Some(runtime) = &self.active
            && let Ok(bindings) = runtime.global_bindings(root)
            && let Ok(global) =
                GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding)
            && let Ok(Some(expected)) = global.project(previous_root)
        {
            let _ = global.relocate_project_if_matches(&expected, project_name(root), root);
        }
    }
}

/// Takes the exclusive cutover lease a relocation publishes under.
fn relocation_lease(home: &Path) -> AppResult<CutoverLease> {
    match acquire_cutover_lock(home, CutoverLockMode::Exclusive) {
        Ok(lease) => Ok(lease),
        Err(error) if error.to_string().contains("cutover lock is unavailable") => {
            Err(AppError::Message(
                "another p-track process is running; quit it and run 'ptrack relocate' again"
                    .to_owned(),
            ))
        }
        Err(error) => Err(recovery(error)),
    }
}

/// The canonical relocation destination and the store found there.
struct RelocationTarget {
    root: PathBuf,
    root_text: String,
    database: PathBuf,
    database_text: String,
}

impl RelocationTarget {
    fn resolve(requested: &Path, marker: &ActiveGeneration, home: &Path) -> AppResult<Self> {
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
        require_relocation_target(&root, home)?;
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
        Ok(Self {
            root,
            root_text,
            database,
            database_text,
        })
    }
}

/// The binding a moved store recorded, which must belong to the current
/// generation under a database ID the marker does not already use.
fn relocation_binding(database: &Path, marker: &ActiveGeneration) -> AppResult<ActiveBinding> {
    let recorded = ProjectStore::peek_binding(database)
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
    Ok(recorded)
}

impl ApplicationPort for RoutedApplication {
    fn local_mode(&mut self, action: &str) -> AppResult<String> {
        if action == "status" {
            return match crate::local_mode::discover(&self.current_dir)? {
                Some(metadata) => {
                    crate::local_mode::validate(
                        &metadata,
                        &self.current_dir,
                        &self.writer_version,
                    )?;
                    Ok("project-local mode is enabled".to_owned())
                }
                None => Ok("project-local mode is disabled".to_owned()),
            };
        }
        if action == "disable" {
            self.active_runtime()?.ok_or_else(uninitialized)?;
            crate::local_mode::disable(&self.current_dir)?;
            return Ok("project-local mode disabled; project database retained".to_owned());
        }
        if !matches!(action, "enable" | "sync") {
            return Err(AppError::Message("unknown local mode action".to_owned()));
        }
        // Explicit enable/sync/disable are the only escape from project-only routing.
        // They must be invoked by the user outside the agent sandbox.
        let previous = if action == "sync" {
            crate::local_mode::discover(&self.current_dir)?
                .map(|metadata| {
                    crate::local_mode::validate(&metadata, &self.current_dir, &self.writer_version)
                })
                .transpose()?
        } else {
            None
        };
        let active = self.active_runtime()?.ok_or_else(uninitialized)?;
        let bindings = active.bindings_for(&self.current_dir)?;
        let endpoint = bindings
            .project
            .as_ref()
            .ok_or(AppError::NoProject)?
            .clone();
        if previous
            .as_ref()
            .is_some_and(|old| old.project.as_ref() != Some(&endpoint))
        {
            return Err(AppError::Message(
                "local authority is stale; run 'ptrack local enable' outside the sandbox"
                    .to_owned(),
            ));
        }
        // Explicit sync must report registration failures; ordinary project
        // commands deliberately treat this global bookkeeping as best effort.
        {
            let global =
                GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding)?;
            global.register_project(project_name(&endpoint.root), &endpoint.root)?;
        }
        let mut application = LocalApplication::new(bindings);
        let snapshot = application.snapshot()?;
        let metadata = crate::local_mode::LocalMetadata::capture(
            &endpoint,
            application.identity()?,
            application.guide_extra()?,
        )?;
        crate::overview::write_project_summary(
            active.global_home(),
            &endpoint.root,
            &endpoint.binding.database_id,
            &snapshot,
        )?;
        if action == "enable" || previous.is_some() {
            metadata.write()?;
        }
        Ok(if action == "sync" {
            "project summary and settings synchronized; project database retained".to_owned()
        } else {
            "project-local mode enabled; run 'ptrack sync' outside the sandbox to refresh shared settings and the overview".to_owned()
        })
    }

    fn relocate(&mut self, request: RelocateRequest) -> AppResult<RelocateResult> {
        self.require_global_mode()?;
        self.relocate_project(&request)
    }

    fn initialize(&mut self, request: InitRequest) -> AppResult<InitResult> {
        self.require_global_mode()?;
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

    fn scratchpad(&mut self) -> AppResult<ptrack_core::Scratchpad> {
        self.local()?.scratchpad()
    }

    fn set_scratchpad(
        &mut self,
        expected_revision: u64,
        value: ptrack_core::Scratchpad,
    ) -> AppResult<ptrack_core::Scratchpad> {
        self.local()?.set_scratchpad(expected_revision, value)
    }

    fn agent_runs(&mut self) -> AppResult<ptrack_agent::AgentRunsV2> {
        self.local()?.agent_runs()
    }

    fn agent_run(&mut self, run_id: &str) -> AppResult<ptrack_agent::AgentRunObservationV1> {
        self.local()?.agent_run(run_id)
    }

    fn agent_inbox(&mut self) -> AppResult<ptrack_agent::AgentHandoffInbox> {
        self.local()?.agent_inbox()
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
        if crate::local_mode::discover(&self.current_dir)?.is_some() {
            return self.local()?.identity();
        }
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
