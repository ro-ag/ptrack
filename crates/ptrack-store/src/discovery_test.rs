use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::discovery::{PinnedProjectDirectory, init_project_directory_from};
#[cfg(unix)]
use super::{ActiveBinding, ProjectStore, StoreError, StoreKind, find_project_database};

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
        crate::protect_private_directory(&path).unwrap();
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

#[cfg(windows)]
#[test]
fn windows_pinned_project_directory_protects_a_new_child_from_its_handle() {
    let temp = Temp::new();
    let root = temp.0.join("windows-project");
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    assert!(!root.join(".ptrack").exists());
    let pinned = PinnedProjectDirectory::prepare(&root).unwrap();
    pinned.verify().unwrap();
    assert_eq!(pinned.database_path(), root.join(".ptrack/ptrack.redb"));
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

#[cfg(unix)]
#[test]
fn pinned_project_directory_creates_and_binds_exact_private_child() {
    let temp = Temp::new();
    let root = temp.0.join("project");
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let pinned = PinnedProjectDirectory::prepare(&root).unwrap();
    assert_eq!(pinned.database_path(), root.join(".ptrack/ptrack.redb"));
    pinned.verify().unwrap();
    let binding = project_binding(pinned.database_path());
    let store = ProjectStore::create_new_pinned(&pinned, binding, "test").unwrap();
    assert_eq!(store.path(), pinned.database_path());
}

#[cfg(unix)]
#[test]
fn pinned_new_project_rejects_a_preexisting_empty_private_child() {
    let temp = Temp::new();
    let root = temp.0.join("project");
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let root_identity = PinnedProjectDirectory::identify_root(&root).unwrap();
    fs::create_dir(root.join(".ptrack")).unwrap();
    crate::protect_private_directory(&root.join(".ptrack")).unwrap();
    fs::write(root.join(".ptrack/existing-marker"), b"do not adopt").unwrap();

    assert!(PinnedProjectDirectory::prepare_new_expected(&root, root_identity).is_err());
    assert!(root.join(".ptrack/existing-marker").exists());
    assert!(!root.join(".ptrack/ptrack.redb").exists());
}

#[cfg(unix)]
#[test]
fn pinned_project_directory_rejects_root_replacement_before_store_creation() {
    let temp = Temp::new();
    let root = temp.0.join("project");
    let held = temp.0.join("held-project");
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let pinned = PinnedProjectDirectory::prepare(&root).unwrap();
    fs::rename(&root, &held).unwrap();
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join(".ptrack")).unwrap();
    crate::protect_private_directory(&root.join(".ptrack")).unwrap();
    let binding = project_binding(pinned.database_path());
    assert!(ProjectStore::create_new_pinned(&pinned, binding, "test").is_err());
    assert!(!root.join(".ptrack/ptrack.redb").exists());
}

#[cfg(unix)]
#[test]
fn pinned_directory_creation_stays_on_retained_root_after_path_replacement() {
    let temp = Temp::new();
    let root = temp.0.join("project");
    let held = temp.0.join("held-project");
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let result = PinnedProjectDirectory::prepare_with_before_child_open(&root, || {
        fs::rename(&root, &held)?;
        fs::create_dir(&root)?;
        Ok(())
    });
    assert!(result.is_err());
    assert!(!root.join(".ptrack").exists());
    assert!(held.join(".ptrack").exists());
}

#[cfg(unix)]
#[test]
fn project_root_lock_excludes_second_publisher_after_child_creation() {
    let temp = Temp::new();
    let root = temp.0.join("project");
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let pinned = PinnedProjectDirectory::prepare_with_after_child_creation(&root, || {
        assert!(matches!(
            PinnedProjectDirectory::prepare(&root),
            Err(StoreError::Busy)
        ));
        assert!(root.join(".ptrack").exists());
        Ok(())
    })
    .unwrap();
    pinned.verify().unwrap();
    assert!(root.join(".ptrack").is_dir());
}

#[cfg(unix)]
#[test]
fn pinned_project_directory_rejects_child_replacement_before_store_creation() {
    let temp = Temp::new();
    let root = temp.0.join("project");
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let pinned = PinnedProjectDirectory::prepare(&root).unwrap();
    fs::rename(root.join(".ptrack"), root.join("held-ptrack")).unwrap();
    fs::create_dir(root.join(".ptrack")).unwrap();
    crate::protect_private_directory(&root.join(".ptrack")).unwrap();
    let binding = project_binding(pinned.database_path());
    assert!(ProjectStore::create_new_pinned(&pinned, binding, "test").is_err());
    assert!(!root.join(".ptrack/ptrack.redb").exists());
}

#[cfg(unix)]
#[test]
fn pinned_create_fails_closed_when_child_moves_after_final_verify_before_open() {
    let temp = Temp::new();
    let root = temp.0.join("project");
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let pinned = PinnedProjectDirectory::prepare(&root).unwrap();
    let binding = project_binding(pinned.database_path());
    let result = ProjectStore::create_new_pinned_with_before_open(&pinned, binding, "test", || {
        fs::rename(root.join(".ptrack"), root.join("held-ptrack"))?;
        fs::create_dir(root.join(".ptrack"))?;
        crate::protect_private_directory(&root.join(".ptrack"))?;
        Ok(())
    });
    assert!(result.is_err());
    assert!(!root.join(".ptrack/ptrack.redb").exists());
    assert!(root.join("held-ptrack/ptrack.redb").exists());
}

#[cfg(unix)]
#[test]
fn pinned_open_fails_closed_when_child_moves_after_final_verify_before_open() {
    let temp = Temp::new();
    let root = temp.0.join("project");
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let pinned = PinnedProjectDirectory::prepare(&root).unwrap();
    let binding = project_binding(pinned.database_path());
    drop(ProjectStore::create_new_pinned(&pinned, binding.clone(), "test").unwrap());
    let result =
        ProjectStore::open_existing_pinned_with_before_open(&pinned, &binding, "test", || {
            fs::rename(root.join(".ptrack"), root.join("held-ptrack"))?;
            fs::create_dir(root.join(".ptrack"))?;
            crate::protect_private_directory(&root.join(".ptrack"))?;
            Ok(())
        });
    assert!(result.is_err());
    assert!(!root.join(".ptrack/ptrack.redb").exists());
    assert!(root.join("held-ptrack/ptrack.redb").exists());
}

#[cfg(unix)]
fn project_binding(path: &std::path::Path) -> ActiveBinding {
    ActiveBinding {
        generation: 1,
        database_id: "pinned-project".to_owned(),
        kind: StoreKind::Project,
        canonical_path: path.to_path_buf(),
    }
}
