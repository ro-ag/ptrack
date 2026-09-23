use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{
    ActiveBinding, ActiveGeneration, ActiveGenerationProject, CutoverLockMode, GlobalStore,
    ProjectStore, StoreKind, acquire_bootstrap_lock, acquire_cutover_lock,
    append_active_generation, install_active_generation, load_active_generation,
    protect_private_directory, protect_private_file, retire_active_generation,
    validate_active_generation,
};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ptrack-runtime-binding-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        private_directory(&path);
        Self(path.canonicalize().unwrap())
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn marker_is_the_canonical_attested_routing_authority() {
    let temp = Temp::new();
    let project_root = temp.0.join("project");
    fs::create_dir(&project_root).unwrap();
    private_directory(&project_root);
    fs::create_dir(project_root.join(".ptrack")).unwrap();
    private_directory(&project_root.join(".ptrack"));
    let global_path = temp.0.join("global.redb");
    let project_path = project_root.join(".ptrack/ptrack.redb");
    let global_binding = binding(&global_path, StoreKind::Global, "global-1");
    let project_binding = binding(&project_path, StoreKind::Project, "project-1");
    drop(GlobalStore::create_new(&global_path, global_binding).unwrap());
    drop(ProjectStore::create_new(&project_path, project_binding, "test").unwrap());

    let marker = ActiveGeneration::new(
        7,
        "global-1".to_owned(),
        &global_path,
        vec![ActiveGenerationProject {
            root: project_root.to_str().unwrap().to_owned(),
            database_id: "project-1".to_owned(),
            path: project_path.to_str().unwrap().to_owned(),
        }],
    )
    .unwrap();
    let shared = acquire_cutover_lock(&temp.0, CutoverLockMode::Shared).unwrap();
    assert!(load_active_generation(&temp.0, &shared).unwrap().is_none());
    assert!(
        install_active_generation(&temp.0, &shared, &marker, "test")
            .unwrap_err()
            .to_string()
            .contains("exclusive cutover lease")
    );
    drop(shared);

    let exclusive = acquire_cutover_lock(&temp.0, CutoverLockMode::Exclusive).unwrap();
    install_active_generation(&temp.0, &exclusive, &marker, "test").unwrap();
    drop(exclusive);
    let shared = acquire_cutover_lock(&temp.0, CutoverLockMode::Shared).unwrap();
    let loaded = load_active_generation(&temp.0, &shared).unwrap().unwrap();
    assert_eq!(loaded, marker);
    validate_active_generation(&temp.0, &loaded, "test").unwrap();
}

#[test]
fn marker_rejects_unknown_noncanonical_or_unsafe_input() {
    let temp = Temp::new();
    let lease = acquire_cutover_lock(&temp.0, CutoverLockMode::Shared).unwrap();
    let path = temp.0.join("runtime/active-generation.json");
    let mut invalid = br#"{"format":"ptrack-active-generation","version":"1","generation":"1","global":{"database_id":"g","path":"/missing/global.redb"},"projects":[],"extra":true}"#.to_vec();
    invalid.push(b'\n');
    fs::write(&path, invalid).unwrap();
    private_file(&path);
    assert!(
        load_active_generation(&temp.0, &lease)
            .unwrap_err()
            .to_string()
            .contains("marker is invalid")
    );
}

#[test]
fn marker_shape_rejects_zero_unsorted_and_duplicate_authority() {
    let temp = Temp::new();
    let global = temp.0.join("global.redb");
    assert!(ActiveGeneration::new(0, "global".to_owned(), &global, Vec::new()).is_err());
    let first = ActiveGenerationProject {
        root: temp.0.join("z").to_string_lossy().into_owned(),
        database_id: "first".to_owned(),
        path: temp
            .0
            .join("z/.ptrack/ptrack.redb")
            .to_string_lossy()
            .into_owned(),
    };
    let second = ActiveGenerationProject {
        root: temp.0.join("a").to_string_lossy().into_owned(),
        database_id: "second".to_owned(),
        path: temp
            .0
            .join("a/.ptrack/ptrack.redb")
            .to_string_lossy()
            .into_owned(),
    };
    assert!(
        ActiveGeneration::new(1, "global".to_owned(), &global, vec![first.clone(), second])
            .is_err()
    );
    assert!(
        ActiveGeneration::new(
            1,
            "global".to_owned(),
            &global,
            vec![first.clone(), first.clone()],
        )
        .is_err()
    );
    let duplicate_global_id = ActiveGenerationProject {
        root: temp.0.join("a").to_string_lossy().into_owned(),
        database_id: "global".to_owned(),
        path: temp
            .0
            .join("a/.ptrack/ptrack.redb")
            .to_string_lossy()
            .into_owned(),
    };
    assert!(
        ActiveGeneration::new(1, "global".to_owned(), &global, vec![duplicate_global_id],).is_err()
    );
    let mut duplicate_project_id = first.clone();
    duplicate_project_id.root = temp.0.join("zz").to_string_lossy().into_owned();
    duplicate_project_id.path = temp
        .0
        .join("zz/.ptrack/ptrack.redb")
        .to_string_lossy()
        .into_owned();
    assert!(
        ActiveGeneration::new(
            1,
            "global".to_owned(),
            &global,
            vec![first, duplicate_project_id],
        )
        .is_err()
    );
}

fn binding(path: &Path, kind: StoreKind, database_id: &str) -> ActiveBinding {
    ActiveBinding {
        generation: 7,
        database_id: database_id.to_owned(),
        kind,
        canonical_path: path
            .parent()
            .unwrap()
            .canonicalize()
            .unwrap()
            .join(path.file_name().unwrap()),
    }
}

fn private_directory(path: &Path) {
    protect_private_directory(path).unwrap();
}

fn private_file(path: &Path) {
    protect_private_file(path).unwrap();
}

#[test]
fn appending_a_project_publishes_beside_live_shared_leases() {
    let temp = Temp::new();
    let world = append_world(&temp);
    let live = acquire_cutover_lock(&temp.0, CutoverLockMode::Shared).unwrap();

    let publication = acquire_bootstrap_lock(&temp.0).unwrap();
    let lease = acquire_cutover_lock(&temp.0, CutoverLockMode::Shared).unwrap();
    append_active_generation(
        &temp.0,
        &lease,
        &publication,
        &world.previous,
        &world.appended,
        "test",
    )
    .unwrap();

    assert_eq!(
        load_active_generation(&temp.0, &lease).unwrap().unwrap(),
        world.appended
    );
    drop(live);
}

#[test]
fn appending_refuses_an_exclusive_lease_a_foreign_writer_or_a_changed_generation() {
    let temp = Temp::new();
    let world = append_world(&temp);
    let publication = acquire_bootstrap_lock(&temp.0).unwrap();
    let lease = acquire_cutover_lock(&temp.0, CutoverLockMode::Shared).unwrap();

    let mut renumbered = world.appended.clone();
    renumbered.generation = "8".to_owned();
    assert!(
        append_active_generation(
            &temp.0,
            &lease,
            &publication,
            &world.previous,
            &renumbered,
            "test",
        )
        .unwrap_err()
        .to_string()
        .contains("changes the live generation")
    );

    let mut rebound = world.appended.clone();
    rebound.projects[0].database_id = "rebound".to_owned();
    assert!(
        append_active_generation(
            &temp.0,
            &lease,
            &publication,
            &world.previous,
            &rebound,
            "test",
        )
        .unwrap_err()
        .to_string()
        .contains("drops or rewrites")
    );

    let mut replaced = world.appended.clone();
    replaced
        .projects
        .retain(|project| project.database_id == "project-2");
    assert!(
        append_active_generation(
            &temp.0,
            &lease,
            &publication,
            &world.previous,
            &replaced,
            "test",
        )
        .unwrap_err()
        .to_string()
        .contains("adds no project")
    );

    assert!(
        append_active_generation(
            &temp.0,
            &lease,
            &lease,
            &world.previous,
            &world.appended,
            "test",
        )
        .unwrap_err()
        .to_string()
        .contains("bootstrap publication lease")
    );
    drop(lease);

    let exclusive = acquire_cutover_lock(&temp.0, CutoverLockMode::Exclusive).unwrap();
    assert!(
        append_active_generation(
            &temp.0,
            &exclusive,
            &publication,
            &world.previous,
            &world.appended,
            "test",
        )
        .unwrap_err()
        .to_string()
        .contains("shared cutover lease")
    );
}

#[test]
fn appending_refuses_a_marker_that_moved_since_the_plan_was_built() {
    let temp = Temp::new();
    let world = append_world(&temp);
    let publication = acquire_bootstrap_lock(&temp.0).unwrap();
    let lease = acquire_cutover_lock(&temp.0, CutoverLockMode::Shared).unwrap();
    let mut stale = world.previous.clone();
    stale.projects.clear();

    assert!(
        append_active_generation(
            &temp.0,
            &lease,
            &publication,
            &stale,
            &world.appended,
            "test"
        )
        .unwrap_err()
        .to_string()
        .contains("marker changed")
    );
}

#[test]
fn retiring_a_vanished_project_publishes_beside_live_shared_leases() {
    let temp = Temp::new();
    let world = append_world(&temp);
    let exclusive = acquire_cutover_lock(&temp.0, CutoverLockMode::Exclusive).unwrap();
    install_active_generation(&temp.0, &exclusive, &world.appended, "test").unwrap();
    drop(exclusive);
    let retired = world
        .appended
        .projects
        .iter()
        .find(|project| project.database_id == "project-2")
        .unwrap()
        .clone();
    fs::remove_dir_all(&retired.root).unwrap();
    let live = acquire_cutover_lock(&temp.0, CutoverLockMode::Shared).unwrap();

    let publication = acquire_bootstrap_lock(&temp.0).unwrap();
    let lease = acquire_cutover_lock(&temp.0, CutoverLockMode::Shared).unwrap();
    retire_active_generation(
        &temp.0,
        &lease,
        &publication,
        &world.appended,
        &world.previous,
        "test",
    )
    .unwrap();

    assert_eq!(
        load_active_generation(&temp.0, &lease).unwrap().unwrap(),
        world.previous
    );
    drop(live);
}

#[test]
fn retiring_refuses_a_live_root_an_addition_or_a_moved_marker() {
    let temp = Temp::new();
    let world = append_world(&temp);
    let exclusive = acquire_cutover_lock(&temp.0, CutoverLockMode::Exclusive).unwrap();
    install_active_generation(&temp.0, &exclusive, &world.appended, "test").unwrap();
    drop(exclusive);
    let publication = acquire_bootstrap_lock(&temp.0).unwrap();
    let lease = acquire_cutover_lock(&temp.0, CutoverLockMode::Shared).unwrap();

    // Every retired root still exists, so there is no evidence to act on.
    assert!(
        retire_active_generation(
            &temp.0,
            &lease,
            &publication,
            &world.appended,
            &world.previous,
            "test",
        )
        .unwrap_err()
        .to_string()
        .contains("root still exists")
    );

    assert!(
        retire_active_generation(
            &temp.0,
            &lease,
            &publication,
            &world.previous,
            &world.appended,
            "test",
        )
        .unwrap_err()
        .to_string()
        .contains("retires no project")
    );

    let mut stale = world.appended.clone();
    stale.generation = "8".to_owned();
    assert!(
        retire_active_generation(
            &temp.0,
            &lease,
            &publication,
            &stale,
            &world.previous,
            "test"
        )
        .unwrap_err()
        .to_string()
        .contains("changes the live generation")
    );
}

struct AppendWorld {
    previous: ActiveGeneration,
    appended: ActiveGeneration,
}

/// Publishes a one-project generation and returns it beside the two-project
/// marker that only appends `second` to it.
fn append_world(temp: &Temp) -> AppendWorld {
    let global_path = temp.0.join("global.redb");
    drop(
        GlobalStore::create_new(
            &global_path,
            binding(&global_path, StoreKind::Global, "global-1"),
        )
        .unwrap(),
    );
    let first = create_project(temp, "first", "project-1");
    let second = create_project(temp, "second", "project-2");
    let previous =
        ActiveGeneration::new(7, "global-1".to_owned(), &global_path, vec![first.clone()]).unwrap();
    let mut projects = vec![first, second];
    projects.sort_by(|left, right| left.root.cmp(&right.root));
    let appended = ActiveGeneration::new(7, "global-1".to_owned(), &global_path, projects).unwrap();
    let exclusive = acquire_cutover_lock(&temp.0, CutoverLockMode::Exclusive).unwrap();
    install_active_generation(&temp.0, &exclusive, &previous, "test").unwrap();
    drop(exclusive);
    AppendWorld { previous, appended }
}

fn create_project(temp: &Temp, name: &str, database_id: &str) -> ActiveGenerationProject {
    let root = temp.0.join(name);
    fs::create_dir(&root).unwrap();
    private_directory(&root);
    fs::create_dir(root.join(".ptrack")).unwrap();
    private_directory(&root.join(".ptrack"));
    let path = root.join(".ptrack/ptrack.redb");
    drop(
        ProjectStore::create_new(
            &path,
            binding(&path, StoreKind::Project, database_id),
            "test",
        )
        .unwrap(),
    );
    ActiveGenerationProject {
        root: root.to_str().unwrap().to_owned(),
        database_id: database_id.to_owned(),
        path: path.to_str().unwrap().to_owned(),
    }
}

#[cfg(unix)]
#[test]
fn runtime_load_defers_project_permission_but_publication_remains_strict() {
    use std::os::unix::fs::PermissionsExt;
    let temp = Temp::new();
    let world = append_world(&temp);
    let path = Path::new(&world.previous.projects[0].path);
    fs::set_permissions(path, fs::Permissions::from_mode(0o0)).unwrap();
    // Privileged test runners can bypass mode bits; do not claim denial there.
    if fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .is_ok()
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }
    crate::validate_active_generation_for_load(&temp.0, &world.previous, "test").unwrap();
    assert!(validate_active_generation(&temp.0, &world.previous, "test").is_err());
    let marker_path = temp.0.join("runtime/active-generation.json");
    let before = fs::read(&marker_path).unwrap();
    let lease = acquire_cutover_lock(&temp.0, CutoverLockMode::Exclusive).unwrap();
    assert!(install_active_generation(&temp.0, &lease, &world.previous, "test").is_err());
    assert_eq!(before, fs::read(marker_path).unwrap());
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn runtime_load_rejects_invalid_marker_and_corrupt_global_but_defers_projects() {
    let temp = Temp::new();
    let world = append_world(&temp);
    let mut invalid = world.previous.clone();
    invalid.version = "unsupported".into();
    assert!(crate::validate_active_generation_for_load(&temp.0, &invalid, "test").is_err());
    // A corrupt project database fails the command that resolves it, never
    // the runtime load every other project shares.
    let project = &world.previous.projects[0];
    let project_before = fs::read(&project.path).unwrap();
    fs::write(&project.path, b"corrupt project").unwrap();
    crate::validate_active_generation_for_load(&temp.0, &world.previous, "test").unwrap();
    assert!(validate_active_generation(&temp.0, &world.previous, "test").is_err());
    assert!(
        ProjectStore::open_existing(
            &project.path,
            &world.previous.project_binding(project).unwrap(),
            "test"
        )
        .is_err()
    );
    fs::write(&project.path, project_before).unwrap();
    fs::write(&world.previous.global.path, b"corrupt global").unwrap();
    assert!(crate::validate_active_generation_for_load(&temp.0, &world.previous, "test").is_err());
}

#[cfg(unix)]
#[test]
fn runtime_load_heals_a_group_readable_project_database() {
    use std::os::unix::fs::PermissionsExt;
    let temp = Temp::new();
    let world = append_world(&temp);
    let path = Path::new(&world.previous.projects[0].path);
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
    crate::validate_active_generation_for_load(&temp.0, &world.previous, "test").unwrap();
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    validate_active_generation(&temp.0, &world.previous, "test").unwrap();
}

#[test]
fn runtime_load_treats_a_missing_project_database_as_unavailable() {
    let temp = Temp::new();
    let world = append_world(&temp);
    let project = &world.previous.projects[0];
    fs::remove_file(&project.path).unwrap();
    crate::validate_active_generation_for_load(&temp.0, &world.previous, "test").unwrap();
    // A deleted `.ptrack` directory (git clean -fdx) is the same case.
    fs::remove_dir_all(Path::new(&project.root).join(".ptrack")).unwrap();
    crate::validate_active_generation_for_load(&temp.0, &world.previous, "test").unwrap();
    assert!(validate_active_generation(&temp.0, &world.previous, "test").is_err());
    // A vanished root is still reported, so the caller can prune it.
    fs::remove_dir_all(&project.root).unwrap();
    assert!(crate::validate_active_generation_for_load(&temp.0, &world.previous, "test").is_err());
}

#[test]
fn runtime_load_ignores_a_busy_project_and_other_projects_stay_usable() {
    let temp = Temp::new();
    let world = append_world(&temp);
    let busy = &world.appended.projects[0];
    let free = &world.appended.projects[1];
    let exclusive = acquire_cutover_lock(&temp.0, CutoverLockMode::Exclusive).unwrap();
    install_active_generation(&temp.0, &exclusive, &world.appended, "test").unwrap();
    drop(exclusive);
    // Another process's writer lock on one project database.
    let held = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&busy.path)
        .unwrap();
    held.lock().unwrap();
    let start = std::time::Instant::now();
    crate::validate_active_generation_for_load(&temp.0, &world.appended, "test").unwrap();
    assert!(start.elapsed() < std::time::Duration::from_millis(500));
    let store = ProjectStore::open_existing(
        &free.path,
        &world.appended.project_binding(free).unwrap(),
        "test",
    )
    .unwrap();
    store.add_plan("still works", 0).unwrap();
    drop(store);
    held.unlock().unwrap();
}

#[test]
fn a_leftover_marker_temporary_file_does_not_block_publication() {
    let temp = Temp::new();
    let world = append_world(&temp);
    let temporary = temp.0.join("runtime/.active-generation.json.tmp");
    fs::write(&temporary, b"interrupted publication").unwrap();
    let exclusive = acquire_cutover_lock(&temp.0, CutoverLockMode::Exclusive).unwrap();
    install_active_generation(&temp.0, &exclusive, &world.appended, "test").unwrap();
    assert!(!temporary.exists());
    drop(exclusive);
    let shared = acquire_cutover_lock(&temp.0, CutoverLockMode::Shared).unwrap();
    assert_eq!(
        load_active_generation(&temp.0, &shared).unwrap(),
        Some(world.appended)
    );
}

#[cfg(unix)]
#[test]
fn runtime_load_defers_denied_ancestor_only_after_fixed_path_validation() {
    use std::os::unix::fs::PermissionsExt;
    let temp = Temp::new();
    let world = append_world(&temp);
    let ancestor = temp.0.join("denied");
    fs::create_dir(&ancestor).unwrap();
    private_directory(&ancestor);
    let project = create_project(&temp, "denied/project", "denied-project");
    let mut marker = world.previous;
    marker.projects = vec![project.clone()];
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o0)).unwrap();
    let denied = fs::canonicalize(&project.root);
    if denied.is_ok() {
        fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o700)).unwrap();
        return;
    }
    let loaded = crate::validate_active_generation_for_load(&temp.0, &marker, "test");
    let strict = validate_active_generation(&temp.0, &marker, "test");
    marker.projects[0].path = temp.0.join("outside.redb").to_str().unwrap().to_owned();
    let invalid_path = crate::validate_active_generation_for_load(&temp.0, &marker, "test");
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        denied.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    loaded.unwrap();
    assert!(strict.is_err());
    assert!(
        invalid_path
            .unwrap_err()
            .to_string()
            .contains("fixed runtime path")
    );
}

#[cfg(unix)]
#[test]
fn runtime_load_rejects_symlinks_to_permission_denied_targets() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = Temp::new();
    let world = append_world(&temp);
    let project = &world.previous.projects[0];
    let database = Path::new(&project.path);
    let target = temp.0.join("denied-target.redb");
    fs::rename(database, &target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o0)).unwrap();
    symlink(&target, database).unwrap();
    let result = crate::validate_active_generation_for_load(&temp.0, &world.previous, "test");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        result,
        Err(crate::StoreError::SymbolicLink { .. })
    ));
    fs::remove_file(database).unwrap();
    let directory = Path::new(&project.root).join(".ptrack");
    fs::remove_dir(&directory).unwrap();
    let denied_directory = temp.0.join("denied-target-directory");
    fs::create_dir(&denied_directory).unwrap();
    fs::set_permissions(&denied_directory, fs::Permissions::from_mode(0o0)).unwrap();
    symlink(&denied_directory, &directory).unwrap();
    let result = crate::validate_active_generation_for_load(&temp.0, &world.previous, "test");
    fs::set_permissions(&denied_directory, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(
        result,
        Err(crate::StoreError::SymbolicLink { .. })
    ));
}
