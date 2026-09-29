//! Handle-relative file operations beneath a retained directory handle.
//!
//! The Windows counterpart of the `openat`/`renameat`/`unlinkat` family the
//! Unix publishers use: every name is one component resolved against the
//! directory handle itself, so a swapped pathname cannot redirect the
//! operation, and a final component that is a reparse point is never
//! followed.

use std::fs::File;
use std::io;
use std::path::Path;

use windows_sys::Wdk::Storage::FileSystem::{FILE_CREATE, FILE_OPEN};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ,
};

use crate::PrivatePathIdentity;
use crate::private_windows as sys;

/// Opens an existing real directory without following a reparse point.
///
/// The handle refuses delete sharing, so the directory cannot be renamed or
/// removed while it is held; entries beneath it stay fully usable.
///
/// # Errors
/// Returns an error when the path is missing, a reparse point, or not a
/// directory.
pub fn open_directory(path: &Path) -> io::Result<File> {
    sys::open_no_reparse_no_delete(path, true, false)
}

/// Returns the volume and file-index identity of an open handle.
///
/// # Errors
/// Returns an error when the operating system cannot describe the handle.
pub fn identity(file: &File) -> io::Result<PrivatePathIdentity> {
    let identity = sys::identity(file)?;
    Ok(PrivatePathIdentity {
        device: u64::from(identity.volume),
        inode: identity.index,
    })
}

/// Opens the existing regular file `name` beneath `directory` for reading,
/// or returns `None` when it is absent.
///
/// Only read sharing is granted, so the file cannot be written, renamed, or
/// deleted by anyone while the handle is held.
///
/// # Errors
/// Returns an error for an invalid name, a reparse point, a directory, or an
/// open that fails for any reason other than absence.
pub fn open_file_at(directory: &File, name: &str) -> io::Result<Option<File>> {
    absent_as_none(sys::open_relative(
        directory,
        name,
        FILE_GENERIC_READ,
        FILE_SHARE_READ,
        FILE_OPEN,
    ))
}

/// Opens the existing regular file `name` beneath `directory` for reading
/// with DELETE access, so the caller can verify it and then [`delete`] the
/// exact object it verified.
///
/// # Errors
/// As [`open_file_at`].
pub fn open_file_for_delete_at(directory: &File, name: &str) -> io::Result<Option<File>> {
    absent_as_none(sys::open_relative(
        directory,
        name,
        FILE_GENERIC_READ | DELETE,
        FILE_SHARE_READ,
        FILE_OPEN,
    ))
}

/// Creates the new regular file `name` beneath `directory`, failing with
/// [`io::ErrorKind::AlreadyExists`] when any entry already has that name.
///
/// The handle is exclusive and carries DELETE access, so it can later be
/// published with [`rename_at`] or discarded with [`delete`]. The new file
/// inherits `directory`'s descriptor.
///
/// # Errors
/// Returns an error for an invalid name or a failed create.
pub fn create_file_at(directory: &File, name: &str) -> io::Result<File> {
    sys::open_relative(
        directory,
        name,
        FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE,
        0,
        FILE_CREATE,
    )
}

/// Renames the file behind `file` (opened with DELETE access) to `name`
/// beneath `directory`. Without `replace`, an existing entry makes the
/// rename fail instead of being overwritten.
///
/// # Errors
/// Returns an error for an invalid name, a refused replacement, or a failed
/// rename.
pub fn rename_at(file: &File, directory: &File, name: &str, replace: bool) -> io::Result<()> {
    sys::rename_handle(file, directory, name, replace)
}

/// Marks the exact object behind `file` (opened with DELETE access) for
/// deletion; it disappears when the last handle closes.
///
/// # Errors
/// Returns an error when the disposition cannot be set.
pub fn delete(file: &File) -> io::Result<()> {
    sys::delete_directory_handle(file)
}

fn absent_as_none(result: io::Result<File>) -> io::Result<Option<File>> {
    match result {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
