use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    ActiveBinding, CutoverLease, CutoverLockMode, GlobalStore, ProjectStore, StoreError, StoreKind,
    StoreResult, replace_private_file, sync_private_directory,
};

pub const ACTIVE_GENERATION_MARKER: &str = "active-generation.json";
const MARKER_FORMAT: &str = "ptrack-active-generation";
const MARKER_VERSION: &str = "1";
const MARKER_LIMIT: u64 = 1024 * 1024;
const PROJECT_LIMIT: usize = 10_000;

/// A live active-store guard retained while an activation marker is published.
pub enum RetainedActiveStore<'a> {
    Global(&'a GlobalStore),
    Project(&'a ProjectStore),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveGeneration {
    pub format: String,
    pub version: String,
    pub generation: String,
    pub global: ActiveGenerationDatabase,
    pub projects: Vec<ActiveGenerationProject>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveGenerationDatabase {
    pub database_id: String,
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveGenerationProject {
    pub root: String,
    pub database_id: String,
    pub path: String,
}

impl ActiveGeneration {
    pub fn new(
        generation: u64,
        global_database_id: String,
        global_path: &Path,
        projects: Vec<ActiveGenerationProject>,
    ) -> StoreResult<Self> {
        let marker = Self {
            format: MARKER_FORMAT.to_owned(),
            version: MARKER_VERSION.to_owned(),
            generation: generation.to_string(),
            global: ActiveGenerationDatabase {
                database_id: global_database_id,
                path: utf8_path(global_path, "global database path")?,
            },
            projects,
        };
        marker.validate_shape()?;
        Ok(marker)
    }

    pub fn generation_number(&self) -> StoreResult<u64> {
        parse_canonical_u64(&self.generation, "generation")
    }

    pub fn global_binding(&self) -> StoreResult<ActiveBinding> {
        Ok(ActiveBinding {
            generation: self.generation_number()?,
            database_id: self.global.database_id.clone(),
            kind: StoreKind::Global,
            canonical_path: PathBuf::from(&self.global.path),
        })
    }

    pub fn project_binding(&self, project: &ActiveGenerationProject) -> StoreResult<ActiveBinding> {
        Ok(ActiveBinding {
            generation: self.generation_number()?,
            database_id: project.database_id.clone(),
            kind: StoreKind::Project,
            canonical_path: PathBuf::from(&project.path),
        })
    }

    fn validate_shape(&self) -> StoreResult<()> {
        if self.format != MARKER_FORMAT || self.version != MARKER_VERSION {
            return marker_error("active-generation format or version is unsupported");
        }
        self.generation_number()?;
        validate_id(&self.global.database_id)?;
        validate_clean_absolute(Path::new(&self.global.path), "global database path")?;
        if self.projects.len() > PROJECT_LIMIT {
            return marker_error("active-generation project count exceeds the limit");
        }
        let mut roots = BTreeSet::new();
        let mut paths = BTreeSet::new();
        let mut database_ids = BTreeSet::new();
        paths.insert(self.global.path.as_str());
        database_ids.insert(self.global.database_id.as_str());
        let mut previous_root: Option<&str> = None;
        for project in &self.projects {
            validate_id(&project.database_id)?;
            validate_clean_absolute(Path::new(&project.root), "project root")?;
            validate_clean_absolute(Path::new(&project.path), "project database path")?;
            if !roots.insert(project.root.as_str())
                || !paths.insert(project.path.as_str())
                || !database_ids.insert(project.database_id.as_str())
            {
                return marker_error(
                    "active-generation contains duplicate database IDs, roots, or paths",
                );
            }
            if previous_root.is_some_and(|previous| previous >= project.root.as_str()) {
                return marker_error("active-generation project roots are not strictly sorted");
            }
            previous_root = Some(&project.root);
        }
        Ok(())
    }
}

/// Loads the one routing authority while the caller retains a shared lease.
pub fn load_active_generation(
    global_home: &Path,
    lease: &CutoverLease,
) -> StoreResult<Option<ActiveGeneration>> {
    require_matching_lease(global_home, lease)?;
    let path = marker_path(global_home);
    let file = match open_existing_private(&path)? {
        Some(file) => file,
        None => return Ok(None),
    };
    let length = file.metadata()?.len();
    if length == 0 || length > MARKER_LIMIT {
        return marker_error("active-generation marker size is invalid");
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(MARKER_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != length || bytes.last() != Some(&b'\n') {
        return marker_error("active-generation marker is truncated or noncanonical");
    }
    let marker: ActiveGeneration = serde_json::from_slice(&bytes).map_err(|_| {
        StoreError::ActivationBinding("active-generation marker is invalid".to_owned())
    })?;
    let mut canonical = serde_json::to_vec(&marker)
        .map_err(|error| StoreError::ActivationBinding(error.to_string()))?;
    canonical.push(b'\n');
    if canonical != bytes {
        return marker_error("active-generation marker is not canonical JSON");
    }
    marker.validate_shape()?;
    Ok(Some(marker))
}

/// Reopens and attests every destination named by a marker without mutation.
pub fn validate_active_generation(
    global_home: &Path,
    marker: &ActiveGeneration,
    writer_version: &str,
) -> StoreResult<()> {
    marker.validate_shape()?;
    let expected_global = fs::canonicalize(global_home)?.join("global.redb");
    if Path::new(&marker.global.path) != expected_global {
        return marker_error("global database is outside the fixed runtime path");
    }
    let global = GlobalStore::open_existing(&marker.global.path, &marker.global_binding()?)?;
    drop(global);
    for project in &marker.projects {
        let root = fs::canonicalize(&project.root)?;
        if root != Path::new(&project.root) {
            return marker_error("project root is not canonical");
        }
        let expected = root.join(".ptrack/ptrack.redb");
        if Path::new(&project.path) != expected {
            return marker_error("project database is outside the fixed runtime path");
        }
        let store = ProjectStore::open_existing(
            &project.path,
            &marker.project_binding(project)?,
            writer_version,
        )?;
        drop(store);
    }
    Ok(())
}

/// Publishes the canonical marker under an exclusive cutover lease, after all
/// destinations have been reopened and attested.
pub fn install_active_generation(
    global_home: &Path,
    lease: &CutoverLease,
    marker: &ActiveGeneration,
    writer_version: &str,
) -> StoreResult<()> {
    require_matching_lease(global_home, lease)?;
    if lease.mode() != CutoverLockMode::Exclusive {
        return marker_error("active-generation publication requires the exclusive cutover lease");
    }
    validate_active_generation(global_home, marker, writer_version)?;
    publish_marker(global_home, marker)
}

/// Publishes a marker while exact active redb handles remain live and locked.
///
/// # Errors
/// Returns an activation error unless the retained stores form the exact,
/// write-free destination set named by the marker under an exclusive lease.
pub fn install_active_generation_retained(
    global_home: &Path,
    lease: &CutoverLease,
    marker: &ActiveGeneration,
    stores: &[RetainedActiveStore<'_>],
) -> StoreResult<()> {
    require_matching_lease(global_home, lease)?;
    if lease.mode() != CutoverLockMode::Exclusive {
        return marker_error("active-generation publication requires the exclusive cutover lease");
    }
    marker.validate_shape()?;
    if stores.len() != marker.projects.len() + 1 {
        return marker_error("retained active store count does not match the marker");
    }
    let expected_global = fs::canonicalize(global_home)?.join("global.redb");
    if Path::new(&marker.global.path) != expected_global {
        return marker_error("global database is outside the fixed runtime path");
    }
    let mut global_seen = false;
    let mut projects_seen = vec![false; marker.projects.len()];
    for store in stores {
        let (binding, application_writes) = match store {
            RetainedActiveStore::Global(store) => (store.binding(), store.application_writes()?),
            RetainedActiveStore::Project(store) => (store.binding(), store.application_writes()?),
        };
        if application_writes {
            return marker_error("retained active store has application writes");
        }
        if binding.kind == StoreKind::Global {
            if global_seen || *binding != marker.global_binding()? {
                return marker_error("retained global store does not match the marker");
            }
            global_seen = true;
            continue;
        }
        let Some((index, project)) = marker.projects.iter().enumerate().find(|(index, project)| {
            !projects_seen[*index]
                && marker
                    .project_binding(project)
                    .is_ok_and(|expected| expected == *binding)
        }) else {
            return marker_error("retained project store does not match the marker");
        };
        let root = fs::canonicalize(&project.root)?;
        if root != Path::new(&project.root)
            || Path::new(&project.path) != root.join(".ptrack/ptrack.redb")
        {
            return marker_error("project database is outside the fixed runtime path");
        }
        projects_seen[index] = true;
    }
    if !global_seen || projects_seen.iter().any(|seen| !seen) {
        return marker_error("retained active stores do not cover the marker");
    }
    publish_marker(global_home, marker)
}

/// Restores a previously attested marker, or the previous absent state, under
/// an exclusive cutover lease. The caller owns the rollback write-fuse checks.
pub fn restore_active_generation(
    global_home: &Path,
    lease: &CutoverLease,
    previous: Option<&ActiveGeneration>,
    writer_version: &str,
) -> StoreResult<()> {
    require_matching_lease(global_home, lease)?;
    if lease.mode() != CutoverLockMode::Exclusive {
        return marker_error("active-generation restoration requires the exclusive cutover lease");
    }
    if let Some(marker) = previous {
        validate_active_generation(global_home, marker, writer_version)?;
        return publish_marker(global_home, marker);
    }
    let runtime = marker_path(global_home)
        .parent()
        .expect("marker path has parent")
        .to_path_buf();
    match fs::remove_file(marker_path(global_home)) {
        Ok(()) => sync_private_directory(&runtime)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn publish_marker(global_home: &Path, marker: &ActiveGeneration) -> StoreResult<()> {
    let runtime = marker_path(global_home)
        .parent()
        .expect("marker path has parent")
        .to_path_buf();
    let path = runtime.join(ACTIVE_GENERATION_MARKER);
    let temporary = runtime.join(".active-generation.json.tmp");
    if temporary.exists() {
        return marker_error("active-generation temporary file requires recovery");
    }
    let mut bytes = serde_json::to_vec(marker)
        .map_err(|error| StoreError::ActivationBinding(error.to_string()))?;
    bytes.push(b'\n');
    let mut file = create_private_new(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    replace_private_file(&temporary, &path)?;
    sync_private_directory(&runtime)?;
    Ok(())
}

fn require_matching_lease(global_home: &Path, lease: &CutoverLease) -> StoreResult<()> {
    if lease.path() != global_home.join("runtime/cutover.lock") {
        return marker_error("cutover lease does not belong to this global home");
    }
    Ok(())
}

fn marker_path(global_home: &Path) -> PathBuf {
    global_home.join("runtime").join(ACTIVE_GENERATION_MARKER)
}

fn validate_id(value: &str) -> StoreResult<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return marker_error("active-generation database ID is invalid");
    }
    Ok(())
}

fn validate_clean_absolute(path: &Path, label: &str) -> StoreResult<()> {
    if !path.is_absolute()
        || path.to_str().is_none()
        || !path.components().all(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
    {
        return marker_error(&format!("{label} must be absolute, clean UTF-8"));
    }
    Ok(())
}

fn parse_canonical_u64(value: &str, label: &str) -> StoreResult<u64> {
    if value.is_empty() || (value.len() > 1 && value.starts_with('0')) {
        return marker_error(&format!("{label} is not a canonical integer"));
    }
    let value = value
        .parse::<u64>()
        .map_err(|_| StoreError::ActivationBinding(format!("{label} is invalid")))?;
    if value == 0 {
        return marker_error(&format!("{label} must be nonzero"));
    }
    Ok(value)
}

fn utf8_path(path: &Path, label: &str) -> StoreResult<String> {
    validate_clean_absolute(path, label)?;
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| StoreError::ActivationBinding(format!("{label} is not UTF-8")))
}

fn marker_error<T>(detail: &str) -> StoreResult<T> {
    Err(StoreError::ActivationBinding(detail.to_owned()))
}

#[cfg(unix)]
fn open_existing_private(path: &Path) -> StoreResult<Option<File>> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let opened = file.metadata()?;
    let current = fs::symlink_metadata(path)?;
    if current.file_type().is_symlink()
        || !current.is_file()
        || current.dev() != opened.dev()
        || current.ino() != opened.ino()
        || opened.permissions().mode() & 0o077 != 0
    {
        return marker_error("active-generation marker identity or permissions are unsafe");
    }
    Ok(Some(file))
}

#[cfg(windows)]
fn open_existing_private(path: &Path) -> StoreResult<Option<File>> {
    match crate::private_windows::open_no_reparse(path, false, false, false) {
        Ok(file) => {
            crate::private_windows::verify_private_handle(&file)?;
            Ok(Some(file))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(any(unix, windows)))]
fn open_existing_private(_: &Path) -> StoreResult<Option<File>> {
    Err(StoreError::ActivationBinding(
        "active-generation file verification is unsupported".to_owned(),
    ))
}

#[cfg(unix)]
fn create_private_new(path: &Path) -> StoreResult<File> {
    use std::os::unix::fs::OpenOptionsExt;

    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}

#[cfg(windows)]
fn create_private_new(path: &Path) -> StoreResult<File> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    crate::private_windows::protect_file(path)?;
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn create_private_new(_: &Path) -> StoreResult<File> {
    Err(StoreError::ActivationBinding(
        "active-generation file creation is unsupported".to_owned(),
    ))
}
