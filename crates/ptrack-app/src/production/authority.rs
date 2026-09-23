//! The replaceable desktop authority shared by workspaces, recents, updates,
//! and first-run initialization.
//!
//! Initialization is the authority's state machine and lives in child
//! modules: `initialization` serves the service surface, `target` classifies
//! a selected folder, `guide` previews and binds project guidance, and
//! `commit` runs the exclusive-lease commit transaction.

mod commit;
mod guide;
mod initialization;
mod target;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::factory::ProductionDesktopWorkspaceFactory;
use super::guide::DesktopGuideManifest;
use super::journal::{read_desktop_initialization, reconcile_startup_initialization};
use super::recent::ProductionRecentProjects;
use super::runtime::ActiveRuntime;
use super::startup::StartupProjectV1;
use super::{BOOTSTRAP_PLAN, lock, path_is_present, uninitialized};
use crate::{
    AppError, AppResult, DesktopEventSink, DesktopRuntime, DesktopRuntimeConfig,
    DesktopUpdateEventSink, DesktopUpdateService, DesktopWorkspace, DesktopWorkspaceFactory,
    ForgetRecentProjectResultV1, InitializationOutcomeV1, InitializationStatusV1,
    RecentProjectOpenAuthorizationV1, RecentProjectRegistryCommitV1, RecentProjectsProvider,
    RecentProjectsV1, ResolvedRecentProjectV1, UnavailableUpdateService, UpdateEventSink,
    UpdateRuntime, UpdateState,
};

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
            super::test_support::run_startup_initialization_inference_hook();
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
        drop(state);
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
    production_desktop_runtime_for_startup(
        global_home,
        writer_version,
        &StartupProjectV1::Open(current.to_path_buf()),
        events,
        initial_plan,
    )
}

/// Creates the desktop without interpreting a Welcome decision as a project path.
///
/// # Errors
/// Returns an error when the global runtime or selected workspace cannot load.
pub fn production_desktop_runtime_for_startup(
    global_home: PathBuf,
    writer_version: impl Into<String>,
    startup: &StartupProjectV1,
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
    if let StartupProjectV1::Open(current) = startup
        && let Some(runtime) = authority.initial_workspace_runtime()
    {
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
        match factory.build(root, generation) {
            // A project the command line registered after launch is missing
            // only from this process's copy of the marker.
            Err(AppError::NoProject) if self.reload_marker().is_some() => {
                factory.build(root, generation)
            }
            built => built,
        }
    }
}

impl RecentProjectsProvider for ProductionDesktopAuthority {
    fn global_overview_v1(&self) -> AppResult<crate::overview::GlobalOverviewV1> {
        let recents = lock(&self.state).recents.clone();
        recents.map_or_else(
            || Ok(crate::overview::GlobalOverviewV1::default()),
            |recents| recents.global_overview_v1(),
        )
    }

    fn refresh_global_overview_v1(&self) -> AppResult<crate::overview::RefreshGlobalOverviewV1> {
        let recents = lock(&self.state).recents.clone();
        recents.map_or_else(
            || Ok(crate::overview::RefreshGlobalOverviewV1::default()),
            |recents| recents.refresh_global_overview_v1(),
        )
    }

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
        let recents = self.recent_provider()?;
        recents.resolve_recent_project(entry_id, base, candidate)
    }

    fn authorize_recent_project_open(
        &self,
        entry_id: &str,
        base: &str,
        canonical_root: &Path,
        relocation_confirmation_token: &str,
    ) -> AppResult<RecentProjectOpenAuthorizationV1> {
        let recents = self.recent_provider()?;
        recents.authorize_recent_project_open(
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
        let recents = self.recent_provider()?;
        recents.finish_recent_project_open(authorization)
    }

    fn forget_recent_project(
        &self,
        entry_id: &str,
        base: &str,
    ) -> AppResult<ForgetRecentProjectResultV1> {
        let recents = self.recent_provider()?;
        recents.forget_recent_project(entry_id, base)
    }
}

impl ProductionDesktopAuthority {
    /// Clones the current update service out of the authority state.
    ///
    /// Every update delegate binds this in its own statement so the state
    /// mutex is released before a (possibly network-bound) update call runs;
    /// holding it across the call blocked `CancelUpdateOperation`,
    /// `GetUpdateState`, and `OpenProject` for the whole download.
    fn update_service(&self) -> Arc<dyn DesktopUpdateService> {
        Arc::clone(&lock(&self.state).updates)
    }

    /// Swaps in a scripted update service so a test can hold one operation
    /// open and prove the other delegates still answer.
    #[cfg(test)]
    pub(crate) fn replace_update_service_for_test(&self, updates: Arc<dyn DesktopUpdateService>) {
        lock(&self.state).updates = updates;
    }

    /// Reloads the generation marker when another process published a newer
    /// one, and rebinds the workspace factory and recent projects to it.
    ///
    /// The desktop authority loads the marker once, so a project the command
    /// line registers while the app runs is unknown to it until this runs.
    /// Returns the fresh runtime only when the marker changed. The update
    /// service is left alone: it is bound to the global store, not to the
    /// project list, and may be mid-operation.
    fn reload_marker(&self) -> Option<Arc<ActiveRuntime>> {
        let current = lock(&self.state).runtime.clone()?;
        let fresh = ActiveRuntime::load(&self.global_home, &self.writer_version)
            .ok()
            .flatten()?;
        if fresh.marker() == current.marker() {
            return None;
        }
        let mut state = lock(&self.state);
        if !state
            .runtime
            .as_ref()
            .is_some_and(|runtime| Arc::ptr_eq(runtime, &current))
        {
            // Another caller rebound the authority first; its runtime wins.
            return state.runtime.clone();
        }
        if let Some(factory) = &state.factory {
            factory.replace_runtime(Arc::clone(&fresh));
        }
        if state.recents.is_some() {
            state.recents = Some(ProductionRecentProjects::new(Arc::clone(&fresh)));
        }
        state.runtime = Some(Arc::clone(&fresh));
        drop(state);
        Some(fresh)
    }

    /// Clones the recent-project provider without holding the state mutex
    /// across the provider call.
    fn recent_provider(&self) -> AppResult<Arc<ProductionRecentProjects>> {
        let recents = lock(&self.state).recents.clone();
        recents.ok_or_else(uninitialized)
    }
}

impl DesktopUpdateService for ProductionDesktopAuthority {
    fn start(&self) -> Result<(), String> {
        let updates = self.update_service();
        updates.start()
    }

    fn state(&self) -> UpdateState {
        let updates = self.update_service();
        updates.state()
    }

    fn set_automatic_checks(&self, enabled: bool) -> Result<UpdateState, String> {
        let updates = self.update_service();
        updates.set_automatic_checks(enabled)
    }

    fn check_for_updates(&self) -> Result<UpdateState, String> {
        let updates = self.update_service();
        updates.check_for_updates()
    }

    fn download_update(&self, expected_version: &str) -> Result<UpdateState, String> {
        let updates = self.update_service();
        updates.download_update(expected_version)
    }

    fn apply_update(&self, expected_version: &str) -> Result<UpdateState, String> {
        let updates = self.update_service();
        updates.apply_update(expected_version)
    }

    fn cancel_operation(&self) -> UpdateState {
        let updates = self.update_service();
        updates.cancel_operation()
    }

    fn shutdown(&self) -> Result<(), String> {
        let updates = self.update_service();
        updates.shutdown()
    }
}
