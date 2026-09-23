//! What a desktop launch opens: an explicit path, the restored last project,
//! or Welcome.

use std::path::{Path, PathBuf};

use ptrack_store::GlobalStore;

use super::recent::ProductionRecentProjects;
use super::runtime::ActiveRuntime;
use crate::{RecentProjectAvailabilityV1, RecentProjectResolutionV1, RecentProjectsProvider};

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
/// A named command-line path wins. Otherwise the stored restore preference
/// decides, even when the process inherits a tracked working directory. A
/// relocated project requires confirmation on Welcome instead of auto-opening.
#[must_use]
pub fn startup_project(
    cli_path: Option<PathBuf>,
    _working_directory_project: Option<PathBuf>,
    restore_last_project: bool,
    last_project_root: Option<&str>,
    resolved: Option<(RecentProjectAvailabilityV1, RecentProjectResolutionV1)>,
) -> StartupProjectV1 {
    if let Some(path) = cli_path {
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
/// Only an explicit path bypasses the preference. The recorded root must be
/// available and ready through the same resolution used by the project list.
/// Missing or invalid state leaves the desktop on Welcome.
#[must_use]
pub fn resolved_startup_project(
    global_home: &Path,
    writer_version: &str,
    cli_path: Option<PathBuf>,
    _current_dir: &Path,
) -> StartupProjectV1 {
    if cli_path.is_some() {
        return startup_project(cli_path, None, false, None, None);
    }
    let Ok(Some(runtime)) = ActiveRuntime::load(global_home, writer_version) else {
        return StartupProjectV1::Welcome(None);
    };
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
