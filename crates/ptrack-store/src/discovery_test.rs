use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::discovery::init_project_directory_from;
#[cfg(unix)]
use super::{StoreError, find_project_database};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ptrack-discovery-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn empty_init_root_uses_nearest_git_directory_or_file_boundary() {
    for git_is_file in [false, true] {
        let temp = Temp::new();
        let root = temp.0.join(if git_is_file {
            "worktree"
        } else {
            "repository"
        });
        let nested = root.join("a/b");
        fs::create_dir_all(&nested).unwrap();
        if git_is_file {
            fs::write(root.join(".git"), b"gitdir: elsewhere").unwrap();
        } else {
            fs::create_dir(root.join(".git")).unwrap();
        }
        let database = init_project_directory_from(std::path::Path::new(""), &nested).unwrap();
        assert_eq!(database, root.join(".ptrack/ptrack.redb"));
    }
}

#[test]
fn empty_init_root_falls_back_to_current_directory_without_git() {
    let temp = Temp::new();
    let database = init_project_directory_from(std::path::Path::new(""), &temp.0).unwrap();
    assert_eq!(database, temp.0.join(".ptrack/ptrack.redb"));
}

#[cfg(unix)]
#[test]
fn discovery_rejects_symlink_database_and_git_marker() {
    use std::os::unix::fs::symlink;

    let temp = Temp::new();
    let root = temp.0.join("repository");
    let metadata = root.join(".ptrack");
    fs::create_dir_all(&metadata).unwrap();
    fs::write(root.join("target"), b"not opened").unwrap();
    symlink(root.join("target"), metadata.join("ptrack.redb")).unwrap();
    assert!(matches!(
        find_project_database(&root),
        Err(StoreError::SymbolicLink { .. })
    ));

    fs::remove_file(metadata.join("ptrack.redb")).unwrap();
    symlink(root.join("target"), root.join(".git")).unwrap();
    assert!(matches!(
        init_project_directory_from(std::path::Path::new(""), &root),
        Err(StoreError::SymbolicLink { .. })
    ));

    let real = temp.0.join("real-root");
    fs::create_dir(&real).unwrap();
    let linked = temp.0.join("linked-root");
    symlink(&real, &linked).unwrap();
    assert!(matches!(
        init_project_directory_from(&linked, &temp.0),
        Err(StoreError::SymbolicLink { .. })
    ));
}
