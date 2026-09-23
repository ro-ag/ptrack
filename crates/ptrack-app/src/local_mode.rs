//! Project-only authority stored separately from the database format.
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use ptrack_store::{ActiveBinding, ActorIdentity, PinnedProjectDirectory, ProjectStore, StoreKind};
use serde::{Deserialize, Serialize};

use crate::{AppError, AppResult, ProjectEndpoint, WorkspaceBindings};

const FILE: &str = "local.json";
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocalMetadata {
    version: u32,
    root: PathBuf,
    generation: u64,
    database_id: String,
    actor_id: Option<String>,
    actor_name: Option<String>,
    guide: String,
}

pub(crate) fn global_refusal() -> AppError {
    AppError::Message("this command requires global access and is unavailable in project-local mode; run 'ptrack sync' outside the agent sandbox, or 'ptrack local disable' outside the sandbox before using global commands".to_owned())
}

impl LocalMetadata {
    pub(crate) fn capture(
        endpoint: &ProjectEndpoint,
        actor: Option<ActorIdentity>,
        guide: String,
    ) -> AppResult<Self> {
        if endpoint.database != endpoint.root.join(".ptrack/ptrack.redb") {
            return Err(AppError::Message(
                "local mode requires the existing project database at .ptrack/ptrack.redb"
                    .to_owned(),
            ));
        }
        Ok(Self {
            version: 1,
            root: endpoint.root.clone(),
            generation: endpoint.binding.generation,
            database_id: endpoint.binding.database_id.clone(),
            actor_id: actor.as_ref().map(|a| a.id.clone()),
            actor_name: actor.map(|a| a.name),
            guide,
        })
    }

    pub(crate) fn actor(&self) -> Option<ActorIdentity> {
        self.actor_id
            .as_ref()
            .zip(self.actor_name.as_ref())
            .map(|(id, name)| ActorIdentity {
                id: id.clone(),
                name: name.clone(),
            })
    }

    pub(crate) fn guide(&self) -> &str {
        &self.guide
    }

    pub(crate) fn bindings(&self, current: &Path, writer_version: &str) -> WorkspaceBindings {
        let database = self.root.join(".ptrack/ptrack.redb");
        let binding = ActiveBinding {
            generation: self.generation,
            database_id: self.database_id.clone(),
            kind: StoreKind::Project,
            canonical_path: database.clone(),
        };
        WorkspaceBindings {
            current_dir: current.to_owned(),
            project: Some(ProjectEndpoint {
                root: self.root.clone(),
                database,
                binding: binding.clone(),
            }),
            // These inert fields are never consulted by project-only applications.
            global_database: PathBuf::new(),
            global_binding: binding,
            global_home: PathBuf::new(),
            writer_version: writer_version.to_owned(),
        }
    }

    pub(crate) fn write(&self) -> AppResult<()> {
        let pinned = pin(&self.root)?;
        let bytes =
            serde_json::to_vec_pretty(self).map_err(|e| AppError::Message(e.to_string()))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(AppError::Message(
                "local metadata exceeds size limit".to_owned(),
            ));
        }
        // Serialize publishers with the project's existing publication lease. A
        // unique temporary file + fsync + rename preserves either complete version.
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|e| AppError::Message(e.to_string()))?;
        let name = format!(".local-{:032x}.tmp", u128::from_le_bytes(random));
        let dir = pinned.try_clone_project_directory()?;
        #[cfg(unix)]
        {
            use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, unlinkat};
            let fd = openat(
                &dir,
                name.as_str(),
                OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(std::io::Error::from)?;
            let mut file = fs::File::from(fd);
            let result = (|| -> AppResult<()> {
                file.write_all(&bytes)?;
                file.sync_all()?;
                pinned.verify()?;
                renameat(&dir, name.as_str(), &dir, FILE).map_err(std::io::Error::from)?;
                dir.sync_all()?;
                Ok(())
            })();
            if result.is_err() {
                let _ = unlinkat(&dir, name.as_str(), AtFlags::empty());
            }
            result
        }
        #[cfg(not(unix))]
        {
            let _ = dir;
            let directory = self.root.join(".ptrack");
            let temporary = directory.join(name);
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            let result = (|| -> AppResult<()> {
                ptrack_store::protect_private_file(&temporary)?;
                pinned.verify()?;
                file.write_all(&bytes)?;
                file.sync_all()?;
                drop(file);
                ptrack_store::replace_private_file(&temporary, &directory.join(FILE))?;
                ptrack_store::sync_private_directory(&directory)?;
                pinned.verify()?;
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            result
        }
    }
}

pub(crate) fn pin(root: &Path) -> AppResult<PinnedProjectDirectory> {
    let root_id = PinnedProjectDirectory::identify_root(root)?;
    let directory_id = PinnedProjectDirectory::identify_directory(root)?;
    Ok(PinnedProjectDirectory::prepare_expected_identities(
        root,
        root_id,
        directory_id,
    )?)
}

/// Discover local authority before consulting any global runtime. Discovery
/// stops at the nearest project metadata or Git boundary, including malformed
/// metadata; an invalid sidecar never falls back to home.
pub(crate) fn discover(current: &Path) -> AppResult<Option<LocalMetadata>> {
    discover_root(current)?
        .map(|root| read(&root))
        .transpose()
        .map(Option::flatten)
}

fn discover_root(current: &Path) -> AppResult<Option<PathBuf>> {
    let current = fs::canonicalize(current)?;
    for root in current.ancestors() {
        let metadata = root.join(".ptrack");
        match fs::symlink_metadata(&metadata) {
            Ok(entry) => {
                if !entry.is_dir() || entry.file_type().is_symlink() {
                    return Err(AppError::Message(
                        "project storage is unsafe: metadata directory must not be a symlink"
                            .to_owned(),
                    ));
                }
                return Ok(Some(root.to_owned()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        if fs::symlink_metadata(root.join(".git")).is_ok() {
            return Ok(None);
        }
    }
    Ok(None)
}

fn read(root: &Path) -> AppResult<Option<LocalMetadata>> {
    let path = root.join(".ptrack").join(FILE);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(m) if !m.is_file() || m.file_type().is_symlink() || m.len() > MAX_BYTES => {
            return Err(AppError::Message("unsafe local metadata file".to_owned()));
        }
        Ok(_) => {}
    }
    let pinned = pin(root)?;
    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags, openat};
        let dir = pinned.try_clone_project_directory()?;
        fs::File::from(
            openat(
                &dir,
                FILE,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        )
    };
    #[cfg(not(unix))]
    let file = ptrack_store::open_private_path(&path, false, false)?;
    if !file.metadata()?.is_file() {
        return Err(AppError::Message("unsafe local metadata file".to_owned()));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(AppError::Message(
            "local metadata exceeds size limit".to_owned(),
        ));
    }
    let value: LocalMetadata = serde_json::from_slice(&bytes).map_err(|e| {
        AppError::Message(format!(
            "invalid local metadata; refusing global fallback: {e}"
        ))
    })?;
    if value.version == 1 && value.root != root {
        // The project folder moved. `local enable` alone cannot help: the
        // registry still names the old folder, and `relocate` needs global
        // routing, which this sidecar blocks until it is removed.
        return Err(AppError::Message(format!(
            "local metadata was written for {} but this project is now at {}; outside the sandbox run 'ptrack local disable', then 'ptrack relocate', then 'ptrack local enable'",
            value.root.display(),
            root.display()
        )));
    }
    if value.version != 1
        || value.generation == 0
        || value.database_id.is_empty()
        || value.actor_id.is_some() != value.actor_name.is_some()
    {
        return Err(AppError::Message("local metadata identity does not match this project; run 'ptrack local enable' outside the sandbox".to_owned()));
    }
    if let Some(actor) = value.actor()
        && (!ptrack_core::is_identity_id(&actor.id)
            || ptrack_core::check_identity_name(&actor.name).is_err())
    {
        return Err(AppError::Message("invalid local actor identity".to_owned()));
    }
    pinned.verify()?;
    Ok(Some(value))
}

pub(crate) fn validate(
    metadata: &LocalMetadata,
    current: &Path,
    version: &str,
) -> AppResult<WorkspaceBindings> {
    let bindings = metadata.bindings(current, version);
    let endpoint = bindings.project.as_ref().ok_or(AppError::NoProject)?;
    let pinned = pin(&endpoint.root)?;
    drop(ProjectStore::open_existing_pinned(
        &pinned,
        &endpoint.binding,
        version,
    )?);
    Ok(bindings)
}

pub(crate) fn disable(current: &Path) -> AppResult<()> {
    let root = discover_root(current)?.ok_or(AppError::NoProject)?;
    let pinned = pin(&root)?;
    #[cfg(unix)]
    {
        let dir = pinned.try_clone_project_directory()?;
        rustix::fs::unlinkat(&dir, FILE, rustix::fs::AtFlags::empty())
            .map_err(std::io::Error::from)?;
        dir.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        fs::remove_file(root.join(".ptrack").join(FILE))?;
        ptrack_store::sync_private_directory(&root.join(".ptrack"))?;
    }
    pinned.verify()?;
    Ok(())
}
