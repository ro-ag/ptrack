use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::store::{FileIdentity, ensure_path_identity};
use crate::{ProjectStore, Store, StoreError, StoreKind, StoreResult};

static NEXT_BACKUP_TEMP: AtomicU64 = AtomicU64::new(1);

impl ProjectStore {
    /// Copies a transaction-consistent store image to a create-only private path.
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> StoreResult<PathBuf> {
        let destination = destination.as_ref();
        if destination.exists() {
            return Err(StoreError::DestinationExists {
                path: destination.to_path_buf(),
            });
        }
        self.backup_to_inner(destination, |_| Ok(()))?;
        Ok(destination.to_path_buf())
    }

    pub(crate) fn backup_to_inner(
        &self,
        destination: &Path,
        after_copy: impl FnOnce(&Path) -> StoreResult<()>,
    ) -> StoreResult<()> {
        let expected_binding = self.binding().clone();
        let expected_provenance = self.json_stage_provenance()?;
        self.backup_with_writer_barrier(|source, source_file| {
            let source_identity = FileIdentity::from_path(source, false)?;
            ensure_path_identity(source, source_identity)?;
            let parent = BackupParent::prepare(destination)?;
            let temporary = parent.temporary_path(destination)?;
            let temporary_identity = copy_private(source_file, &temporary)?;
            let verified = (|| {
                ensure_path_identity(source, source_identity)?;
                ensure_path_identity(&temporary, temporary_identity)?;
                after_copy(&temporary)?;
                ensure_path_identity(&temporary, temporary_identity)?;
                let backup = Store::open_existing(&temporary, StoreKind::Project)?;
                if backup.active_binding()? != Some(expected_binding)
                    || backup.json_stage_provenance()? != expected_provenance
                {
                    return Err(StoreError::InvalidManifest(
                        "backup attestation does not match its source".to_owned(),
                    ));
                }
                drop(backup);
                ensure_path_identity(source, source_identity)?;
                ensure_path_identity(&temporary, temporary_identity)?;
                parent.publish(&temporary, temporary_identity, destination)
            })();
            if verified.is_err() {
                remove_owned_temporary(&temporary, temporary_identity);
            }
            verified
        })
    }

    pub(crate) fn backup_with_writer_barrier<R>(
        &self,
        operation: impl FnOnce(&Path, &fs::File) -> StoreResult<R>,
    ) -> StoreResult<R> {
        self.raw_writer_barrier(operation)
    }
}

fn copy_private(source: &fs::File, destination: &Path) -> StoreResult<FileIdentity> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(destination)?;
    #[cfg(windows)]
    crate::protect_private_file(destination)?;
    let identity = FileIdentity::from_file(&output)?;
    #[cfg(not(windows))]
    {
        let mut input = source.try_clone()?;
        io::copy(&mut input, &mut output)?;
    }
    #[cfg(windows)]
    {
        use std::io::Write as _;
        use std::os::windows::fs::FileExt as _;

        let mut offset = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = source.seek_read(&mut buffer, offset)?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read])?;
            offset = offset.saturating_add(read as u64);
        }
    }
    output.sync_all()?;
    ensure_path_identity(destination, identity)?;
    Ok(identity)
}

struct BackupParent {
    path: PathBuf,
    directory: fs::File,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    identity: FileIdentity,
}

impl BackupParent {
    fn prepare(destination: &Path) -> StoreResult<Self> {
        let path = destination
            .parent()
            .ok_or_else(|| StoreError::DestinationParentInvalid {
                path: destination.to_path_buf(),
            })?
            .to_path_buf();
        let existed = path.exists();
        fs::create_dir_all(&path)?;
        if existed {
            crate::verify_private_path(&path, true)?;
        } else {
            crate::protect_private_directory(&path)?;
        }
        Self::capture(path)
    }

    #[cfg(unix)]
    fn capture(path: PathBuf) -> StoreResult<Self> {
        use std::os::unix::fs::MetadataExt;

        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(StoreError::DestinationParentInvalid { path });
        }
        let directory = fs::File::open(&path)?;
        let opened = directory.metadata()?;
        if metadata.dev() != opened.dev() || metadata.ino() != opened.ino() {
            return Err(StoreError::DestinationParentChanged { path });
        }
        Ok(Self {
            path,
            directory,
            device: opened.dev(),
            inode: opened.ino(),
        })
    }

    #[cfg(windows)]
    fn capture(path: PathBuf) -> StoreResult<Self> {
        let directory = crate::private_windows::open_no_reparse(&path, true, true, false)?;
        crate::private_windows::verify_private_handle(&directory)?;
        let identity = FileIdentity::from_file(&directory)?;
        if FileIdentity::from_path(&path, true)? != identity {
            return Err(StoreError::DestinationParentChanged { path });
        }
        Ok(Self {
            path,
            directory,
            identity,
        })
    }

    #[cfg(not(any(unix, windows)))]
    fn capture(path: PathBuf) -> StoreResult<Self> {
        Err(StoreError::DestinationParentIdentityUnavailable { path })
    }

    #[cfg(unix)]
    fn ensure_current(&self) -> StoreResult<()> {
        use std::os::unix::fs::MetadataExt;

        let metadata = fs::symlink_metadata(&self.path)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || metadata.dev() != self.device
            || metadata.ino() != self.inode
        {
            return Err(StoreError::DestinationParentChanged {
                path: self.path.clone(),
            });
        }
        Ok(())
    }

    #[cfg(windows)]
    fn ensure_current(&self) -> StoreResult<()> {
        crate::private_windows::verify_private_handle(&self.directory)?;
        if FileIdentity::from_file(&self.directory)? != self.identity
            || FileIdentity::from_path(&self.path, true)? != self.identity
        {
            return Err(StoreError::DestinationParentChanged {
                path: self.path.clone(),
            });
        }
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    fn ensure_current(&self) -> StoreResult<()> {
        Err(StoreError::DestinationParentIdentityUnavailable {
            path: self.path.clone(),
        })
    }

    fn temporary_path(&self, destination: &Path) -> StoreResult<PathBuf> {
        self.ensure_current()?;
        let name = destination
            .file_name()
            .ok_or_else(|| StoreError::DestinationParentInvalid {
                path: destination.to_path_buf(),
            })?;
        for _ in 0..32 {
            let number = NEXT_BACKUP_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = self.path.join(format!(
                ".{}.ptrack-backup-{}-{number}.tmp",
                name.to_string_lossy(),
                std::process::id()
            ));
            if !path.exists() {
                return Ok(path);
            }
        }
        Err(StoreError::Io(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a backup temporary path",
        )))
    }

    fn publish(
        &self,
        temporary: &Path,
        identity: FileIdentity,
        destination: &Path,
    ) -> StoreResult<()> {
        self.ensure_current()?;
        ensure_path_identity(temporary, identity)?;
        match fs::hard_link(temporary, destination) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(StoreError::DestinationExists {
                    path: destination.to_path_buf(),
                });
            }
            Err(error) => return Err(error.into()),
        }
        let published = (|| {
            ensure_path_identity(destination, identity)?;
            self.directory.sync_all()?;
            ensure_path_identity(destination, identity)?;
            ensure_path_identity(temporary, identity)?;
            fs::remove_file(temporary)?;
            self.directory.sync_all()?;
            self.ensure_current()?;
            ensure_path_identity(destination, identity)
        })();
        if published.is_err() {
            remove_owned_temporary(destination, identity);
            let _ = self.directory.sync_all();
        }
        published
    }
}

fn remove_owned_temporary(path: &Path, identity: FileIdentity) {
    if ensure_path_identity(path, identity).is_ok() {
        let _ = fs::remove_file(path);
    }
}
