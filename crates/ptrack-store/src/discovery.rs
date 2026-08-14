use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::paths::lexical_absolute;
use crate::store::DestinationParent;
use crate::{StoreError, StoreResult};

pub const PROJECT_DIRECTORY: &str = ".ptrack";
pub const PROJECT_DATABASE_FILENAME: &str = "ptrack.redb";
pub const GLOBAL_DATABASE_FILENAME: &str = "global.redb";

/// Retained identity guards for one canonical project root and its private
/// `.ptrack` directory.
///
/// The guards must remain alive through database creation so pathname swaps
/// cannot publish a store under a different root or metadata directory. On
/// Unix, the guard also retains an exclusive advisory lock on the canonical
/// root. That lock is the cooperative boundary between all ptrack publishers;
/// same-UID processes that deliberately ignore advisory locks are outside it.
#[derive(Clone)]
pub struct PinnedProjectDirectory {
    inner: Arc<PinnedProjectDirectoryInner>,
}

struct PinnedProjectDirectoryInner {
    root: DestinationParent,
    #[cfg(unix)]
    _publication_lease: crate::store::ProjectRootPublicationLease,
    directory: DestinationParent,
    database_path: PathBuf,
}

impl PinnedProjectDirectory {
    /// Reads the retained identity of an exact canonical project root without
    /// creating `.ptrack`.
    ///
    /// # Errors
    /// Returns an error when the path is not canonical or cannot be pinned as
    /// a real directory.
    pub fn identify_root(canonical_root: &Path) -> StoreResult<crate::PrivatePathIdentity> {
        if !canonical_root.is_absolute() || fs::canonicalize(canonical_root)? != canonical_root {
            return Err(StoreError::ActivationBinding(
                "project root must be an exact canonical directory".to_owned(),
            ));
        }
        let root =
            DestinationParent::capture_unrestricted(&canonical_root.join(PROJECT_DIRECTORY))?;
        root.ensure_current()?;
        Ok(root.identity())
    }

    /// Reads the no-follow identity of an existing private `.ptrack`
    /// directory through the retained canonical-root handle.
    ///
    /// # Errors
    /// Returns an error when either directory is unsafe, replaced, or missing.
    pub fn identify_directory(canonical_root: &Path) -> StoreResult<crate::PrivatePathIdentity> {
        if !canonical_root.is_absolute() || fs::canonicalize(canonical_root)? != canonical_root {
            return Err(StoreError::ActivationBinding(
                "project root must be an exact canonical directory".to_owned(),
            ));
        }
        let directory_path = canonical_root.join(PROJECT_DIRECTORY);
        let root = DestinationParent::capture_unrestricted(&directory_path)?;
        let directory = root.pin_private_child_directory(
            PROJECT_DIRECTORY,
            &directory_path,
            false,
            false,
            || Ok(()),
            || Ok(()),
        )?;
        Ok(directory.identity())
    }

    /// Pins an existing canonical root and creates or validates its private
    /// `.ptrack` directory without following a replacement child.
    ///
    /// # Errors
    /// Returns an error when the root is not already canonical, either pinned
    /// directory changes identity, or `.ptrack` is linked, non-directory, or
    /// not protected for the current user only. On Unix, also returns busy
    /// while another ptrack publisher retains the root lock.
    pub fn prepare(canonical_root: &Path) -> StoreResult<Self> {
        Self::prepare_inner(canonical_root, None, None, false, || Ok(()), || Ok(()))
    }

    /// Pins a root only when it retains a previously persisted identity.
    ///
    /// # Errors
    /// Returns before child creation when the pathname identifies a
    /// replacement root.
    pub fn prepare_expected(
        canonical_root: &Path,
        expected_root: crate::PrivatePathIdentity,
    ) -> StoreResult<Self> {
        Self::prepare_inner(
            canonical_root,
            Some(expected_root),
            None,
            false,
            || Ok(()),
            || Ok(()),
        )
    }

    /// Pins a root identity and creates `.ptrack` only when it is absent.
    ///
    /// # Errors
    /// Returns without adopting a preexisting child or a child published by a
    /// concurrent participant after target validation.
    pub fn prepare_new_expected(
        canonical_root: &Path,
        expected_root: crate::PrivatePathIdentity,
    ) -> StoreResult<Self> {
        Self::prepare_inner(
            canonical_root,
            Some(expected_root),
            None,
            true,
            || Ok(()),
            || Ok(()),
        )
    }

    /// Pins a prepared project only when both persisted directory identities
    /// still match. A missing or replaced `.ptrack` is never recreated.
    ///
    /// # Errors
    /// Returns before database creation when either identity changed.
    pub fn prepare_expected_identities(
        canonical_root: &Path,
        expected_root: crate::PrivatePathIdentity,
        expected_directory: crate::PrivatePathIdentity,
    ) -> StoreResult<Self> {
        Self::prepare_inner(
            canonical_root,
            Some(expected_root),
            Some(expected_directory),
            false,
            || Ok(()),
            || Ok(()),
        )
    }

    #[cfg(all(test, unix))]
    pub(crate) fn prepare_with_before_child_open(
        canonical_root: &Path,
        before_child_open: impl FnOnce() -> StoreResult<()>,
    ) -> StoreResult<Self> {
        Self::prepare_inner(canonical_root, None, None, true, before_child_open, || {
            Ok(())
        })
    }

    #[cfg(all(test, unix))]
    pub(crate) fn prepare_with_after_child_creation(
        canonical_root: &Path,
        after_child_creation: impl FnOnce() -> StoreResult<()>,
    ) -> StoreResult<Self> {
        Self::prepare_inner(
            canonical_root,
            None,
            None,
            true,
            || Ok(()),
            after_child_creation,
        )
    }

    fn prepare_inner(
        canonical_root: &Path,
        expected_root: Option<crate::PrivatePathIdentity>,
        expected_directory: Option<crate::PrivatePathIdentity>,
        require_child_absent: bool,
        before_child_open: impl FnOnce() -> StoreResult<()>,
        after_child_creation: impl FnOnce() -> StoreResult<()>,
    ) -> StoreResult<Self> {
        if !canonical_root.is_absolute() || fs::canonicalize(canonical_root)? != canonical_root {
            return Err(StoreError::ActivationBinding(
                "project root must be an exact canonical directory".to_owned(),
            ));
        }
        let directory_path = canonical_root.join(PROJECT_DIRECTORY);
        let database_path = directory_path.join(PROJECT_DATABASE_FILENAME);
        let root = DestinationParent::capture_unrestricted(&directory_path)?;
        if expected_root.is_some_and(|expected| root.identity() != expected) {
            return Err(StoreError::DestinationParentChanged {
                path: canonical_root.to_path_buf(),
            });
        }
        #[cfg(unix)]
        let publication_lease = root.acquire_project_publication_lease()?;
        let directory = root.pin_private_child_directory(
            PROJECT_DIRECTORY,
            &directory_path,
            expected_directory.is_none(),
            require_child_absent,
            before_child_open,
            after_child_creation,
        )?;
        if expected_directory.is_some_and(|expected| directory.identity() != expected) {
            return Err(StoreError::DestinationParentChanged {
                path: directory_path,
            });
        }
        let pinned = Self {
            inner: Arc::new(PinnedProjectDirectoryInner {
                root,
                #[cfg(unix)]
                _publication_lease: publication_lease,
                directory,
                database_path,
            }),
        };
        pinned.verify()?;
        Ok(pinned)
    }

    /// Returns the exact database path under the pinned project directory.
    #[must_use]
    pub fn database_path(&self) -> &Path {
        &self.inner.database_path
    }

    /// Returns the retained identity of the canonical project root.
    #[must_use]
    pub fn root_identity(&self) -> crate::PrivatePathIdentity {
        self.inner.root.identity()
    }

    /// Returns the retained identity of the private `.ptrack` directory.
    #[must_use]
    pub fn directory_identity(&self) -> crate::PrivatePathIdentity {
        self.inner.directory.identity()
    }

    /// Clones the retained canonical-project-root directory handle.
    ///
    /// # Errors
    /// Returns an error when the operating system cannot duplicate the exact
    /// handle or the pinned root no longer verifies.
    pub fn try_clone_root_directory(&self) -> StoreResult<std::fs::File> {
        self.verify()?;
        self.inner.root.try_clone_directory()
    }

    /// Clones the retained private `.ptrack` directory handle.
    ///
    /// # Errors
    /// Returns an error when the exact pinned directories no longer verify or
    /// the operating system cannot duplicate the child handle.
    pub fn try_clone_project_directory(&self) -> StoreResult<std::fs::File> {
        self.verify()?;
        self.inner.directory.try_clone_directory()
    }

    /// Revalidates both retained directory identities and private child policy.
    ///
    /// # Errors
    /// Returns an error after any root or `.ptrack` replacement or permission
    /// change.
    pub fn verify(&self) -> StoreResult<()> {
        self.inner.root.ensure_current()?;
        self.inner.directory.ensure_private_current()?;
        self.inner.root.ensure_current()?;
        Ok(())
    }

    pub(crate) fn create_database_file(&self) -> StoreResult<std::fs::File> {
        self.inner
            .directory
            .create_private_file_at(PROJECT_DATABASE_FILENAME)
    }

    pub(crate) fn open_database_file(&self) -> StoreResult<std::fs::File> {
        self.inner
            .directory
            .open_private_file_at(PROJECT_DATABASE_FILENAME)
    }

    pub(crate) fn sync(&self) -> StoreResult<()> {
        self.inner.directory.sync()
    }
}

pub fn find_project_database(start: impl AsRef<Path>) -> StoreResult<PathBuf> {
    let mut directory = std::fs::canonicalize(start)?;
    loop {
        let candidate = directory
            .join(PROJECT_DIRECTORY)
            .join(PROJECT_DATABASE_FILENAME);
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(StoreError::SymbolicLink { path: candidate });
            }
            Ok(metadata) if metadata.is_file() => return Ok(candidate),
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(StoreError::NotRegularFile { path: candidate });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if git_marker(&directory.join(".git"))? || !directory.pop() {
            return Err(StoreError::NotFound);
        }
    }
}

pub fn init_project_directory(root: impl AsRef<Path>) -> StoreResult<PathBuf> {
    init_project_directory_from(root.as_ref(), &std::env::current_dir()?)
}

pub(crate) fn init_project_directory_from(root: &Path, current: &Path) -> StoreResult<PathBuf> {
    let root = if root.as_os_str().is_empty() {
        enclosing_git_root(current)?.unwrap_or_else(|| current.to_path_buf())
    } else {
        lexical_absolute(root, current)?
    };
    validate_real_directory(&root)?;
    let directory = root.join(PROJECT_DIRECTORY);
    let database = directory.join(PROJECT_DATABASE_FILENAME);
    if database.exists() {
        return Err(StoreError::DestinationExists { path: database });
    }
    std::fs::create_dir_all(&directory)?;
    Ok(database)
}

fn validate_real_directory(path: &Path) -> StoreResult<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        Err(StoreError::SymbolicLink {
            path: path.to_path_buf(),
        })
    } else if metadata.is_dir() {
        Ok(())
    } else {
        Err(StoreError::NotRegularFile {
            path: path.to_path_buf(),
        })
    }
}

pub fn global_home_from(override_path: Option<&Path>, user_home: &Path) -> PathBuf {
    override_path.map_or_else(|| user_home.join(PROJECT_DIRECTORY), Path::to_path_buf)
}

fn git_marker(path: &Path) -> StoreResult<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(StoreError::SymbolicLink {
            path: path.to_path_buf(),
        }),
        Ok(metadata) if metadata.is_dir() || metadata.is_file() => Ok(true),
        Ok(_) => Err(StoreError::NotRegularFile {
            path: path.to_path_buf(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn enclosing_git_root(start: &Path) -> StoreResult<Option<PathBuf>> {
    let mut directory = start.to_path_buf();
    loop {
        if git_marker(&directory.join(".git"))? {
            return Ok(Some(directory));
        }
        if !directory.pop() {
            return Ok(None);
        }
    }
}
