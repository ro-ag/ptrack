use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::{StoreError, StoreResult};

const RUNTIME_DIRECTORY: &str = "runtime";
const CUTOVER_LOCK: &str = "cutover.lock";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CutoverLockMode {
    Shared,
    Exclusive,
}

/// Process-owned lease fencing runtime store use against offline activation.
#[derive(Debug)]
pub struct CutoverLease {
    file: File,
    path: PathBuf,
    mode: CutoverLockMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PrivatePathIdentity {
    pub device: u64,
    pub inode: u64,
}

impl CutoverLease {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn mode(&self) -> CutoverLockMode {
        self.mode
    }
}

impl Drop for CutoverLease {
    fn drop(&mut self) {
        unlock(&self.file);
    }
}

/// Acquires the sole cutover lock. Runtime processes retain a shared lease;
/// the offline activation/rollback tool requires the exclusive lease.
///
/// # Errors
///
/// Returns an activation or I/O error when the home is unsafe, the lock file
/// cannot be protected, or another process holds a conflicting lease.
pub fn acquire_cutover_lock(
    global_home: &Path,
    mode: CutoverLockMode,
) -> StoreResult<CutoverLease> {
    let metadata = fs::symlink_metadata(global_home)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StoreError::ActivationBinding(
            "global home must be a real directory".to_owned(),
        ));
    }
    require_private_directory(global_home, "global home")?;
    let runtime = global_home.join(RUNTIME_DIRECTORY);
    ensure_private_directory(&runtime)?;
    let path = runtime.join(CUTOVER_LOCK);
    let file = open_lock_file(&path)?;
    require_private_file(&file, &path)?;
    lock(&file, mode)?;
    Ok(CutoverLease { file, path, mode })
}

#[cfg(unix)]
fn open_lock_file(path: &Path) -> StoreResult<File> {
    use std::os::unix::fs::OpenOptionsExt;

    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?)
}

#[cfg(not(unix))]
fn open_lock_file(path: &Path) -> StoreResult<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    #[cfg(windows)]
    crate::private_windows::protect_file(path)?;
    Ok(file)
}

fn ensure_private_directory(path: &Path) -> StoreResult<()> {
    match fs::create_dir(path) {
        Ok(()) => set_private_directory(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StoreError::ActivationBinding(
            "runtime directory must be a real directory".to_owned(),
        ));
    }
    require_private_directory(path, "runtime directory")
}

/// Applies current-user-only protection to a newly created real directory.
///
/// # Errors
///
/// Returns an activation or I/O error when the path is not a real directory or
/// its private protection cannot be applied.
pub fn protect_private_directory(path: &Path) -> StoreResult<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StoreError::ActivationBinding(
            "private directory must be a real directory".to_owned(),
        ));
    }
    set_private_directory(path)
}

/// Applies current-user-only protection to a newly created real file.
///
/// # Errors
///
/// Returns an activation or I/O error when the path is not a real file or its
/// private protection cannot be applied.
pub fn protect_private_file(path: &Path) -> StoreResult<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(StoreError::ActivationBinding(
            "private file must be a real file".to_owned(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    crate::private_windows::protect_file(path)?;
    #[cfg(not(any(unix, windows)))]
    return Err(StoreError::ActivationBinding(
        "private file protection is unsupported on this platform".to_owned(),
    ));
    Ok(())
}

/// Opens a no-reparse path and verifies current-user-only protection.
///
/// # Errors
///
/// Returns an activation or I/O error when the path is missing, unsafe, has the
/// wrong type, or cannot be opened with the requested access.
pub fn open_private_path(path: &Path, directory: bool, writable: bool) -> StoreResult<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = OpenOptions::new();
        options.read(true).write(writable && !directory);
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        let file = options.open(path)?;
        let opened = file.metadata()?;
        if opened.is_dir() != directory {
            return Err(StoreError::ActivationBinding(
                "private path has the wrong type".to_owned(),
            ));
        }
        let current = verify_private_path(path, directory)?;
        use std::os::unix::fs::MetadataExt;
        if current.device != opened.dev() || current.inode != opened.ino() {
            return Err(StoreError::ActivationBinding(
                "private path changed while it was opened".to_owned(),
            ));
        }
        Ok(file)
    }
    #[cfg(windows)]
    {
        let file = crate::private_windows::open_no_reparse(path, directory, writable, false)?;
        crate::private_windows::verify_private_handle(&file)?;
        Ok(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, directory, writable);
        Err(StoreError::ActivationBinding(
            "private path opening is unsupported on this platform".to_owned(),
        ))
    }
}

/// Verifies the private protection and stable identity of an already-open handle.
///
/// # Errors
/// Returns an activation or I/O error when the handle no longer identifies a
/// private real file or directory.
pub fn verify_private_open_handle(file: &File) -> StoreResult<PrivatePathIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        let metadata = file.metadata()?;
        if metadata.file_type().is_symlink() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(StoreError::ActivationBinding(
                "private handle identity or permissions are unsafe".to_owned(),
            ));
        }
        Ok(PrivatePathIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        crate::private_windows::verify_private_handle(file)?;
        let identity = crate::private_windows::identity(file)?;
        Ok(PrivatePathIdentity {
            device: u64::from(identity.volume),
            inode: identity.index,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(StoreError::ActivationBinding(
            "private handle verification is unsupported on this platform".to_owned(),
        ))
    }
}

/// Verifies type, identity, and current-user-only protection.
///
/// # Errors
///
/// Returns an activation or I/O error when the path is missing, reparsed,
/// incorrectly typed, or accessible to another principal.
pub fn verify_private_path(path: &Path, directory: bool) -> StoreResult<PrivatePathIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink()
            || metadata.is_dir() != directory
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(StoreError::ActivationBinding(
                "private path identity or permissions are unsafe".to_owned(),
            ));
        }
        Ok(PrivatePathIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        let file = crate::private_windows::open_no_reparse(path, directory, false, false)?;
        crate::private_windows::verify_private(path)?;
        let identity = crate::private_windows::identity(&file)?;
        Ok(PrivatePathIdentity {
            device: u64::from(identity.volume),
            inode: identity.index,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, directory);
        Err(StoreError::ActivationBinding(
            "private path verification is unsupported on this platform".to_owned(),
        ))
    }
}

/// Atomically replaces a protected file with a same-directory temporary file.
///
/// # Errors
///
/// Returns an activation or I/O error when either path is unsafe or the
/// platform replacement operation fails.
pub fn replace_private_file(temporary: &Path, destination: &Path) -> StoreResult<()> {
    verify_private_path(temporary, false)?;
    if let Some(parent) = destination.parent() {
        verify_private_path(parent, true)?;
    } else {
        return Err(StoreError::ActivationBinding(
            "private replacement destination has no parent".to_owned(),
        ));
    }
    #[cfg(unix)]
    fs::rename(temporary, destination)?;
    #[cfg(windows)]
    crate::private_windows::replace_file(temporary, destination)?;
    #[cfg(not(any(unix, windows)))]
    return Err(StoreError::ActivationBinding(
        "private file replacement is unsupported on this platform".to_owned(),
    ));
    Ok(())
}

/// Flushes a protected directory after a namespace mutation.
///
/// # Errors
///
/// Returns an activation or I/O error when the directory is unsafe or cannot
/// be flushed durably.
pub fn sync_private_directory(path: &Path) -> StoreResult<()> {
    open_private_path(path, true, cfg!(windows))?.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> StoreResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(windows)]
fn set_private_directory(path: &Path) -> StoreResult<()> {
    crate::private_windows::protect_directory(path)?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn set_private_directory(_: &Path) -> StoreResult<()> {
    Err(StoreError::ActivationBinding(
        "private directory protection is unsupported on this platform".to_owned(),
    ))
}

#[cfg(unix)]
fn require_private_directory(path: &Path, label: &str) -> StoreResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.permissions().mode() & 0o077 == 0 {
        return Ok(());
    }
    // Leaked group/other bits — a restore, a sync, a copy under a default
    // umask — are healed by tightening, never refused: removing access cannot
    // leak anything, while failing closed locked the whole runtime out
    // (v0.24.x field reports, file and directory alike). The re-read proves
    // the tightening took effect on a real directory.
    let healed = !metadata.file_type().is_symlink()
        && metadata.is_dir()
        && fs::set_permissions(path, fs::Permissions::from_mode(0o700)).is_ok()
        && fs::symlink_metadata(path)
            .map(|current| current.is_dir() && current.permissions().mode() & 0o077 == 0)
            .unwrap_or(false);
    if healed {
        Ok(())
    } else {
        Err(StoreError::ActivationBinding(format!(
            "{label} permissions are not private"
        )))
    }
}

#[cfg(windows)]
fn require_private_directory(path: &Path, _: &str) -> StoreResult<()> {
    drop(crate::private_windows::open_no_reparse(
        path, true, false, false,
    )?);
    crate::private_windows::verify_private(path)?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn require_private_directory(_: &Path, _: &str) -> StoreResult<()> {
    Err(StoreError::ActivationBinding(
        "private directory verification is unsupported on this platform".to_owned(),
    ))
}

#[cfg(unix)]
fn require_private_file(file: &File, path: &Path) -> StoreResult<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let opened = file.metadata()?;
    let current = fs::symlink_metadata(path)?;
    if current.file_type().is_symlink()
        || !current.is_file()
        || current.dev() != opened.dev()
        || current.ino() != opened.ino()
        || opened.permissions().mode() & 0o077 != 0
    {
        return Err(StoreError::ActivationBinding(
            "cutover lock identity or permissions are unsafe".to_owned(),
        ));
    }
    if opened.permissions().mode() & 0o777 != 0o600 {
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(windows)]
fn require_private_file(file: &File, path: &Path) -> StoreResult<()> {
    let reopened = crate::private_windows::open_no_reparse(path, false, false, false)?;
    if crate::private_windows::identity(&reopened)? != crate::private_windows::identity(file)? {
        return Err(StoreError::ActivationBinding(
            "cutover lock identity changed".to_owned(),
        ));
    }
    crate::private_windows::verify_private(path)?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn require_private_file(_: &File, _: &Path) -> StoreResult<()> {
    Err(StoreError::ActivationBinding(
        "private file verification is unsupported on this platform".to_owned(),
    ))
}

#[cfg(unix)]
fn lock(file: &File, mode: CutoverLockMode) -> StoreResult<()> {
    use rustix::fs::{FlockOperation, flock};

    let operation = match mode {
        CutoverLockMode::Shared => FlockOperation::LockShared,
        CutoverLockMode::Exclusive => FlockOperation::NonBlockingLockExclusive,
    };
    flock(file, operation).map_err(|error| {
        StoreError::ActivationBinding(format!("cutover lock is unavailable: {error}"))
    })
}

#[cfg(unix)]
fn unlock(file: &File) {
    let _ = rustix::fs::flock(file, rustix::fs::FlockOperation::Unlock);
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn lock(file: &File, mode: CutoverLockMode) -> StoreResult<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let mut overlapped = OVERLAPPED::default();
    let flags = if mode == CutoverLockMode::Exclusive {
        LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY
    } else {
        LOCKFILE_FAIL_IMMEDIATELY
    };
    // SAFETY: the handle and OVERLAPPED pointer remain valid for the call;
    // the leased byte range is fixed and unlocked by Drop.
    let result = unsafe { LockFileEx(file.as_raw_handle(), flags, 0, 1, 0, &raw mut overlapped) };
    if result == 0 {
        let error = std::io::Error::last_os_error();
        let detail = if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
            "cutover lock is unavailable".to_owned()
        } else {
            format!("cutover lock failed: {error}")
        };
        return Err(StoreError::ActivationBinding(detail));
    }
    Ok(())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn unlock(file: &File) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let mut overlapped = OVERLAPPED::default();
    // SAFETY: the handle is live for Drop and the byte range matches lock().
    let _ = unsafe { UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &raw mut overlapped) };
}

#[cfg(not(any(unix, windows)))]
fn lock(_: &File, _: CutoverLockMode) -> StoreResult<()> {
    Err(StoreError::ActivationBinding(
        "cutover locking is unsupported on this platform".to_owned(),
    ))
}

#[cfg(not(any(unix, windows)))]
fn unlock(_: &File) {}
