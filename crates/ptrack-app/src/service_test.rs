use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ptrack_core::PlanStatus;
use ptrack_store::{ActiveBinding, GlobalStore, ProjectStore, StoreKind};

use crate::{
    ApplicationPort, InitRequest, LocalApplication, Mutation, MutationResult, ProjectEndpoint,
    WorkspaceBindings,
};
#[cfg(unix)]
use crate::{GuideAction, HookAction, HookResult};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("ptrack-app-{name}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(std::fs::canonicalize(path).expect("canonical test directory"))
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn binding(path: &Path, kind: StoreKind, id: &str) -> ActiveBinding {
    ActiveBinding {
        generation: 9,
        database_id: id.to_owned(),
        kind,
        canonical_path: path.to_path_buf(),
    }
}

fn configured(test: &TestDirectory, create_project: bool) -> (LocalApplication, ProjectEndpoint) {
    let root = test.0.join("project");
    let home = test.0.join("home");
    std::fs::create_dir_all(root.join(".ptrack")).expect("project directory");
    std::fs::create_dir_all(&home).expect("home directory");
    let project_database = root.join(".ptrack/ptrack.redb");
    let global_database = home.join("global.redb");
    let project_binding = binding(&project_database, StoreKind::Project, "project-9");
    let global_binding = binding(&global_database, StoreKind::Global, "global-9");
    drop(
        GlobalStore::create_new(&global_database, global_binding.clone())
            .expect("create global store"),
    );
    if create_project {
        drop(
            ProjectStore::create_new(&project_database, project_binding.clone(), "test")
                .expect("create project store"),
        );
    }
    let endpoint = ProjectEndpoint {
        root: root.clone(),
        database: project_database,
        binding: project_binding,
    };
    let application = LocalApplication::new(WorkspaceBindings {
        current_dir: root,
        project: Some(endpoint.clone()),
        global_database,
        global_binding,
        global_home: home,
        writer_version: "test".to_owned(),
    });
    (application, endpoint)
}

#[test]
fn operations_reopen_and_drop_the_store() {
    let directory = TestDirectory::new("reopen");
    let (mut application, endpoint) = configured(&directory, true);
    let result = application
        .mutate(Mutation::AddPlan {
            title: "one".to_owned(),
            milestone_id: 0,
        })
        .expect("add plan");
    let MutationResult::Plan(plan) = result else {
        panic!("wrong mutation result");
    };
    assert_eq!(application.snapshot().expect("snapshot").plans.len(), 1);

    // A successful open while the application object remains alive proves the
    // preceding service operation retained no redb handle/lock.
    let concurrent =
        ProjectStore::open_existing(&endpoint.database, &endpoint.binding, "concurrent")
            .expect("store was not held idle");
    concurrent
        .set_plan_status(plan.id, PlanStatus::Done)
        .expect("concurrent write");
    drop(concurrent);
    assert_eq!(
        application.snapshot().expect("reload").plans[0].status,
        PlanStatus::Done
    );
}

#[test]
fn initialize_uses_the_explicit_binding_and_installs_no_ambient_authority() {
    let directory = TestDirectory::new("initialize");
    let (mut application, endpoint) = configured(&directory, false);
    let result = application
        .initialize(InitRequest {
            root: Some(endpoint.root.clone()),
            goal: "ship".to_owned(),
            force: false,
            no_guide: true,
        })
        .expect("initialize");
    assert_eq!(result.database, endpoint.database);
    assert!(!result.already_initialized);
    assert!(result.guide_files.is_empty());
    assert_eq!(application.snapshot().expect("snapshot").meta.goal, "ship");
}

#[cfg(unix)]
#[test]
fn guide_install_rejects_a_symbolic_link_destination() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new("guide-link");
    let (mut application, endpoint) = configured(&directory, true);
    let outside = directory.0.join("outside");
    std::fs::write(&outside, "private").expect("outside file");
    symlink(&outside, endpoint.root.join("AGENTS.md")).expect("guide link");
    let error = application
        .guide(GuideAction::Install)
        .expect_err("symlink must fail");
    assert!(error.to_string().contains("symbolic link"));
    assert_eq!(
        std::fs::read_to_string(outside).expect("outside unchanged"),
        "private"
    );
}

#[cfg(unix)]
#[test]
fn guide_refresh_preserves_existing_mode() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TestDirectory::new("guide-mode");
    let (mut application, endpoint) = configured(&directory, true);
    let guide = endpoint.root.join("AGENTS.md");
    std::fs::write(&guide, "private notes\n").expect("guide seed");
    std::fs::set_permissions(&guide, std::fs::Permissions::from_mode(0o600)).expect("private mode");

    application
        .guide(GuideAction::Install)
        .expect("guide install");
    assert_eq!(
        std::fs::metadata(guide)
            .expect("guide metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[test]
fn hook_operations_reject_links_and_publish_exact_executable_block() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let directory = TestDirectory::new("hook-safe");
    let (mut application, endpoint) = configured(&directory, true);
    let hooks = endpoint.root.join(".git/hooks");
    std::fs::create_dir_all(&hooks).expect("hooks directory");
    let hook = hooks.join("post-commit");
    let outside = directory.0.join("outside-hook");
    std::fs::write(&outside, "private").expect("outside hook");
    symlink(&outside, &hook).expect("hook link");
    let error = application
        .hook(HookAction::Install)
        .expect_err("linked hook must fail");
    assert!(error.to_string().contains("symbolic link"));
    assert_eq!(
        std::fs::read_to_string(&outside).expect("outside"),
        "private"
    );

    std::fs::remove_file(&hook).expect("remove link");
    let HookResult::Installed { changed, .. } =
        application.hook(HookAction::Install).expect("install hook")
    else {
        panic!("wrong hook result");
    };
    assert!(changed);
    assert_eq!(
        std::fs::read_to_string(&hook).expect("hook text"),
        concat!(
            "#!/bin/sh\n",
            "# ptrack:begin\n",
            "command -v ptrack >/dev/null 2>&1 && ptrack commit record --sha \"$(git rev-parse HEAD)\" --subject \"$(git log -1 --pretty=%s)\" >/dev/null 2>&1 || true\n",
            "# ptrack:end\n"
        )
    );
    assert_eq!(
        std::fs::metadata(&hook)
            .expect("hook metadata")
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}
