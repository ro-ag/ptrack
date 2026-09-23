//! The durable bootstrap plan: building, validating, and publishing one exact
//! project addition, the stores it creates, and the target-path guards shared
//! by initialization and relocation.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use ptrack_store::{
    ActiveBinding, ActiveGeneration, ActiveGenerationProject, GlobalStore, PinnedProjectDirectory,
    PrivatePathIdentity, ProjectStore, StoreKind, open_private_path, protect_private_directory,
    protect_private_file, sync_private_directory, validate_active_generation,
};
use serde::{Deserialize, Serialize};

use super::runtime::resolve_global_home;
use super::{
    BOOTSTRAP_LIMIT, RECOVERY_REQUIRED, path_is_present, random_id, recovery, validate_operation_id,
};
use crate::{AppError, AppResult};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BootstrapPlan {
    pub(super) format: String,
    pub(super) version: String,
    pub(super) operation_id: Option<String>,
    pub(super) previous_marker: Option<ActiveGeneration>,
    pub(super) target_marker: ActiveGeneration,
    pub(super) project_root: String,
    pub(super) project_root_identity: PrivatePathIdentity,
    pub(super) project_directory_identity: PrivatePathIdentity,
}

pub(super) fn ensure_private_home(path: &Path) -> AppResult<()> {
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

pub(super) fn ensure_private_directory(path: &Path) -> AppResult<()> {
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

pub(super) fn new_bootstrap_plan(
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
pub(super) fn home_project_refusal(root: &Path, global_homes: &[PathBuf]) -> Option<&'static str> {
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

pub(super) fn validate_new_bootstrap_target(
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

pub(super) fn require_new_project_storage_absent(root: &Path, global_home: &Path) -> AppResult<()> {
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
pub(super) fn global_home_exemptions(global_home: &Path) -> [PathBuf; 2] {
    let resolved = resolve_global_home().unwrap_or_else(|_| global_home.to_owned());
    [
        comparable_global_home(global_home),
        comparable_global_home(&resolved),
    ]
}

/// Reports whether an ancestor's `.ptrack` names one of the global homes.
pub(super) fn is_global_home(storage: &Path, global_homes: &[PathBuf]) -> bool {
    global_homes.iter().any(|home| same_path(storage, home))
}

/// Rewrites a global home into the shape an ancestor walk produces, so the two can
/// be compared without following a symlink at the final component. Falls back to the
/// path as given when the home or its parent does not exist.
pub(super) fn comparable_global_home(global_home: &Path) -> PathBuf {
    match (global_home.parent(), global_home.file_name()) {
        (Some(parent), Some(name)) => fs::canonicalize(parent)
            .map_or_else(|_| global_home.to_owned(), |parent| parent.join(name)),
        _ => global_home.to_owned(),
    }
}

/// Compares two paths case-insensitively where the platform file systems are.
pub(super) fn same_path(left: &Path, right: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        left.as_os_str().eq_ignore_ascii_case(right.as_os_str())
    } else {
        left == right
    }
}

pub(super) fn selected_project_storage_present(root: &Path) -> bool {
    path_is_present(&root.join(".ptrack/ptrack.redb")).unwrap_or(true)
}

pub(super) fn selected_project_directory_present(root: &Path) -> bool {
    path_is_present(&root.join(".ptrack")).unwrap_or(true)
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DirectoryIdentity {
    pub(super) device: u64,
    pub(super) inode: u64,
}

#[cfg(windows)]
pub(super) type DirectoryIdentity = PrivatePathIdentity;

#[cfg(unix)]
pub(super) fn directory_identity(path: &Path) -> AppResult<DirectoryIdentity> {
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
pub(super) fn directory_identity(path: &Path) -> AppResult<DirectoryIdentity> {
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

pub(super) fn require_directory_identity(
    path: &Path,
    expected: DirectoryIdentity,
) -> AppResult<()> {
    if directory_identity(path)? == expected {
        Ok(())
    } else {
        Err(recovery(
            "selected project root identity changed before initialization",
        ))
    }
}

pub(super) fn validate_bootstrap_plan(
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

pub(super) fn validate_bootstrap_plan_intent(
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

pub(super) fn ensure_bootstrap_stores(
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

pub(super) fn read_bootstrap_plan(path: &Path) -> AppResult<BootstrapPlan> {
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

pub(super) fn publish_bootstrap_plan(path: &Path, plan: &BootstrapPlan) -> AppResult<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    protect_private_file(path).map_err(recovery)?;
    file.write_all(&canonical_bootstrap_bytes(plan)?)?;
    file.sync_all()?;
    drop(file);
    sync_private_directory(path.parent().expect("bootstrap plan has parent")).map_err(recovery)
}

pub(super) fn clear_bootstrap_plan(path: &Path) -> AppResult<()> {
    open_private_path(path, false, true).map_err(recovery)?;
    fs::remove_file(path)?;
    sync_private_directory(path.parent().expect("bootstrap plan has parent")).map_err(recovery)
}

pub(super) fn canonical_bootstrap_bytes(plan: &BootstrapPlan) -> AppResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec(plan)
        .map_err(|error| AppError::Message(format!("{RECOVERY_REQUIRED}: {error}")))?;
    bytes.push(b'\n');
    if bytes.len() as u64 > BOOTSTRAP_LIMIT {
        return Err(recovery("bootstrap plan exceeds the fixed limit"));
    }
    Ok(bytes)
}

pub(super) fn binding_for_new(
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

pub(super) fn random_nonzero_u64() -> AppResult<u64> {
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
