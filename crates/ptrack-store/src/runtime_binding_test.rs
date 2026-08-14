use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{
    ActiveBinding, ActiveGeneration, ActiveGenerationProject, CutoverLockMode, GlobalStore,
    ProjectStore, StoreKind, acquire_cutover_lock, install_active_generation,
    load_active_generation, protect_private_directory, protect_private_file,
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
