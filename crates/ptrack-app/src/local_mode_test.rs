use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{ApplicationPort, GuideAction, HookAction, InitRequest, Mutation, RoutedApplication};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    home: PathBuf,
    project: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ptrack-local-mode-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let home = root.join("home");
        let project = root.join("project");
        fs::create_dir(&project).unwrap();
        let mut app = RoutedApplication::new(home.clone(), project.clone(), "test");
        app.initialize(InitRequest {
            root: Some(project.clone()),
            goal: "initial".to_owned(),
            force: false,
            no_guide: true,
        })
        .unwrap();
        app.set_identity("Local User").unwrap();
        app.local_mode("enable").unwrap();
        Self {
            root,
            home,
            project,
        }
    }
    fn app(&self) -> RoutedApplication {
        RoutedApplication::new(self.home.clone(), self.project.clone(), "test")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn local_commands_work_when_home_is_a_regular_file_and_sync_retains_data() {
    let fixture = Fixture::new();
    let original = fixture.app().bindings().unwrap().project.unwrap().binding;
    let saved = fixture.root.join("saved-home");
    fs::rename(&fixture.home, &saved).unwrap();
    fs::write(&fixture.home, "home must not be opened").unwrap();
    let mut app = fixture.app();
    assert_eq!(app.snapshot().unwrap().meta.goal, "initial");
    app.mutate(Mutation::SetGoal("sandbox work".to_owned()))
        .unwrap();
    assert_eq!(app.identity().unwrap().unwrap().name, "Local User");
    assert!(app.guide(GuideAction::Print).is_ok());
    assert!(app.projects().unwrap_err().to_string().contains("outside"));
    assert!(app.backup().unwrap_err().to_string().contains("outside"));
    assert!(
        app.hook(HookAction::Status)
            .unwrap_err()
            .to_string()
            .contains("outside")
    );
    assert!(app.require_global_mode().is_err());
    assert!(app.local_mode("sync").is_err());
    assert_eq!(
        fs::read_to_string(&fixture.home).unwrap(),
        "home must not be opened"
    );
    fs::remove_file(&fixture.home).unwrap();
    fs::rename(saved, &fixture.home).unwrap();
    app.local_mode("sync").unwrap();
    assert_eq!(app.snapshot().unwrap().meta.goal, "sandbox work");
    assert_eq!(app.bindings().unwrap().project.unwrap().binding, original);
    app.local_mode("disable").unwrap();
    assert_eq!(fixture.app().snapshot().unwrap().meta.goal, "sandbox work");
}

#[test]
fn malformed_or_copied_metadata_refuses_global_fallback() {
    let fixture = Fixture::new();
    let path = fixture.project.join(".ptrack/local.json");
    let original = fs::read(&path).unwrap();
    fs::write(&path, b"{broken").unwrap();
    assert!(
        fixture
            .app()
            .snapshot()
            .unwrap_err()
            .to_string()
            .contains("local metadata")
    );
    assert!(fixture.app().projects().is_err());
    let mut metadata: serde_json::Value = serde_json::from_slice(&original).unwrap();
    metadata["root"] = serde_json::json!(fixture.home);
    fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    assert!(fixture.app().snapshot().is_err());
    metadata["root"] = serde_json::json!(fixture.project);
    metadata["actor_id"] = serde_json::json!("invalid");
    fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    assert!(fixture.app().snapshot().is_err());
}

#[test]
fn sync_without_local_mode_does_not_enable_it() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.local_mode("disable").unwrap();
    app.local_mode("sync").unwrap();
    assert!(!fixture.project.join(".ptrack/local.json").exists());
    assert!(app.projects().is_ok());
}

#[cfg(unix)]
#[test]
fn local_mode_rejects_symlinked_database_without_touching_target() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    let database = fixture.project.join(".ptrack/ptrack.redb");
    let saved = fixture.project.join(".ptrack/saved.redb");
    fs::rename(&database, saved).unwrap();
    let outside = fixture.root.join("outside");
    fs::write(&outside, "do not touch").unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
    symlink(&outside, &database).unwrap();
    assert!(fixture.app().snapshot().is_err());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "do not touch");
    assert_eq!(
        fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

#[cfg(unix)]
#[test]
fn local_mode_rejects_symlinked_metadata() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let path = fixture.project.join(".ptrack/local.json");
    let outside = fixture.root.join("outside.json");
    fs::rename(&path, &outside).unwrap();
    symlink(&outside, &path).unwrap();
    assert!(fixture.app().snapshot().is_err());
}

#[test]
fn nested_git_boundary_does_not_inherit_parent_local_authority() {
    let fixture = Fixture::new();
    let child = fixture.project.join("nested");
    fs::create_dir(&child).unwrap();
    fs::create_dir(child.join(".git")).unwrap();
    assert!(crate::local_mode::discover(&child).unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn metadata_directory_symlink_is_rejected_before_reading_children() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let metadata = fixture.project.join(".ptrack");
    let outside = fixture.root.join("external-metadata");
    fs::rename(&metadata, &outside).unwrap();
    symlink(&outside, &metadata).unwrap();
    assert!(
        fixture
            .app()
            .snapshot()
            .unwrap_err()
            .to_string()
            .contains("project storage is unsafe")
    );
}

#[test]
fn disable_can_recover_malformed_local_metadata_without_reading_it() {
    let fixture = Fixture::new();
    fs::write(fixture.project.join(".ptrack/local.json"), "invalid").unwrap();
    let mut app = fixture.app();
    assert!(app.snapshot().is_err());
    app.local_mode("disable").unwrap();
    assert_eq!(app.snapshot().unwrap().meta.goal, "initial");
}
