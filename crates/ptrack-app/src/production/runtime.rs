//! The process-owned active generation and the startup self-heal that prunes
//! vanished projects from its marker.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ptrack_store::{
    ActiveGeneration, ActiveGenerationProject, CutoverLease, CutoverLockMode,
    acquire_bootstrap_lock, acquire_cutover_lock, load_active_generation, protect_private_file,
    retire_active_generation, validate_active_generation_for_load,
};
use time::OffsetDateTime;

use super::{BOOTSTRAP_PLAN, path_is_present, recovery};
use crate::{AppError, AppResult, ProjectEndpoint, WorkspaceBindings};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeBindingState {
    Uninitialized,
    Active,
}

/// Process-owned active generation. The retained shared lease prevents an
/// offline activation or rollback while any caller can still write stores.
pub struct ActiveRuntime {
    pub(super) home: PathBuf,
    pub(super) marker: ActiveGeneration,
    pub(super) writer_version: String,
    pub(super) _lease: CutoverLease,
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
        validate_active_generation_for_load(&home, &marker, writer_version).map_err(recovery)?;
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

/// Prunes marker projects whose root directories no longer exist so one
/// deleted project cannot block every startup. Returns true only when a
/// pruned marker was published; any other outcome leaves the caller's
/// original fail-closed error in force. Retiring a vanished root rebinds
/// nothing that a live process routed to, so this publishes under the shared
/// cutover lease that running apps and sessions hold — one deleted folder no
/// longer locks the whole runtime out until every one of them is closed.
pub(super) fn prune_missing_marker_projects(
    global_home: &Path,
    writer_version: &str,
) -> AppResult<bool> {
    if !global_home.exists() {
        return Ok(false);
    }
    let home = fs::canonicalize(global_home).map_err(recovery)?;
    if path_is_present(&home.join("runtime").join(BOOTSTRAP_PLAN))? {
        return Ok(false);
    }
    let publication = acquire_bootstrap_lock(&home).map_err(recovery)?;
    let lease = acquire_cutover_lock(&home, CutoverLockMode::Shared).map_err(recovery)?;
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
        ..marker.clone()
    };
    retire_active_generation(
        &home,
        &lease,
        &publication,
        &marker,
        &pruned,
        writer_version,
    )
    .map_err(recovery)?;
    Ok(true)
}

pub(super) fn backup_marker(home: &Path) -> AppResult<()> {
    let marker = home.join("runtime").join("active-generation.json");
    let backup = home.join("runtime").join(format!(
        "active-generation.json.pruned-{}",
        OffsetDateTime::now_utc().unix_timestamp()
    ));
    fs::copy(&marker, &backup)?;
    protect_private_file(&backup).map_err(recovery)?;
    Ok(())
}
