use std::path::{Path, PathBuf};

use crate::paths::lexical_absolute;
use crate::{StoreError, StoreResult};

pub const PROJECT_DIRECTORY: &str = ".ptrack";
pub const PROJECT_DATABASE_FILENAME: &str = "ptrack.redb";
pub const GLOBAL_DATABASE_FILENAME: &str = "global.redb";

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
