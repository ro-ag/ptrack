use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ptrack_store::{
    ActiveBinding, ActiveGeneration, ActiveGenerationProject, CutoverLockMode, ProjectStore,
    StoreKind, acquire_cutover_lock, load_active_generation, protect_private_directory,
    protect_private_file,
};

use crate::{
    ActiveRuntime, ApplicationPort, DesktopWorkspaceFactory, InitRequest,
    ProductionDesktopWorkspaceFactory, ProductionRecentProjects, RecentProjectsProvider,
    RoutedApplication,
};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Temp(PathBuf);

#[derive(serde::Serialize)]
struct TestBootstrapPlan {
    format: &'static str,
    version: &'static str,
    previous_marker: ActiveGeneration,
    target_marker: ActiveGeneration,
    project_root: String,
}

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ptrack-production-{}-{}",
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
fn routed_init_publishes_marker_before_user_writes_and_nested_cwd_resolves() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    let nested = project.join("src/deep");
    fs::create_dir(&home).unwrap();
    fs::create_dir_all(&nested).unwrap();
    private_directory(&home);
    private_directory(&project);

    let mut application = RoutedApplication::new(home.clone(), nested.clone(), "test");
    let result = application
        .initialize(InitRequest {
            root: Some(project.clone()),
            goal: "ship".to_owned(),
            force: false,
            no_guide: true,
        })
        .unwrap();
    assert!(!result.already_initialized);
    assert_eq!(application.snapshot().unwrap().meta.goal, "ship");
    let bindings = application.bindings().unwrap();
    assert_eq!(bindings.project.unwrap().root, project);

    drop(application);
    let lease = acquire_cutover_lock(&home, CutoverLockMode::Shared).unwrap();
    let marker = load_active_generation(&home, &lease).unwrap().unwrap();
    assert_eq!(marker.projects.len(), 1);
    assert_eq!(marker.projects[0].root, project.to_string_lossy());
}

#[test]
fn routed_init_uses_an_explicit_root_outside_the_process_cwd() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let process_cwd = temp.0.join("process-cwd");
    let project = temp.0.join("explicit-project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&process_cwd).unwrap();
    fs::create_dir(&project).unwrap();
    private_directory(&home);

    let mut application = RoutedApplication::new(home, process_cwd, "test");
    let result = application
        .initialize(InitRequest {
            root: Some(project.clone()),
            goal: "explicit".to_owned(),
            force: false,
            no_guide: true,
        })
        .unwrap();
    assert_eq!(result.database, project.join(".ptrack/ptrack.redb"));
    assert_eq!(application.snapshot().unwrap().meta.goal, "explicit");
    assert_eq!(
        application.bindings().unwrap().project.unwrap().root,
        project
    );
}

#[test]
fn production_recents_reprobe_a_mapped_project_store() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    private_directory(&home);

    let mut application = RoutedApplication::new(home, project.clone(), "test");
    application
        .initialize(InitRequest {
            root: Some(project.clone()),
            goal: String::new(),
            force: false,
            no_guide: true,
        })
        .unwrap();
    let runtime = application.active_runtime().unwrap().unwrap();
    let recents = ProductionRecentProjects::new(runtime);
    assert_eq!(recents.recent_projects().unwrap()[0]["available"], true);

    fs::remove_file(project.join(".ptrack/ptrack.redb")).unwrap();
    assert_eq!(recents.recent_projects().unwrap()[0]["available"], false);
}

#[test]
fn routed_init_refuses_legacy_global_and_project_databases() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    private_directory(&home);
    fs::write(home.join("global.db"), b"legacy").unwrap();
    let request = InitRequest {
        root: Some(project.clone()),
        goal: String::new(),
        force: false,
        no_guide: true,
    };
    let mut application = RoutedApplication::new(home.clone(), project.clone(), "test");
    assert!(
        application
            .initialize(request.clone())
            .unwrap_err()
            .to_string()
            .contains("legacy global.db requires the offline migration workflow")
    );

    fs::remove_file(home.join("global.db")).unwrap();
    fs::create_dir(project.join(".ptrack")).unwrap();
    fs::write(project.join(".ptrack/ptrack.db"), b"legacy").unwrap();
    let mut application = RoutedApplication::new(home, project, "test");
    assert!(
        application
            .initialize(request)
            .unwrap_err()
            .to_string()
            .contains("legacy .ptrack/ptrack.db requires the offline migration workflow")
    );
}

#[test]
fn data_calls_distinguish_uninitialized_from_recovery_required() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    fs::create_dir(&home).unwrap();
    private_directory(&home);
    let mut application = RoutedApplication::new(home.clone(), temp.0.clone(), "test");
    assert!(
        application
            .snapshot()
            .unwrap_err()
            .to_string()
            .contains("runtime is not initialized")
    );

    drop(application);
    let lease = acquire_cutover_lock(&home, CutoverLockMode::Exclusive).unwrap();
    let marker = home.join("runtime/active-generation.json");
    fs::write(&marker, b"{}\n").unwrap();
    private_file(&marker);
    drop(lease);
    let mut application = RoutedApplication::new(home, temp.0.clone(), "test");
    assert!(
        application
            .snapshot()
            .unwrap_err()
            .to_string()
            .contains("runtime recovery is required")
    );
}

#[test]
fn bootstrap_resumes_unpublished_stores_and_deepest_project_wins() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let outer = temp.0.join("outer");
    let inner = outer.join("inner");
    let deep = inner.join("src");
    fs::create_dir(&home).unwrap();
    fs::create_dir_all(&deep).unwrap();
    for directory in [&home, &outer, &inner] {
        private_directory(directory);
    }
    fs::create_dir(outer.join(".ptrack")).unwrap();
    private_directory(&outer.join(".ptrack"));

    let mut outer_app = RoutedApplication::new(home.clone(), outer.clone(), "test");
    outer_app
        .initialize(InitRequest {
            root: Some(outer.clone()),
            goal: "outer".to_owned(),
            force: false,
            no_guide: true,
        })
        .unwrap();
    drop(outer_app);

    fs::create_dir(inner.join(".ptrack")).unwrap();
    private_directory(&inner.join(".ptrack"));
    let lease = acquire_cutover_lock(&home, CutoverLockMode::Exclusive).unwrap();
    let previous = load_active_generation(&home, &lease).unwrap().unwrap();
    let generation = previous.generation_number().unwrap();
    let project_path = inner.join(".ptrack/ptrack.redb");
    let project = ActiveGenerationProject {
        root: inner.to_string_lossy().into_owned(),
        database_id: "resumed-project".to_owned(),
        path: project_path.to_string_lossy().into_owned(),
    };
    let mut projects = previous.projects.clone();
    projects.push(project.clone());
    projects.sort_by(|left, right| left.root.cmp(&right.root));
    let target = ActiveGeneration::new(
        generation,
        previous.global.database_id.clone(),
        Path::new(&previous.global.path),
        projects,
    )
    .unwrap();
    let binding = ActiveBinding {
        generation,
        database_id: project.database_id.clone(),
        kind: StoreKind::Project,
        canonical_path: project_path.clone(),
    };
    drop(ProjectStore::create_new(&project_path, binding, "test").unwrap());
    let plan = TestBootstrapPlan {
        format: "ptrack-bootstrap-plan",
        version: "1",
        previous_marker: previous,
        target_marker: target,
        project_root: inner.to_string_lossy().into_owned(),
    };
    let plan_path = home.join("runtime/bootstrap.json");
    let mut bytes = serde_json::to_vec(&plan).unwrap();
    bytes.push(b'\n');
    fs::write(&plan_path, bytes).unwrap();
    private_file(&plan_path);
    drop(lease);

    let mut application = RoutedApplication::new(home.clone(), deep, "test");
    application
        .initialize(InitRequest {
            root: Some(inner.clone()),
            goal: "resumed".to_owned(),
            force: false,
            no_guide: true,
        })
        .unwrap();
    assert!(!plan_path.exists());
    assert_eq!(application.bindings().unwrap().project.unwrap().root, inner);
    assert_eq!(application.snapshot().unwrap().meta.goal, "resumed");
}

#[test]
fn production_workspace_factory_composes_and_shuts_down_real_services() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    private_directory(&home);
    private_directory(&project);
    let mut application = RoutedApplication::new(home.clone(), project.clone(), "test");
    application
        .initialize(InitRequest {
            root: Some(project.clone()),
            goal: "production".to_owned(),
            force: false,
            no_guide: true,
        })
        .unwrap();
    drop(application);

    let runtime = ActiveRuntime::load(&home, "test").unwrap().unwrap();
    let factory = ProductionDesktopWorkspaceFactory::new(runtime, None, 0).unwrap();
    let workspace = factory.build(&project, 1).unwrap();
    assert_eq!(Path::new(&workspace.project().root), project);
    workspace.shutdown().unwrap();
}

fn private_directory(path: &Path) {
    protect_private_directory(path).unwrap();
}

fn private_file(path: &Path) {
    protect_private_file(path).unwrap();
}
