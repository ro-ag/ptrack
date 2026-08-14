use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use ptrack_store::{
    ActiveBinding, ActiveGeneration, ActiveGenerationProject, CutoverLockMode, GlobalStore,
    PinnedProjectDirectory, PrivatePathIdentity, ProjectStore, StoreKind, acquire_cutover_lock,
    load_active_generation, protect_private_directory, protect_private_file,
};

use crate::{
    ActiveRuntime, ApplicationPort, DesktopCommandRequest, DesktopInitializationService,
    DesktopRuntime, DesktopRuntimeConfig, DesktopWorkspaceFactory, InitRequest,
    InitializationCheckpointV1, InitializationOutcomeV1, InitializationStatusV1,
    InitializeProjectRequestV1, NoRecentProjectsProvider, ProductionDesktopAuthority,
    ProductionDesktopWorkspaceFactory, ProductionRecentProjects, ProjectGuideChoiceV1,
    ProjectGuideFileActionV1, ProjectGuidePreviewRequestV1, ProjectTargetKindV1,
    RecentProjectAvailabilityV1, RecentProjectsProvider, RoutedApplication,
    UnavailableUpdateService, WorkspaceStatus, production_desktop_runtime,
};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Temp(PathBuf);

#[derive(Clone, serde::Serialize)]
struct TestBootstrapPlan {
    format: &'static str,
    version: &'static str,
    operation_id: Option<String>,
    previous_marker: Option<ActiveGeneration>,
    target_marker: ActiveGeneration,
    project_root: String,
    project_root_identity: PrivatePathIdentity,
    project_directory_identity: PrivatePathIdentity,
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

fn desktop_request(method: &str, arguments: Vec<serde_json::Value>) -> DesktopCommandRequest {
    DesktopCommandRequest {
        method: method.to_owned(),
        arguments,
    }
}

#[test]
#[allow(clippy::too_many_lines)] // One shipping-constructor smoke spans cancel through reopen.
fn production_desktop_json_smoke_first_launch_cancel_initialize_onboard_and_reopen() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let first = production_desktop_runtime(home.clone(), "test", &temp.0, None, 0).unwrap();

    let welcome = first
        .invoke(desktop_request("GetWorkspaceState", Vec::new()))
        .unwrap();
    assert_eq!(welcome["status"], "welcome");
    assert_eq!(welcome["generation"], 0);
    assert_eq!(
        first
            .invoke(desktop_request("GetPendingInitializationV1", Vec::new()))
            .unwrap(),
        serde_json::json!({ "pending": false })
    );
    let abandoned = first
        .invoke(desktop_request(
            "ValidateProjectTargetV1",
            vec![serde_json::json!(project)],
        ))
        .unwrap();
    assert_eq!(abandoned["kind"], "new");
    assert!(!project.join(".ptrack").exists());
    assert!(!home.exists());
    first.begin_shutdown().unwrap();
    drop(first);

    let desktop = production_desktop_runtime(home.clone(), "test", &temp.0, None, 0).unwrap();
    assert_eq!(desktop.workspace_state().status, WorkspaceStatus::Welcome);
    assert_eq!(
        desktop
            .invoke(desktop_request("GetPendingInitializationV1", Vec::new()))
            .unwrap(),
        serde_json::json!({ "pending": false })
    );
    let validation = desktop
        .invoke(desktop_request(
            "ValidateProjectTargetV1",
            vec![serde_json::json!(project)],
        ))
        .unwrap();
    assert_eq!(validation["kind"], "new");
    assert!(!project.join(".ptrack").exists());
    let initialization_request = desktop_request(
        "InitializeProjectV1",
        vec![serde_json::json!({
            "operationId": validation["operationId"],
            "root": validation["canonicalRoot"],
            "goal": "  Ship the production smoke  ",
            "guideChoice": "skip",
            "guidePreviewToken": ""
        })],
    );
    let initialized = desktop.invoke(initialization_request.clone()).unwrap();
    assert_eq!(initialized["initialization"]["checkpoint"], "desktop-bound");
    assert_eq!(initialized["initialization"]["outcome"], "complete");
    assert_eq!(initialized["state"]["status"], "open");
    assert_eq!(initialized["state"]["generation"], 1);
    assert!(project.join(".ptrack/ptrack.redb").is_file());
    assert_eq!(
        desktop.invoke(initialization_request.clone()).unwrap(),
        initialized
    );
    assert_eq!(desktop.workspace_state().generation, 1);

    let plan = desktop
        .invoke(desktop_request(
            "CreateFirstPlanV1",
            vec![serde_json::json!(1), serde_json::json!("  First plan  ")],
        ))
        .unwrap();
    assert_eq!(plan["plan"]["id"], 1);
    assert_eq!(plan["plan"]["title"], "First plan");
    assert_eq!(plan["plan"]["status"], "active");
    assert_eq!(plan["state"]["generation"], 1);
    assert_eq!(
        desktop
            .invoke(desktop_request(
                "CreateFirstPlanV1",
                vec![serde_json::json!(1), serde_json::json!("First plan")],
            ))
            .unwrap(),
        plan
    );
    let task = desktop
        .invoke(desktop_request(
            "CreateFirstTaskV1",
            vec![
                serde_json::json!(1),
                serde_json::json!(1),
                serde_json::json!("  First task  "),
            ],
        ))
        .unwrap();
    assert_eq!(task["task"]["status"], "todo");
    let started = desktop
        .invoke(desktop_request(
            "StartFirstTaskV1",
            vec![
                serde_json::json!(1),
                serde_json::json!(1),
                task["task"]["updatedAt"].clone(),
            ],
        ))
        .unwrap();
    assert_eq!(started["task"]["status"], "doing");
    assert_eq!(started["state"]["generation"], 1);
    let recents = desktop
        .invoke(desktop_request("GetRecentProjectsV1", Vec::new()))
        .unwrap();
    assert_eq!(recents["projects"].as_array().unwrap().len(), 1);
    assert_eq!(
        recents["projects"][0]["canonicalPath"],
        project.to_str().unwrap()
    );
    assert_eq!(recents["projects"][0]["availability"], "available");
    assert!(
        recents["projects"][0]["entryId"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(
        recents["projects"][0]["base"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    desktop.begin_shutdown().unwrap();
    drop(desktop);

    let replay = production_desktop_runtime(home.clone(), "test", &temp.0, None, 0).unwrap();
    assert_eq!(replay.workspace_state().status, WorkspaceStatus::Welcome);
    assert_eq!(
        replay.invoke(initialization_request.clone()).unwrap(),
        initialized
    );
    assert_eq!(
        replay
            .invoke(desktop_request("GetRecentProjectsV1", Vec::new()))
            .unwrap()["projects"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    replay.begin_shutdown().unwrap();
    drop(replay);

    let nested = project.join("src/nested");
    fs::create_dir_all(&nested).unwrap();
    let auto_opened = production_desktop_runtime(home.clone(), "test", &nested, None, 0).unwrap();
    assert_eq!(auto_opened.workspace_state().status, WorkspaceStatus::Open);
    assert_eq!(auto_opened.workspace_state().generation, 1);
    assert_eq!(
        auto_opened.workspace_state().project.unwrap().root,
        project.to_str().unwrap()
    );
    let auto_board = auto_opened
        .invoke(desktop_request(
            "GetBoardV2",
            vec![serde_json::json!(1), serde_json::json!(1)],
        ))
        .unwrap();
    assert_eq!(auto_board["board"]["goal"], "Ship the production smoke");
    assert_eq!(
        auto_board["board"]["columns"][1]["tasks"][0]["status"],
        "doing"
    );
    let auto_recents = auto_opened
        .invoke(desktop_request("GetRecentProjectsV1", Vec::new()))
        .unwrap();
    let mut mismatched = initialization_request.clone();
    mismatched.arguments[0]["goal"] = serde_json::json!("Different goal");
    assert_eq!(
        auto_opened.invoke(mismatched).unwrap_err().to_string(),
        "initialization operation goal does not match its durable request"
    );
    assert_eq!(
        auto_opened.invoke(initialization_request.clone()).unwrap(),
        initialized
    );
    assert_eq!(auto_opened.workspace_state().generation, 1);
    assert_eq!(
        auto_opened
            .invoke(desktop_request("GetRecentProjectsV1", Vec::new()))
            .unwrap(),
        auto_recents
    );
    assert_eq!(
        auto_opened
            .invoke(desktop_request(
                "GetBoardV2",
                vec![serde_json::json!(1), serde_json::json!(1)],
            ))
            .unwrap(),
        auto_board
    );

    auto_opened
        .invoke(desktop_request("CloseProject", vec![serde_json::json!("")]))
        .unwrap();
    let reopened_same_root = auto_opened
        .invoke(desktop_request(
            "OpenProject",
            vec![serde_json::json!(project), serde_json::json!("")],
        ))
        .unwrap();
    assert_eq!(reopened_same_root["state"]["generation"], 2);
    let reopened_board = auto_opened
        .invoke(desktop_request(
            "GetBoardV2",
            vec![serde_json::json!(2), serde_json::json!(1)],
        ))
        .unwrap();
    let reopened_recents = auto_opened
        .invoke(desktop_request("GetRecentProjectsV1", Vec::new()))
        .unwrap();
    let reopened_replay = auto_opened.invoke(initialization_request).unwrap();
    assert_eq!(
        reopened_replay["initialization"],
        initialized["initialization"]
    );
    assert_eq!(reopened_replay["state"], reopened_same_root["state"]);
    assert_eq!(auto_opened.workspace_state().generation, 2);
    assert_eq!(
        auto_opened
            .invoke(desktop_request("GetRecentProjectsV1", Vec::new()))
            .unwrap(),
        reopened_recents
    );
    assert_eq!(
        auto_opened
            .invoke(desktop_request(
                "GetBoardV2",
                vec![serde_json::json!(2), serde_json::json!(1)],
            ))
            .unwrap(),
        reopened_board
    );
    auto_opened.begin_shutdown().unwrap();
    drop(auto_opened);

    let reopened = production_desktop_runtime(home.clone(), "test", &temp.0, None, 0).unwrap();
    assert_eq!(reopened.workspace_state().status, WorkspaceStatus::Welcome);
    let existing = reopened
        .invoke(desktop_request(
            "ValidateProjectTargetV1",
            vec![serde_json::json!(nested)],
        ))
        .unwrap();
    assert_eq!(existing["kind"], "existing");
    assert_eq!(existing["canonicalRoot"], project.to_str().unwrap());
    let opened = reopened
        .invoke(desktop_request(
            "OpenProject",
            vec![existing["canonicalRoot"].clone(), serde_json::json!("")],
        ))
        .unwrap();
    assert_eq!(opened["requiresConfirmation"], false);
    assert_eq!(opened["state"]["status"], "open");
    assert_eq!(opened["state"]["generation"], 1);
    let board = reopened
        .invoke(desktop_request(
            "GetBoardV2",
            vec![serde_json::json!(1), serde_json::json!(1)],
        ))
        .unwrap();
    assert_eq!(board["board"]["goal"], "Ship the production smoke");
    assert!(
        board["board"]["columns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|column| {
                column["status"] == "doing"
                    && column["tasks"].as_array().unwrap().iter().any(|task| {
                        task["id"] == 1
                            && task["title"] == "First task"
                            && task["status"] == "doing"
                    })
            })
    );
    reopened.begin_shutdown().unwrap();
}

#[test]
#[allow(clippy::too_many_lines)] // One exact request crosses two injected restart boundaries.
fn production_desktop_json_smoke_retries_no_write_then_recovers_durable_bootstrap() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let desktop = production_desktop_runtime(home.clone(), "test", &temp.0, None, 0).unwrap();
    let validation = desktop
        .invoke(desktop_request(
            "ValidateProjectTargetV1",
            vec![serde_json::json!(project)],
        ))
        .unwrap();
    let operation_id = validation["operationId"].as_str().unwrap().to_owned();
    let initialize = desktop_request(
        "InitializeProjectV1",
        vec![serde_json::json!({
            "operationId": operation_id,
            "root": validation["canonicalRoot"],
            "goal": "Recover the production smoke",
            "guideChoice": "skip",
            "guidePreviewToken": ""
        })],
    );
    crate::production::set_initialization_after_started_hook(|| {
        panic!("simulated no-write interruption");
    });
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = desktop.invoke(initialize.clone());
        }))
        .is_err()
    );
    drop(desktop);
    assert!(!project.join(".ptrack").exists());

    let retry = production_desktop_runtime(home.clone(), "test", &temp.0, None, 0).unwrap();
    assert_eq!(
        retry
            .invoke(desktop_request("GetPendingInitializationV1", Vec::new()))
            .unwrap(),
        serde_json::json!({ "pending": false })
    );
    let ready = retry
        .invoke(desktop_request(
            "GetInitializationStatusV1",
            vec![serde_json::json!(operation_id)],
        ))
        .unwrap();
    assert_eq!(ready["checkpoint"], "none");
    assert_eq!(ready["outcome"], "ready");
    assert_eq!(ready["errorKind"], "interrupted-before-commit");
    assert!(!project.join(".ptrack").exists());
    let resumable = retry
        .invoke(desktop_request(
            "ValidateProjectTargetV1",
            vec![serde_json::json!(project)],
        ))
        .unwrap();
    assert_eq!(resumable["kind"], "new");
    assert_eq!(resumable["canonicalRoot"], project.to_str().unwrap());
    assert_eq!(resumable["operationId"], operation_id);
    assert_eq!(resumable["initialization"], ready);
    assert_eq!(resumable["goal"], "Recover the production smoke");
    assert_eq!(resumable["guideChoice"], "skip");

    crate::production::set_initialization_after_bootstrap_plan_hook(|| {
        panic!("simulated durable bootstrap interruption");
    });
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = retry.invoke(initialize.clone());
        }))
        .is_err()
    );
    drop(retry);
    assert!(home.join("runtime/bootstrap.json").is_file());
    assert!(project.join(".ptrack").is_dir());
    assert!(!project.join(".ptrack/ptrack.redb").exists());

    let recovered = production_desktop_runtime(home.clone(), "test", &temp.0, None, 0).unwrap();
    let pending = recovered
        .invoke(desktop_request("GetPendingInitializationV1", Vec::new()))
        .unwrap();
    assert_eq!(pending["pending"], true);
    assert_eq!(pending["initialization"]["operationId"], operation_id);
    assert_eq!(pending["initialization"]["checkpoint"], "prepared");
    assert_eq!(pending["initialization"]["outcome"], "recovery-required");
    assert_eq!(pending["validation"]["kind"], "new");
    assert_eq!(
        pending["validation"]["canonicalRoot"],
        project.to_str().unwrap()
    );
    assert_eq!(pending["validation"]["operationId"], operation_id);
    assert_eq!(
        pending["validation"]["initialization"],
        pending["initialization"]
    );
    assert_eq!(
        pending["validation"]["goal"],
        "Recover the production smoke"
    );
    assert_eq!(pending["validation"]["guideChoice"], "skip");
    let durable = recovered
        .invoke(desktop_request(
            "GetInitializationStatusV1",
            vec![serde_json::json!(operation_id)],
        ))
        .unwrap();
    assert_eq!(durable, pending["initialization"]);
    let completed = recovered.invoke(initialize).unwrap();
    assert_eq!(completed["initialization"]["checkpoint"], "desktop-bound");
    assert_eq!(completed["initialization"]["outcome"], "complete");
    assert_eq!(completed["state"]["status"], "open");
    assert_eq!(completed["state"]["generation"], 1);
    assert!(project.join(".ptrack/ptrack.redb").is_file());
    assert!(!home.join("runtime/bootstrap.json").exists());
    let completed_status = recovered
        .invoke(desktop_request(
            "GetInitializationStatusV1",
            vec![serde_json::json!(operation_id)],
        ))
        .unwrap();
    assert_eq!(completed_status, completed["initialization"]);
    recovered.begin_shutdown().unwrap();
    drop(recovered);

    let fresh = production_desktop_runtime(home, "test", &temp.0, None, 0).unwrap();
    assert_eq!(fresh.workspace_state().status, WorkspaceStatus::Welcome);
    assert_eq!(
        fresh
            .invoke(desktop_request("GetPendingInitializationV1", Vec::new()))
            .unwrap(),
        serde_json::json!({ "pending": false })
    );
    assert_eq!(fresh.workspace_state().status, WorkspaceStatus::Open);
    assert_eq!(fresh.workspace_state().generation, 1);
    fresh.begin_shutdown().unwrap();
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
    let recents = ProductionRecentProjects::new(runtime.clone());
    assert_eq!(recents.recent_projects().unwrap()[0]["available"], true);
    assert_eq!(
        recents.recent_projects_v1().unwrap().projects[0].availability,
        RecentProjectAvailabilityV1::Available
    );

    let missing = temp.0.join("unmapped-missing");
    let bindings = runtime.global_bindings(runtime.global_home()).unwrap();
    GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding)
        .unwrap()
        .register_project("missing", &missing)
        .unwrap();
    let missing_row = recents
        .recent_projects_v1()
        .unwrap()
        .projects
        .into_iter()
        .find(|row| row.canonical_path == missing.to_string_lossy())
        .unwrap();
    assert_eq!(
        missing_row.availability,
        RecentProjectAvailabilityV1::Missing
    );

    fs::remove_file(project.join(".ptrack/ptrack.redb")).unwrap();
    assert!(
        recents
            .recent_projects()
            .unwrap()
            .iter()
            .any(|row| row["path"] == project.to_string_lossy().as_ref()
                && row["available"] == false)
    );
    assert_eq!(
        recents
            .recent_projects_v1()
            .unwrap()
            .projects
            .into_iter()
            .find(|row| row.canonical_path == project.to_string_lossy())
            .unwrap()
            .availability,
        RecentProjectAvailabilityV1::Changed
    );
    fs::remove_dir_all(&project).unwrap();
    assert_eq!(
        recents
            .recent_projects_v1()
            .unwrap()
            .projects
            .into_iter()
            .find(|row| row.canonical_path == project.to_string_lossy())
            .unwrap()
            .availability,
        RecentProjectAvailabilityV1::Missing
    );
}

#[test]
fn production_forget_recent_removes_only_the_registry_row() {
    let temp = Temp::new();
    let home = temp.0.join("forget-home");
    let project = temp.0.join("forget-project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    private_directory(&home);
    let sentinel = project.join("sentinel");
    fs::write(&sentinel, b"keep").unwrap();

    let mut application = RoutedApplication::new(home, project.clone(), "test");
    application
        .initialize(InitRequest {
            root: Some(project),
            goal: String::new(),
            force: false,
            no_guide: true,
        })
        .unwrap();
    let runtime = application.active_runtime().unwrap().unwrap();
    let recents = ProductionRecentProjects::new(runtime.clone());
    let row = recents.recent_projects_v1().unwrap().projects.remove(0);
    let bindings = runtime.global_bindings(runtime.global_home()).unwrap();
    let registry =
        GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding).unwrap();
    for index in 0..101 {
        registry
            .register_project(
                format!("later-{index}"),
                temp.0.join(format!("later-{index}")),
            )
            .unwrap();
    }
    assert!(
        registry
            .recent_projects(100)
            .unwrap()
            .iter()
            .all(|project| project.path != row.canonical_path)
    );
    let forgotten = recents
        .forget_recent_project(&row.entry_id, &row.base)
        .unwrap();
    assert!(forgotten.forgotten);
    assert!(registry.project(&row.canonical_path).unwrap().is_none());
    assert!(sentinel.exists());
    assert!(
        recents
            .forget_recent_project(&row.entry_id, &row.base)
            .unwrap()
            .forgotten
    );
}

#[test]
fn production_recent_resolution_uses_descendant_open_semantics_and_exact_confirmation() {
    let temp = Temp::new();
    let home = temp.0.join("resolve-home");
    let first = temp.0.join("resolve-first");
    let second = temp.0.join("resolve-second");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&first).unwrap();
    fs::create_dir_all(second.join("child")).unwrap();
    private_directory(&home);

    for root in [&first, &second] {
        let mut application = RoutedApplication::new(home.clone(), first.clone(), "test");
        application
            .initialize(InitRequest {
                root: Some(root.clone()),
                goal: String::new(),
                force: false,
                no_guide: true,
            })
            .unwrap();
    }
    let mut application = RoutedApplication::new(home.clone(), first.clone(), "test");
    let recents = ProductionRecentProjects::new(application.active_runtime().unwrap().unwrap());
    let listed = recents.recent_projects_v1().unwrap();
    let source = listed
        .projects
        .iter()
        .find(|project| project.canonical_path == first.to_string_lossy())
        .unwrap();
    let resolved = recents
        .resolve_recent_project(&source.entry_id, &source.base, &second.join("child"))
        .unwrap();
    assert_eq!(resolved.canonical_root, second.to_string_lossy());
    assert_eq!(
        resolved.resolution,
        crate::RecentProjectResolutionV1::ConfirmationRequired
    );
    assert!(!resolved.confirmation_token.is_empty());
    assert!(
        recents
            .authorize_recent_project_open(&source.entry_id, &source.base, &second, "")
            .is_err()
    );
    assert!(
        recents
            .authorize_recent_project_open(
                &source.entry_id,
                &source.base,
                &second,
                &resolved.confirmation_token,
            )
            .is_ok()
    );
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
        version: "2",
        operation_id: None,
        previous_marker: Some(previous),
        target_marker: target,
        project_root: inner.to_string_lossy().into_owned(),
        project_root_identity: PinnedProjectDirectory::identify_root(&inner).unwrap(),
        project_directory_identity: PinnedProjectDirectory::identify_directory(&inner).unwrap(),
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
fn prepared_bootstrap_rejects_replacement_root_before_creating_child_storage() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    let held = temp.0.join("held-project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    private_directory(&home);
    let project = project.canonicalize().unwrap();
    let pinned = PinnedProjectDirectory::prepare(&project).unwrap();
    let root_identity = pinned.root_identity();
    let project_directory_identity = pinned.directory_identity();
    drop(pinned);

    let lease = acquire_cutover_lock(&home, CutoverLockMode::Exclusive).unwrap();
    let project_path = project.join(".ptrack/ptrack.redb");
    let target = ActiveGeneration::new(
        7,
        "prepared-global".to_owned(),
        &home.join("global.redb"),
        vec![ActiveGenerationProject {
            root: project.to_string_lossy().into_owned(),
            database_id: "prepared-project".to_owned(),
            path: project_path.to_string_lossy().into_owned(),
        }],
    )
    .unwrap();
    let plan = TestBootstrapPlan {
        format: "ptrack-bootstrap-plan",
        version: "2",
        operation_id: None,
        previous_marker: None,
        target_marker: target,
        project_root: project.to_string_lossy().into_owned(),
        project_root_identity: root_identity,
        project_directory_identity,
    };
    let plan_path = home.join("runtime/bootstrap.json");
    let mut bytes = serde_json::to_vec(&plan).unwrap();
    bytes.push(b'\n');
    fs::write(&plan_path, bytes).unwrap();
    private_file(&plan_path);
    drop(lease);

    fs::rename(&project, &held).unwrap();
    fs::create_dir(&project).unwrap();
    let mut application = RoutedApplication::new(home, project.clone(), "test");
    assert!(
        application
            .initialize(InitRequest {
                root: Some(project.clone()),
                goal: "replacement".to_owned(),
                force: false,
                no_guide: true,
            })
            .unwrap_err()
            .to_string()
            .contains("recovery")
    );
    assert!(!project.join(".ptrack").exists());
    assert!(!project_path.exists());
}

#[test]
fn prepared_bootstrap_rejects_replacement_project_directory_without_creating_database() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    private_directory(&home);
    let project = project.canonicalize().unwrap();
    let pinned = PinnedProjectDirectory::prepare(&project).unwrap();
    let root_identity = pinned.root_identity();
    let project_directory_identity = pinned.directory_identity();
    drop(pinned);

    let lease = acquire_cutover_lock(&home, CutoverLockMode::Exclusive).unwrap();
    let project_path = project.join(".ptrack/ptrack.redb");
    let target = ActiveGeneration::new(
        8,
        "prepared-global-child".to_owned(),
        &home.join("global.redb"),
        vec![ActiveGenerationProject {
            root: project.to_string_lossy().into_owned(),
            database_id: "prepared-project-child".to_owned(),
            path: project_path.to_string_lossy().into_owned(),
        }],
    )
    .unwrap();
    let plan = TestBootstrapPlan {
        format: "ptrack-bootstrap-plan",
        version: "2",
        operation_id: None,
        previous_marker: None,
        target_marker: target,
        project_root: project.to_string_lossy().into_owned(),
        project_root_identity: root_identity,
        project_directory_identity,
    };
    let plan_path = home.join("runtime/bootstrap.json");
    let mut bytes = serde_json::to_vec(&plan).unwrap();
    bytes.push(b'\n');
    fs::write(&plan_path, bytes).unwrap();
    private_file(&plan_path);
    drop(lease);

    fs::rename(project.join(".ptrack"), project.join("held-ptrack")).unwrap();
    fs::create_dir(project.join(".ptrack")).unwrap();
    private_directory(&project.join(".ptrack"));
    let mut application = RoutedApplication::new(home, project.clone(), "test");
    assert!(
        application
            .initialize(InitRequest {
                root: Some(project.clone()),
                goal: "replacement".to_owned(),
                force: false,
                no_guide: true,
            })
            .unwrap_err()
            .to_string()
            .contains("recovery")
    );
    assert!(!project_path.exists());
    assert!(!project.join("held-ptrack/ptrack.redb").exists());
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

#[test]
fn desktop_authority_validation_is_read_only_and_classifies_new_targets() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();

    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();

    assert_eq!(validation.kind, ProjectTargetKindV1::New);
    assert_eq!(Path::new(&validation.canonical_root), project);
    assert_eq!(validation.operation_id.len(), 43);
    assert!(
        validation
            .operation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    );
    assert!(!home.exists());
    assert!(!project.join(".ptrack").exists());
}

#[cfg(unix)]
#[test]
fn desktop_authority_previews_and_applies_only_the_exact_consented_guides() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let preview = authority
        .preview_guide(&ProjectGuidePreviewRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root.clone(),
        })
        .unwrap();

    assert!(preview.available);
    assert!(preview.message.is_empty());
    assert_eq!(preview.preview_token.len(), 43);
    assert_eq!(preview.files.len(), 2);
    assert_eq!(preview.files[0].path, "AGENTS.md");
    assert_eq!(preview.files[1].path, "CLAUDE.md");
    assert!(
        preview
            .files
            .iter()
            .all(|file| file.action == ProjectGuideFileActionV1::Create && !file.diff.is_empty())
    );

    let request = InitializeProjectRequestV1 {
        operation_id: validation.operation_id,
        root: validation.canonical_root,
        goal: "install exact guidance".to_owned(),
        guide_choice: ProjectGuideChoiceV1::Install,
        guide_preview_token: preview.preview_token,
    };
    let applied = authority.initialize(&request).unwrap();
    assert_eq!(applied.checkpoint, InitializationCheckpointV1::GuideApplied);
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let content = fs::read_to_string(project.join(name)).unwrap();
        assert!(content.contains("<!-- ptrack:begin -->"));
        assert!(content.contains("<!-- ptrack:end -->"));
    }
    let agents = fs::read(project.join("AGENTS.md")).unwrap();
    assert_eq!(authority.initialize(&request).unwrap(), applied);
    assert_eq!(fs::read(project.join("AGENTS.md")).unwrap(), agents);
    drop(authority);
    let restarted = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    assert_eq!(restarted.initialize(&request).unwrap(), applied);
    assert_eq!(fs::read(project.join("AGENTS.md")).unwrap(), agents);
}

#[test]
fn desktop_authority_resumes_skip_from_project_committed_without_a_bootstrap_plan() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let request = InitializeProjectRequestV1 {
        operation_id: validation.operation_id,
        root: validation.canonical_root,
        goal: "resume skipped guide".to_owned(),
        guide_choice: ProjectGuideChoiceV1::Skip,
        guide_preview_token: String::new(),
    };
    authority.initialize(&request).unwrap();
    drop(authority);

    let journal = home.join("runtime/desktop-initialization.json");
    let content = fs::read_to_string(&journal).unwrap();
    let interrupted = content.replace(
        "\"checkpoint\":\"guide-applied\",\"outcome\":\"in-progress\"",
        "\"checkpoint\":\"project-committed\",\"outcome\":\"in-progress\"",
    );
    assert_ne!(interrupted, content);
    fs::write(&journal, interrupted).unwrap();
    private_file(&journal);
    assert!(!home.join("runtime/bootstrap.json").exists());

    let restarted = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let pending = restarted.pending().unwrap();
    assert!(pending.pending);
    assert_eq!(
        pending.initialization.as_ref().unwrap().checkpoint,
        InitializationCheckpointV1::ProjectCommitted
    );
    assert_eq!(
        pending
            .validation
            .as_ref()
            .and_then(|validation| validation.initialization.as_ref())
            .map(|status| status.checkpoint),
        Some(InitializationCheckpointV1::ProjectCommitted)
    );
    let resumed_validation = restarted.validate_target(&project).unwrap();
    assert_eq!(
        resumed_validation
            .initialization
            .as_ref()
            .unwrap()
            .checkpoint,
        InitializationCheckpointV1::ProjectCommitted
    );
    assert_eq!(
        resumed_validation.goal.as_deref(),
        Some("resume skipped guide")
    );
    assert_eq!(
        resumed_validation.guide_choice,
        Some(ProjectGuideChoiceV1::Skip)
    );
    let resumed = restarted.initialize(&request).unwrap();
    assert_eq!(resumed.checkpoint, InitializationCheckpointV1::GuideApplied);
    assert!(!project.join("AGENTS.md").exists());
    assert!(!project.join("CLAUDE.md").exists());
}

#[test]
fn pending_initialization_preserves_checkpoint_when_target_vanishes() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    let moved = temp.0.join("moved-project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let request = InitializeProjectRequestV1 {
        operation_id: validation.operation_id,
        root: validation.canonical_root,
        goal: "recover a vanished target".to_owned(),
        guide_choice: ProjectGuideChoiceV1::Skip,
        guide_preview_token: String::new(),
    };
    let applied = authority.initialize(&request).unwrap();
    assert_eq!(applied.checkpoint, InitializationCheckpointV1::GuideApplied);
    fs::rename(&project, moved).unwrap();

    let pending = authority.pending().unwrap();

    assert!(pending.pending);
    let initialization = pending.initialization.unwrap();
    assert_eq!(initialization.operation_id, request.operation_id);
    assert_eq!(
        initialization.checkpoint,
        InitializationCheckpointV1::GuideApplied
    );
    let current = pending.validation.unwrap();
    assert_eq!(current.kind, ProjectTargetKindV1::RecoveryRequired);
    assert_eq!(current.canonical_root, request.root);
    assert!(current.operation_id.is_empty());
    assert!(current.initialization.is_none());
    assert_eq!(
        current.reason,
        "the interrupted initialization target is unavailable"
    );
}

#[cfg(unix)]
#[test]
fn desktop_authority_rejects_skip_after_a_partially_applied_guide() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let preview = authority
        .preview_guide(&ProjectGuidePreviewRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root.clone(),
        })
        .unwrap();
    let install = InitializeProjectRequestV1 {
        operation_id: validation.operation_id.clone(),
        root: validation.canonical_root.clone(),
        goal: "preserve partial guidance".to_owned(),
        guide_choice: ProjectGuideChoiceV1::Install,
        guide_preview_token: preview.preview_token,
    };
    authority.initialize(&install).unwrap();
    drop(authority);

    fs::write(project.join("CLAUDE.md"), "changed after first publish\n").unwrap();
    let journal = home.join("runtime/desktop-initialization.json");
    let content = fs::read_to_string(&journal).unwrap();
    let interrupted = content.replace(
        "\"checkpoint\":\"guide-applied\",\"outcome\":\"in-progress\"",
        "\"checkpoint\":\"project-committed\",\"outcome\":\"recovery-required\",\"errorKind\":\"project-guide-preview-stale\"",
    );
    assert_ne!(interrupted, content);
    fs::write(&journal, interrupted).unwrap();
    private_file(&journal);

    let restarted = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let skip = InitializeProjectRequestV1 {
        operation_id: validation.operation_id,
        root: validation.canonical_root,
        goal: install.goal,
        guide_choice: ProjectGuideChoiceV1::Skip,
        guide_preview_token: String::new(),
    };
    assert_eq!(
        restarted.initialize(&skip).unwrap_err().to_string(),
        "project-guide-partially-applied"
    );
    assert_eq!(
        restarted.status(&skip.operation_id).unwrap().error_kind,
        "project-guide-partially-applied"
    );
    let refreshed = restarted
        .preview_guide(&ProjectGuidePreviewRequestV1 {
            operation_id: skip.operation_id.clone(),
            root: skip.root.clone(),
        })
        .unwrap();
    assert_eq!(
        restarted
            .initialize(&InitializeProjectRequestV1 {
                operation_id: skip.operation_id,
                root: skip.root,
                goal: skip.goal,
                guide_choice: ProjectGuideChoiceV1::Install,
                guide_preview_token: refreshed.preview_token,
            })
            .unwrap()
            .checkpoint,
        InitializationCheckpointV1::GuideApplied
    );
}

#[cfg(unix)]
#[test]
fn desktop_authority_records_partial_apply_when_a_later_guide_turns_stale() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let preview = authority
        .preview_guide(&ProjectGuidePreviewRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root.clone(),
        })
        .unwrap();
    let claude = project.join("CLAUDE.md");
    let claude_for_hook = claude.clone();
    crate::production::set_guide_before_publish_hook(move || {
        fs::write(claude_for_hook, "concurrent user edit\n").unwrap();
    });
    let request = InitializeProjectRequestV1 {
        operation_id: validation.operation_id.clone(),
        root: validation.canonical_root,
        goal: "report partial guidance immediately".to_owned(),
        guide_choice: ProjectGuideChoiceV1::Install,
        guide_preview_token: preview.preview_token,
    };

    assert_eq!(
        authority.initialize(&request).unwrap_err().to_string(),
        "project-guide-partially-applied"
    );
    assert!(
        fs::read_to_string(project.join("AGENTS.md"))
            .unwrap()
            .contains("<!-- ptrack:begin -->")
    );
    assert_eq!(
        fs::read_to_string(claude).unwrap(),
        "concurrent user edit\n"
    );
    let status = authority.status(&validation.operation_id).unwrap();
    assert_eq!(
        status.checkpoint,
        InitializationCheckpointV1::ProjectCommitted
    );
    assert_eq!(status.outcome, InitializationOutcomeV1::RecoveryRequired);
    assert_eq!(status.error_kind, "project-guide-partially-applied");
}

#[cfg(unix)]
#[test]
#[allow(clippy::too_many_lines)] // Two authorities prove journal status and consent reconcile together.
fn desktop_authority_loser_reconciles_the_winning_guide_manifest() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let seeded = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = seeded.validate_target(&project).unwrap();
    let preview = seeded
        .preview_guide(&ProjectGuidePreviewRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root.clone(),
        })
        .unwrap();
    let goal = "reconcile concurrent guide consent".to_owned();
    seeded
        .initialize(&InitializeProjectRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root.clone(),
            goal: goal.clone(),
            guide_choice: ProjectGuideChoiceV1::Install,
            guide_preview_token: preview.preview_token,
        })
        .unwrap();
    drop(seeded);

    fs::write(project.join("CLAUDE.md"), "refresh this base\n").unwrap();
    let journal = home.join("runtime/desktop-initialization.json");
    let content = fs::read_to_string(&journal).unwrap();
    let stale = content.replace(
        "\"checkpoint\":\"guide-applied\",\"outcome\":\"in-progress\"",
        "\"checkpoint\":\"project-committed\",\"outcome\":\"recovery-required\",\"errorKind\":\"project-guide-preview-stale\"",
    );
    assert_ne!(stale, content);
    fs::write(&journal, stale).unwrap();
    private_file(&journal);

    let first = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let second = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let preview_request = ProjectGuidePreviewRequestV1 {
        operation_id: validation.operation_id.clone(),
        root: validation.canonical_root.clone(),
    };
    let first_preview = first.preview_guide(&preview_request).unwrap();
    let second_preview = second.preview_guide(&preview_request).unwrap();
    assert_ne!(first_preview.preview_token, second_preview.preview_token);
    let first_request = InitializeProjectRequestV1 {
        operation_id: validation.operation_id.clone(),
        root: validation.canonical_root.clone(),
        goal: goal.clone(),
        guide_choice: ProjectGuideChoiceV1::Install,
        guide_preview_token: first_preview.preview_token,
    };
    let second_request = InitializeProjectRequestV1 {
        guide_preview_token: second_preview.preview_token,
        ..first_request.clone()
    };
    let barrier = Arc::new(Barrier::new(3));
    let commit_barrier = Arc::new(Barrier::new(2));
    let first_thread = {
        let authority = Arc::clone(&first);
        let request = first_request.clone();
        let barrier = Arc::clone(&barrier);
        let commit_barrier = Arc::clone(&commit_barrier);
        std::thread::spawn(move || {
            crate::production::set_initialization_before_commit_hook(move || {
                commit_barrier.wait();
            });
            barrier.wait();
            authority
                .initialize(&request)
                .map(|status| status.checkpoint)
                .map_err(|error| error.to_string())
        })
    };
    let second_thread = {
        let authority = Arc::clone(&second);
        let request = second_request.clone();
        let barrier = Arc::clone(&barrier);
        let commit_barrier = Arc::clone(&commit_barrier);
        std::thread::spawn(move || {
            crate::production::set_initialization_before_commit_hook(move || {
                commit_barrier.wait();
            });
            barrier.wait();
            authority
                .initialize(&request)
                .map(|status| status.checkpoint)
                .map_err(|error| error.to_string())
        })
    };
    barrier.wait();
    let first_result = first_thread.join().unwrap();
    let second_result = second_thread.join().unwrap();
    let (loser, loser_request) = match (&first_result, &second_result) {
        (Ok(InitializationCheckpointV1::GuideApplied), Err(_)) => {
            (Arc::clone(&second), second_request)
        }
        (Err(_), Ok(InitializationCheckpointV1::GuideApplied)) => {
            (Arc::clone(&first), first_request)
        }
        _ => panic!("expected one winner and one loser: {first_result:?} {second_result:?}"),
    };
    drop(first);
    drop(second);

    assert_eq!(
        loser.initialize(&loser_request).unwrap().checkpoint,
        InitializationCheckpointV1::GuideApplied
    );
    assert_eq!(
        loser
            .mark_desktop_bound(&validation.operation_id)
            .unwrap()
            .checkpoint,
        InitializationCheckpointV1::DesktopBound
    );
}

#[cfg(unix)]
#[test]
fn desktop_authority_stale_guide_preview_is_no_write_and_can_be_refreshed() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let preview = authority
        .preview_guide(&ProjectGuidePreviewRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root.clone(),
        })
        .unwrap();
    fs::write(project.join("AGENTS.md"), "user change\n").unwrap();

    let mut request = InitializeProjectRequestV1 {
        operation_id: validation.operation_id.clone(),
        root: validation.canonical_root.clone(),
        goal: "respect stale files".to_owned(),
        guide_choice: ProjectGuideChoiceV1::Install,
        guide_preview_token: preview.preview_token,
    };
    assert_eq!(
        authority.initialize(&request).unwrap_err().to_string(),
        "project-guide-preview-stale"
    );
    assert!(!home.exists());
    assert!(!project.join(".ptrack").exists());
    let status = authority.status(&validation.operation_id).unwrap();
    assert_eq!(status.checkpoint, InitializationCheckpointV1::None);
    assert_eq!(status.outcome, InitializationOutcomeV1::Ready);
    assert_eq!(status.error_kind, "project-guide-preview-stale");

    let refreshed = authority
        .preview_guide(&ProjectGuidePreviewRequestV1 {
            operation_id: validation.operation_id,
            root: validation.canonical_root,
        })
        .unwrap();
    assert_eq!(refreshed.files[0].action, ProjectGuideFileActionV1::Update);
    request.guide_preview_token = refreshed.preview_token;
    assert_eq!(
        authority.initialize(&request).unwrap().checkpoint,
        InitializationCheckpointV1::GuideApplied
    );
    assert!(
        fs::read_to_string(project.join("AGENTS.md"))
            .unwrap()
            .starts_with("user change\n")
    );
}

#[cfg(unix)]
#[test]
fn desktop_authority_second_validation_stale_can_explicitly_skip_without_guide_writes() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let preview = authority
        .preview_guide(&ProjectGuidePreviewRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root.clone(),
        })
        .unwrap();
    let guide = project.join("AGENTS.md");
    let guide_for_hook = guide.clone();
    crate::production::set_guide_before_commit_hook(move || {
        fs::write(guide_for_hook, "user edit before storage\n").unwrap();
    });
    let install = InitializeProjectRequestV1 {
        operation_id: validation.operation_id,
        root: validation.canonical_root,
        goal: "skip stale guidance".to_owned(),
        guide_choice: ProjectGuideChoiceV1::Install,
        guide_preview_token: preview.preview_token,
    };
    assert_eq!(
        authority.initialize(&install).unwrap_err().to_string(),
        "project-guide-preview-stale"
    );
    assert!(!project.join(".ptrack").exists());

    let skipped = authority
        .initialize(&InitializeProjectRequestV1 {
            guide_choice: ProjectGuideChoiceV1::Skip,
            guide_preview_token: String::new(),
            ..install
        })
        .unwrap();
    assert_eq!(skipped.checkpoint, InitializationCheckpointV1::GuideApplied);
    assert_eq!(
        fs::read_to_string(guide).unwrap(),
        "user edit before storage\n"
    );
    assert!(!project.join("CLAUDE.md").exists());
}

#[cfg(unix)]
#[test]
fn desktop_authority_runtime_committed_stale_restart_can_explicitly_skip() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let preview = authority
        .preview_guide(&ProjectGuidePreviewRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root.clone(),
        })
        .unwrap();
    let goal = "resume runtime commit without guide".to_owned();
    authority
        .initialize(&InitializeProjectRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root.clone(),
            goal: goal.clone(),
            guide_choice: ProjectGuideChoiceV1::Install,
            guide_preview_token: preview.preview_token,
        })
        .unwrap();
    drop(authority);
    fs::remove_file(project.join("AGENTS.md")).unwrap();
    fs::remove_file(project.join("CLAUDE.md")).unwrap();

    let journal = home.join("runtime/desktop-initialization.json");
    let content = fs::read_to_string(&journal).unwrap();
    let stale = content.replace(
        "\"checkpoint\":\"guide-applied\",\"outcome\":\"in-progress\"",
        "\"checkpoint\":\"runtime-committed\",\"outcome\":\"recovery-required\",\"errorKind\":\"project-guide-preview-stale\"",
    );
    assert_ne!(stale, content);
    fs::write(&journal, stale).unwrap();
    private_file(&journal);
    assert!(!home.join("runtime/bootstrap.json").exists());

    let restarted = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let resumed = restarted
        .initialize(&InitializeProjectRequestV1 {
            operation_id: validation.operation_id,
            root: validation.canonical_root,
            goal,
            guide_choice: ProjectGuideChoiceV1::Skip,
            guide_preview_token: String::new(),
        })
        .unwrap();
    assert_eq!(resumed.checkpoint, InitializationCheckpointV1::GuideApplied);
    assert!(!project.join("AGENTS.md").exists());
    assert!(!project.join("CLAUDE.md").exists());
}

#[cfg(unix)]
#[test]
fn desktop_authority_guide_applied_restart_ignores_lost_token_and_preserves_later_edits() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let preview = authority
        .preview_guide(&ProjectGuidePreviewRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root.clone(),
        })
        .unwrap();
    let goal = "bind after guide edit".to_owned();
    authority
        .initialize(&InitializeProjectRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root.clone(),
            goal: goal.clone(),
            guide_choice: ProjectGuideChoiceV1::Install,
            guide_preview_token: preview.preview_token,
        })
        .unwrap();
    fs::write(project.join("AGENTS.md"), "later user edit\n").unwrap();
    drop(authority);

    let restarted = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let resumed_validation = restarted.validate_target(&project).unwrap();
    assert_eq!(
        resumed_validation
            .initialization
            .as_ref()
            .unwrap()
            .checkpoint,
        InitializationCheckpointV1::GuideApplied
    );
    assert_eq!(
        resumed_validation.guide_choice,
        Some(ProjectGuideChoiceV1::Install)
    );
    let resumed = restarted
        .initialize(&InitializeProjectRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root,
            goal,
            guide_choice: ProjectGuideChoiceV1::Skip,
            guide_preview_token: String::new(),
        })
        .unwrap();
    assert_eq!(resumed.checkpoint, InitializationCheckpointV1::GuideApplied);
    assert_eq!(
        fs::read_to_string(project.join("AGENTS.md")).unwrap(),
        "later user edit\n"
    );
    assert_eq!(
        restarted
            .mark_desktop_bound(&validation.operation_id)
            .unwrap()
            .checkpoint,
        InitializationCheckpointV1::DesktopBound
    );
}

#[cfg(unix)]
#[test]
fn desktop_authority_rejects_a_guide_preview_over_the_line_bound_without_writes() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("AGENTS.md"), "x\n".repeat(4_097)).unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    assert!(
        authority
            .preview_guide(&ProjectGuidePreviewRequestV1 {
                operation_id: validation.operation_id,
                root: validation.canonical_root,
            })
            .unwrap_err()
            .to_string()
            .contains("line count")
    );
    assert!(!home.exists());
    assert!(!project.join(".ptrack").exists());
}

#[cfg(not(unix))]
#[test]
fn desktop_authority_reports_guide_capability_unavailable_without_writes() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let preview = authority
        .preview_guide(&ProjectGuidePreviewRequestV1 {
            operation_id: validation.operation_id,
            root: validation.canonical_root,
        })
        .unwrap();
    assert!(!preview.available);
    assert_eq!(
        preview.message,
        "Project guidance is not available on this platform yet"
    );
    assert!(preview.preview_token.is_empty());
    assert!(preview.files.is_empty());
    assert!(!home.exists());
}

#[cfg(unix)]
#[test]
fn desktop_authority_rejects_forged_consent_and_symlink_preview_without_writes() {
    use std::os::unix::fs::symlink;

    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    let outside = temp.0.join("outside.md");
    fs::create_dir(&project).unwrap();
    fs::write(&outside, "outside\n").unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();

    assert_eq!(
        authority
            .initialize(&InitializeProjectRequestV1 {
                operation_id: validation.operation_id.clone(),
                root: validation.canonical_root.clone(),
                goal: "reject forged consent".to_owned(),
                guide_choice: ProjectGuideChoiceV1::Install,
                guide_preview_token: "Z".repeat(43),
            })
            .unwrap_err()
            .to_string(),
        "project-guide-preview-stale"
    );
    assert!(!home.exists());
    assert!(!project.join(".ptrack").exists());

    symlink(&outside, project.join("AGENTS.md")).unwrap();
    assert!(
        authority
            .preview_guide(&ProjectGuidePreviewRequestV1 {
                operation_id: validation.operation_id,
                root: validation.canonical_root,
            })
            .is_err()
    );
    assert_eq!(fs::read_to_string(outside).unwrap(), "outside\n");
    assert!(!home.exists());
}

#[cfg(unix)]
#[test]
fn desktop_authority_create_race_fails_stale_and_cleans_its_root_temporary() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let preview = authority
        .preview_guide(&ProjectGuidePreviewRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root.clone(),
        })
        .unwrap();
    let raced = project.join("AGENTS.md");
    let raced_for_hook = raced.clone();
    let project_for_hook = project.clone();
    crate::production::set_guide_before_publish_hook(move || {
        assert!(fs::read_dir(&project_for_hook).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("guide-AGENTS")
        }));
        assert!(
            fs::read_dir(project_for_hook.join(".ptrack"))
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".guide-AGENTS.md-"))
        );
        fs::write(raced_for_hook, "concurrent user file\n").unwrap();
    });

    let error = authority
        .initialize(&InitializeProjectRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root,
            goal: "preserve guide race".to_owned(),
            guide_choice: ProjectGuideChoiceV1::Install,
            guide_preview_token: preview.preview_token,
        })
        .unwrap_err();
    assert_eq!(error.to_string(), "project-guide-preview-stale");
    assert_eq!(fs::read_to_string(raced).unwrap(), "concurrent user file\n");
    assert!(fs::read_dir(&project).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".guide-")
    }));
    let status = authority.status(&validation.operation_id).unwrap();
    assert_eq!(
        status.checkpoint,
        InitializationCheckpointV1::ProjectCommitted
    );
    assert_eq!(status.outcome, InitializationOutcomeV1::RecoveryRequired);
    assert_eq!(status.error_kind, "project-guide-preview-stale");
}

#[test]
fn desktop_authority_rejects_an_initial_empty_project_directory_without_writes() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    fs::create_dir(project.join(".ptrack")).unwrap();
    private_directory(&project.join(".ptrack"));

    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();

    assert_eq!(validation.kind, ProjectTargetKindV1::RecoveryRequired);
    assert!(validation.reason.contains("preexisting project storage"));
    assert!(validation.operation_id.is_empty());
    assert!(!home.exists());
    assert!(!project.join(".ptrack/ptrack.redb").exists());
}

#[test]
fn desktop_authority_rejects_project_directory_appearing_after_validation_without_writes() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    assert_eq!(validation.kind, ProjectTargetKindV1::New);

    fs::create_dir(project.join(".ptrack")).unwrap();
    private_directory(&project.join(".ptrack"));
    assert!(
        authority
            .initialize(&InitializeProjectRequestV1 {
                operation_id: validation.operation_id.clone(),
                root: validation.canonical_root,
                goal: "do not adopt storage".to_owned(),
                guide_choice: ProjectGuideChoiceV1::Skip,
                guide_preview_token: String::new(),
            })
            .is_err()
    );

    assert!(!home.exists());
    assert!(!project.join(".ptrack/ptrack.redb").exists());
    assert_eq!(
        authority.validate_target(&project).unwrap().kind,
        ProjectTargetKindV1::RecoveryRequired
    );
    assert!(authority.status(&validation.operation_id).is_err());
}

#[test]
fn desktop_authority_classifies_existing_descendant_and_recovery_targets() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    let descendant = project.join("src/deep");
    let recovery = temp.0.join("recovery");
    fs::create_dir(&home).unwrap();
    fs::create_dir_all(&descendant).unwrap();
    fs::create_dir_all(recovery.join(".ptrack")).unwrap();
    private_directory(&home);

    let authority = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let ready = authority.validate_target(&project).unwrap();
    let operation_id = ready.operation_id.clone();
    authority
        .initialize(&InitializeProjectRequestV1 {
            operation_id: ready.operation_id,
            root: ready.canonical_root,
            goal: "ship safely".to_owned(),
            guide_choice: ProjectGuideChoiceV1::Skip,
            guide_preview_token: String::new(),
        })
        .unwrap();
    authority.mark_desktop_bound(&operation_id).unwrap();

    let existing = authority.validate_target(&project).unwrap();
    assert_eq!(existing.kind, ProjectTargetKindV1::Existing);
    assert_eq!(Path::new(&existing.canonical_root), project);
    assert!(existing.operation_id.is_empty());

    let nested = authority.validate_target(&descendant).unwrap();
    assert_eq!(nested.kind, ProjectTargetKindV1::Existing);
    assert_eq!(Path::new(&nested.canonical_root), project);

    fs::write(recovery.join(".ptrack/ptrack.redb"), b"unregistered").unwrap();
    let unsafe_target = authority.validate_target(&recovery).unwrap();
    assert_eq!(unsafe_target.kind, ProjectTargetKindV1::RecoveryRequired);
    assert!(unsafe_target.reason.contains("unregistered project store"));
    assert!(unsafe_target.operation_id.is_empty());
}

#[test]
#[allow(clippy::too_many_lines)] // One lifecycle proves commit, exact resume, and final binding.
fn desktop_authority_true_first_run_initializes_replays_and_opens_without_guide() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    let other_project = temp.0.join("other-project");
    fs::create_dir(&project).unwrap();
    fs::create_dir(&other_project).unwrap();

    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let request = InitializeProjectRequestV1 {
        operation_id: validation.operation_id.clone(),
        root: validation.canonical_root,
        goal: "first-run goal".to_owned(),
        guide_choice: ProjectGuideChoiceV1::Skip,
        guide_preview_token: String::new(),
    };
    let committed = authority.initialize(&request).unwrap();
    assert_eq!(committed.operation_id, validation.operation_id);
    assert_eq!(
        committed.checkpoint,
        InitializationCheckpointV1::GuideApplied
    );
    assert_eq!(committed.outcome, InitializationOutcomeV1::InProgress);
    assert!(authority.initial_workspace_runtime().is_none());
    assert!(home.join("global.redb").exists());
    assert!(project.join(".ptrack/ptrack.redb").exists());
    assert!(!project.join("AGENTS.md").exists());
    assert!(!project.join("CLAUDE.md").exists());

    assert_eq!(
        authority.status(&validation.operation_id).unwrap(),
        committed
    );
    assert_eq!(
        authority.validate_target(&other_project).unwrap().kind,
        ProjectTargetKindV1::RecoveryRequired
    );
    let marker = authority.active_runtime().unwrap().marker().clone();
    drop(authority);
    let stale_plan = TestBootstrapPlan {
        format: "ptrack-bootstrap-plan",
        version: "2",
        operation_id: Some(validation.operation_id.clone()),
        previous_marker: None,
        target_marker: marker,
        project_root: project.to_string_lossy().into_owned(),
        project_root_identity: PinnedProjectDirectory::identify_root(&project).unwrap(),
        project_directory_identity: PinnedProjectDirectory::identify_directory(&project).unwrap(),
    };
    let plan_path = home.join("runtime/bootstrap.json");
    let mut mismatched_plan = stale_plan.clone();
    mismatched_plan.operation_id = Some("m".repeat(43));
    write_test_bootstrap_plan(&plan_path, &mismatched_plan);
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let mismatched = authority.validate_target(&project).unwrap();
    assert_eq!(mismatched.kind, ProjectTargetKindV1::RecoveryRequired);
    assert!(
        mismatched
            .reason
            .contains("not bound to this initialization")
    );
    drop(authority);

    write_test_bootstrap_plan(&plan_path, &stale_plan);

    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    assert_eq!(
        authority.status(&validation.operation_id).unwrap(),
        committed
    );
    let resumed = authority.validate_target(&project).unwrap();
    assert_eq!(resumed.kind, ProjectTargetKindV1::New);
    assert_eq!(resumed.operation_id, validation.operation_id);
    let mut changed_request = request.clone();
    changed_request.goal = "a different goal".to_owned();
    assert!(
        authority
            .initialize(&changed_request)
            .unwrap_err()
            .to_string()
            .contains("does not match its durable request")
    );
    assert_eq!(authority.initialize(&request).unwrap(), committed);
    assert!(!plan_path.exists());
    let bound = authority
        .mark_desktop_bound(&validation.operation_id)
        .unwrap();
    assert_eq!(bound.checkpoint, InitializationCheckpointV1::DesktopBound);
    assert_eq!(bound.outcome, InitializationOutcomeV1::Complete);
    let pending = authority.pending().unwrap();
    assert!(!pending.pending);
    assert!(pending.initialization.is_none());
    assert!(pending.validation.is_none());
    assert!(authority.initial_workspace_runtime().is_some());
    assert_eq!(authority.initialize(&request).unwrap(), bound);
    assert_eq!(authority.status(&validation.operation_id).unwrap(), bound);
    assert_eq!(
        authority
            .mark_desktop_bound(&validation.operation_id)
            .unwrap(),
        bound
    );

    let workspace = authority.build(&project, 1).unwrap();
    assert_eq!(Path::new(&workspace.project().root), project);
    workspace.shutdown().unwrap();

    drop(authority);
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    assert_eq!(authority.status(&validation.operation_id).unwrap(), bound);
    assert_eq!(
        authority.validate_target(&project).unwrap().kind,
        ProjectTargetKindV1::Existing
    );

    let mut application = RoutedApplication::new(home, project, "test");
    assert_eq!(application.snapshot().unwrap().meta.goal, "first-run goal");
}

#[test]
fn desktop_authority_rejects_an_impossible_durable_status() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();

    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    authority
        .initialize(&InitializeProjectRequestV1 {
            operation_id: validation.operation_id,
            root: validation.canonical_root,
            goal: "durable status".to_owned(),
            guide_choice: ProjectGuideChoiceV1::Skip,
            guide_preview_token: String::new(),
        })
        .unwrap();
    drop(authority);

    let journal = home.join("runtime/desktop-initialization.json");
    let content = fs::read_to_string(&journal).unwrap();
    let invalid = content.replace(
        "\"checkpoint\":\"guide-applied\",\"outcome\":\"in-progress\"",
        "\"checkpoint\":\"none\",\"outcome\":\"complete\"",
    );
    assert_ne!(invalid, content);
    fs::write(&journal, invalid).unwrap();
    private_file(&journal);

    assert!(
        ProductionDesktopAuthority::load(home, "test", None, None, 0)
            .err()
            .unwrap()
            .to_string()
            .contains("status fields are invalid")
    );
}

#[test]
fn desktop_authority_lock_contention_is_a_safe_no_write_failure() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let existing_project = temp.0.join("existing");
    let new_project = temp.0.join("new");
    fs::create_dir(&existing_project).unwrap();
    fs::create_dir(&new_project).unwrap();
    let mut application = RoutedApplication::new(home.clone(), existing_project.clone(), "test");
    application
        .initialize(InitRequest {
            root: Some(existing_project),
            goal: "existing".to_owned(),
            force: false,
            no_guide: true,
        })
        .unwrap();
    drop(application);

    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&new_project).unwrap();
    let external_runtime = ActiveRuntime::load(&home, "test").unwrap().unwrap();
    assert!(
        authority
            .initialize(&InitializeProjectRequestV1 {
                operation_id: validation.operation_id.clone(),
                root: validation.canonical_root,
                goal: "new project".to_owned(),
                guide_choice: ProjectGuideChoiceV1::Skip,
                guide_preview_token: String::new(),
            })
            .is_err()
    );
    let status = authority.status(&validation.operation_id).unwrap();
    assert_eq!(status.checkpoint, InitializationCheckpointV1::None);
    assert_eq!(status.outcome, InitializationOutcomeV1::Ready);
    assert!(!new_project.join(".ptrack").exists());
    assert!(!home.join("runtime/desktop-initialization.json").exists());
    drop(external_runtime);
}

#[cfg(unix)]
#[test]
fn interrupted_empty_project_directory_restarts_as_recovery_required() {
    #[derive(serde::Serialize)]
    struct Journal<'a> {
        format: &'static str,
        version: &'static str,
        status: &'a InitializationStatusV1,
        goal: &'static str,
    }

    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    let runtime = home.join("runtime");
    fs::create_dir(&project).unwrap();
    fs::create_dir(&home).unwrap();
    fs::create_dir(&runtime).unwrap();
    fs::create_dir(project.join(".ptrack")).unwrap();
    private_directory(&home);
    private_directory(&runtime);
    private_directory(&project.join(".ptrack"));
    let operation_id = "i".repeat(43);
    let started = InitializationStatusV1 {
        operation_id: operation_id.clone(),
        canonical_root: project.to_string_lossy().into_owned(),
        checkpoint: InitializationCheckpointV1::None,
        outcome: InitializationOutcomeV1::InProgress,
        error_kind: String::new(),
    };
    let mut bytes = serde_json::to_vec(&Journal {
        format: "ptrack-desktop-initialization",
        version: "1",
        status: &started,
        goal: "recover interrupted storage",
    })
    .unwrap();
    bytes.push(b'\n');
    let journal = runtime.join("desktop-initialization.json");
    fs::write(&journal, bytes).unwrap();
    private_file(&journal);
    assert!(!project.join(".ptrack/ptrack.redb").exists());

    let authority = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let restarted = authority.status(&operation_id).unwrap();
    assert_eq!(restarted.checkpoint, InitializationCheckpointV1::Prepared);
    assert_eq!(restarted.outcome, InitializationOutcomeV1::RecoveryRequired);
    assert_eq!(
        authority.validate_target(&project).unwrap().kind,
        ProjectTargetKindV1::RecoveryRequired
    );
}

#[test]
fn interrupted_before_prepared_restarts_as_ready_without_project_writes() {
    #[derive(serde::Serialize)]
    struct Journal<'a> {
        format: &'static str,
        version: &'static str,
        status: &'a InitializationStatusV1,
        goal: &'static str,
    }

    let temp = Temp::new();
    let home = temp.0.join("no-write-home");
    let project = temp.0.join("no-write-project");
    let other = temp.0.join("other-project");
    let runtime = home.join("runtime");
    fs::create_dir(&project).unwrap();
    fs::create_dir(&other).unwrap();
    fs::create_dir(&home).unwrap();
    fs::create_dir(&runtime).unwrap();
    private_directory(&home);
    private_directory(&runtime);
    let operation_id = "n".repeat(43);
    let started = InitializationStatusV1 {
        operation_id: operation_id.clone(),
        canonical_root: project.to_string_lossy().into_owned(),
        checkpoint: InitializationCheckpointV1::None,
        outcome: InitializationOutcomeV1::InProgress,
        error_kind: String::new(),
    };
    let mut bytes = serde_json::to_vec(&Journal {
        format: "ptrack-desktop-initialization",
        version: "1",
        status: &started,
        goal: "resume without writes",
    })
    .unwrap();
    bytes.push(b'\n');
    let journal = runtime.join("desktop-initialization.json");
    fs::write(&journal, bytes).unwrap();
    private_file(&journal);

    let authority = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let restarted = authority.status(&operation_id).unwrap();
    assert_eq!(restarted.checkpoint, InitializationCheckpointV1::None);
    assert_eq!(restarted.outcome, InitializationOutcomeV1::Ready);
    assert_eq!(restarted.error_kind, "interrupted-before-commit");
    let pending = authority.pending().unwrap();
    assert!(!pending.pending);
    assert!(pending.initialization.is_none());
    assert!(pending.validation.is_none());
    assert!(!project.join(".ptrack").exists());
    assert_eq!(
        authority.validate_target(&project).unwrap().operation_id,
        operation_id
    );
    let replacement = authority.validate_target(&other).unwrap();
    assert_eq!(replacement.kind, ProjectTargetKindV1::New);
    assert_ne!(replacement.operation_id, operation_id);
    assert!(!other.join(".ptrack").exists());
}

#[test]
fn startup_inference_rereads_after_a_concurrent_initializer_advances() {
    #[derive(serde::Serialize)]
    struct Journal<'a> {
        format: &'static str,
        version: &'static str,
        status: &'a InitializationStatusV1,
        goal: &'static str,
    }

    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    let runtime = home.join("runtime");
    fs::create_dir(&project).unwrap();
    fs::create_dir(&home).unwrap();
    fs::create_dir(&runtime).unwrap();
    private_directory(&home);
    private_directory(&runtime);
    let operation_id = "q".repeat(43);
    let ready = InitializationStatusV1 {
        operation_id: operation_id.clone(),
        canonical_root: project.to_string_lossy().into_owned(),
        checkpoint: InitializationCheckpointV1::None,
        outcome: InitializationOutcomeV1::Ready,
        error_kind: String::new(),
    };
    let mut bytes = serde_json::to_vec(&Journal {
        format: "ptrack-desktop-initialization",
        version: "1",
        status: &ready,
        goal: "serialize startup inference",
    })
    .unwrap();
    bytes.push(b'\n');
    let journal = runtime.join("desktop-initialization.json");
    fs::write(&journal, bytes).unwrap();
    private_file(&journal);

    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let loading_home = home.clone();
    let loading_entered = Arc::clone(&entered);
    let loading_release = Arc::clone(&release);
    let loading = std::thread::spawn(move || {
        crate::production::set_startup_initialization_inference_hook(move || {
            loading_entered.wait();
            loading_release.wait();
        });
        ProductionDesktopAuthority::load(loading_home, "test", None, None, 0)
    });
    entered.wait();

    let winner = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = winner.validate_target(&project).unwrap();
    assert_eq!(validation.operation_id, operation_id);
    winner
        .initialize(&InitializeProjectRequestV1 {
            operation_id: operation_id.clone(),
            root: validation.canonical_root,
            goal: "serialize startup inference".to_owned(),
            guide_choice: ProjectGuideChoiceV1::Skip,
            guide_preview_token: String::new(),
        })
        .unwrap();
    winner.mark_desktop_bound(&operation_id).unwrap();
    release.wait();

    let loaded = loading.join().unwrap().unwrap();
    let observed = loaded.status(&operation_id).unwrap();
    assert_eq!(
        observed.checkpoint,
        InitializationCheckpointV1::DesktopBound
    );
    assert_eq!(observed.outcome, InitializationOutcomeV1::Complete);
    assert!(!loaded.pending().unwrap().pending);
}

#[test]
fn status_and_pending_refresh_completion_from_another_authority() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let seed = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = seed.validate_target(&project).unwrap();
    let operation_id = validation.operation_id.clone();
    seed.initialize(&InitializeProjectRequestV1 {
        operation_id: operation_id.clone(),
        root: validation.canonical_root,
        goal: "refresh another authority".to_owned(),
        guide_choice: ProjectGuideChoiceV1::Skip,
        guide_preview_token: String::new(),
    })
    .unwrap();
    drop(seed);

    let observer = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let winner = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let desktop = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: observer.clone(),
        event_sink: None,
        initial_workspace: None,
        recent_projects: Arc::new(NoRecentProjectsProvider),
        initialization: observer.clone(),
        update_service: UnavailableUpdateService::new("test"),
        confirmation_ttl: Duration::from_secs(60),
    });
    assert!(observer.pending().unwrap().pending);
    winner.mark_desktop_bound(&operation_id).unwrap();

    assert_eq!(
        desktop
            .invoke(DesktopCommandRequest {
                method: "GetPendingInitializationV1".to_owned(),
                arguments: Vec::new(),
            })
            .unwrap(),
        serde_json::json!({ "pending": false })
    );
    assert_eq!(desktop.workspace_state().status, WorkspaceStatus::Open);
    assert_eq!(desktop.workspace_state().generation, 1);
    let status_value = desktop
        .invoke(DesktopCommandRequest {
            method: "GetInitializationStatusV1".to_owned(),
            arguments: vec![serde_json::json!(operation_id)],
        })
        .unwrap();
    let completed_status: InitializationStatusV1 = serde_json::from_value(status_value).unwrap();
    assert_eq!(
        completed_status.checkpoint,
        InitializationCheckpointV1::DesktopBound
    );
    assert_eq!(completed_status.outcome, InitializationOutcomeV1::Complete);
    assert_eq!(desktop.workspace_state().status, WorkspaceStatus::Open);
    assert_eq!(desktop.workspace_state().generation, 1);
}

#[test]
fn durable_checkpoint_never_falls_back_to_local_state_after_journal_loss() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let seed = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = seed.validate_target(&project).unwrap();
    let operation_id = validation.operation_id.clone();
    seed.initialize(&InitializeProjectRequestV1 {
        operation_id: operation_id.clone(),
        root: validation.canonical_root,
        goal: "fail closed after journal loss".to_owned(),
        guide_choice: ProjectGuideChoiceV1::Skip,
        guide_preview_token: String::new(),
    })
    .unwrap();
    drop(seed);
    let observer = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    fs::remove_file(home.join("runtime/desktop-initialization.json")).unwrap();

    assert_eq!(
        observer.status(&operation_id).unwrap_err().to_string(),
        "initialization status is unavailable"
    );
    assert_eq!(
        observer.pending().unwrap_err().to_string(),
        "initialization status is unavailable"
    );
}

#[test]
fn target_vanishing_at_commit_records_project_not_found_without_project_writes() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    let moved = temp.0.join("moved-project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let request = InitializeProjectRequestV1 {
        operation_id: validation.operation_id,
        root: validation.canonical_root,
        goal: "report the vanished target".to_owned(),
        guide_choice: ProjectGuideChoiceV1::Skip,
        guide_preview_token: String::new(),
    };
    let project_for_hook = project.clone();
    let moved_for_hook = moved.clone();
    crate::production::set_initialization_before_commit_hook(move || {
        fs::rename(project_for_hook, moved_for_hook).unwrap();
    });

    assert!(authority.initialize(&request).is_err());

    let status = authority.status(&request.operation_id).unwrap();
    assert_eq!(status.checkpoint, InitializationCheckpointV1::None);
    assert_eq!(status.outcome, InitializationOutcomeV1::Ready);
    assert_eq!(status.error_kind, "project-not-found");
    assert!(!moved.join(".ptrack").exists());
    assert!(!home.join("runtime/bootstrap.json").exists());
}

#[test]
fn target_vanishing_before_initialize_is_typed_and_remains_no_write() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    let moved = temp.0.join("moved-project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let request = InitializeProjectRequestV1 {
        operation_id: validation.operation_id,
        root: validation.canonical_root,
        goal: "report the vanished target before commit".to_owned(),
        guide_choice: ProjectGuideChoiceV1::Skip,
        guide_preview_token: String::new(),
    };
    fs::rename(&project, &moved).unwrap();

    assert_eq!(
        authority.initialize(&request).unwrap_err().to_string(),
        "project-not-found"
    );

    let status = authority.status(&request.operation_id).unwrap();
    assert_eq!(status.checkpoint, InitializationCheckpointV1::None);
    assert_eq!(status.outcome, InitializationOutcomeV1::Ready);
    assert_eq!(status.error_kind, "project-not-found");
    assert!(!home.exists());
    assert!(!moved.join(".ptrack").exists());
}

fn interrupt_after_bootstrap_plan(label: &str) -> (Temp, PathBuf, PathBuf, String) {
    let temp = Temp::new();
    let home = temp.0.join(format!("{label}-home"));
    let project = temp.0.join(format!("{label}-project"));
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let operation_id = validation.operation_id.clone();
    let request = InitializeProjectRequestV1 {
        operation_id: operation_id.clone(),
        root: validation.canonical_root,
        goal: "recover the published bootstrap plan".to_owned(),
        guide_choice: ProjectGuideChoiceV1::Skip,
        guide_preview_token: String::new(),
    };
    crate::production::set_initialization_after_bootstrap_plan_hook(|| {
        panic!("simulated interruption after bootstrap plan");
    });
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = authority.initialize(&request);
        }))
        .is_err()
    );
    drop(authority);
    assert!(home.join("runtime/bootstrap.json").exists());
    assert!(project.join(".ptrack").exists());
    (temp, home, project, operation_id)
}

#[test]
fn bootstrap_plan_without_project_directory_is_pending_recovery_not_welcome() {
    let (_temp, home, project, operation_id) =
        interrupt_after_bootstrap_plan("missing-project-directory");
    fs::remove_dir(project.join(".ptrack")).unwrap();

    let restarted = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let status = restarted.status(&operation_id).unwrap();
    assert_eq!(status.checkpoint, InitializationCheckpointV1::Prepared);
    assert_eq!(status.outcome, InitializationOutcomeV1::RecoveryRequired);
    assert_eq!(status.error_kind, "interrupted-bootstrap-plan");
    let pending = restarted.pending().unwrap();
    assert!(pending.pending);
    assert_eq!(pending.initialization, Some(status));
    assert_eq!(
        pending.validation.unwrap().kind,
        ProjectTargetKindV1::RecoveryRequired
    );
}

#[test]
fn bootstrap_plan_with_vanished_root_preserves_exact_pending_checkpoint() {
    let (_temp, home, project, operation_id) = interrupt_after_bootstrap_plan("missing-root");
    let moved = project.with_file_name("moved-bootstrap-project");
    fs::rename(&project, moved).unwrap();

    let restarted = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let status = restarted.status(&operation_id).unwrap();
    assert_eq!(status.checkpoint, InitializationCheckpointV1::Prepared);
    assert_eq!(status.outcome, InitializationOutcomeV1::RecoveryRequired);
    assert_eq!(status.error_kind, "project-not-found");
    let pending = restarted.pending().unwrap();
    assert!(pending.pending);
    assert_eq!(pending.initialization, Some(status));
    assert_eq!(
        pending.validation.unwrap().kind,
        ProjectTargetKindV1::RecoveryRequired
    );
}

#[cfg(unix)]
#[test]
fn interrupted_install_before_prepared_can_restart_and_explicitly_skip() {
    let temp = Temp::new();
    let home = temp.0.join("home");
    let project = temp.0.join("project");
    fs::create_dir(&project).unwrap();
    let authority = ProductionDesktopAuthority::load(home.clone(), "test", None, None, 0).unwrap();
    let validation = authority.validate_target(&project).unwrap();
    let preview = authority
        .preview_guide(&ProjectGuidePreviewRequestV1 {
            operation_id: validation.operation_id.clone(),
            root: validation.canonical_root.clone(),
        })
        .unwrap();
    let install = InitializeProjectRequestV1 {
        operation_id: validation.operation_id,
        root: validation.canonical_root,
        goal: "skip an interrupted preview".to_owned(),
        guide_choice: ProjectGuideChoiceV1::Install,
        guide_preview_token: preview.preview_token,
    };
    crate::production::set_initialization_after_started_hook(|| {
        panic!("simulated process interruption");
    });
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = authority.initialize(&install);
        }))
        .is_err()
    );
    drop(authority);
    assert!(!project.join(".ptrack").exists());
    assert!(!home.join("runtime/bootstrap.json").exists());

    let restarted = ProductionDesktopAuthority::load(home, "test", None, None, 0).unwrap();
    let ready = restarted.status(&install.operation_id).unwrap();
    assert_eq!(ready.checkpoint, InitializationCheckpointV1::None);
    assert_eq!(ready.outcome, InitializationOutcomeV1::Ready);
    assert_eq!(ready.error_kind, "interrupted-before-commit");
    let validation = restarted.validate_target(&project).unwrap();
    assert_eq!(validation.operation_id, install.operation_id);
    assert_eq!(validation.guide_choice, Some(ProjectGuideChoiceV1::Install));
    let skipped = restarted
        .initialize(&InitializeProjectRequestV1 {
            operation_id: install.operation_id,
            root: install.root,
            goal: install.goal,
            guide_choice: ProjectGuideChoiceV1::Skip,
            guide_preview_token: String::new(),
        })
        .unwrap();
    assert_eq!(skipped.checkpoint, InitializationCheckpointV1::GuideApplied);
    assert!(!project.join("AGENTS.md").exists());
    assert!(!project.join("CLAUDE.md").exists());
}

#[test]
fn desktop_initialization_status_transitions_never_regress_or_cross_operations() {
    use crate::production::validate_desktop_initialization_transition;

    let status = |operation: char,
                  checkpoint: InitializationCheckpointV1,
                  outcome: InitializationOutcomeV1| InitializationStatusV1 {
        operation_id: operation.to_string().repeat(43),
        canonical_root: "/project".to_owned(),
        checkpoint,
        outcome,
        error_kind: String::new(),
    };
    let complete = status(
        'a',
        InitializationCheckpointV1::DesktopBound,
        InitializationOutcomeV1::Complete,
    );
    let recovery = status(
        'a',
        InitializationCheckpointV1::ProjectCommitted,
        InitializationOutcomeV1::RecoveryRequired,
    );
    assert!(validate_desktop_initialization_transition(&complete, &recovery).is_err());

    let other_started = status(
        'b',
        InitializationCheckpointV1::None,
        InitializationOutcomeV1::InProgress,
    );
    assert!(validate_desktop_initialization_transition(&other_started, &complete).is_err());
    let replacement_start = status(
        'c',
        InitializationCheckpointV1::None,
        InitializationOutcomeV1::InProgress,
    );
    validate_desktop_initialization_transition(&other_started, &replacement_start).unwrap();
}

#[test]
fn stale_guide_skip_is_allowed_only_before_guide_application() {
    use crate::production::stale_guide_skip_allowed;

    let status = |checkpoint, outcome, error_kind: &str| InitializationStatusV1 {
        operation_id: "a".repeat(43),
        canonical_root: "/project".to_owned(),
        checkpoint,
        outcome,
        error_kind: error_kind.to_owned(),
    };
    for checkpoint in [
        InitializationCheckpointV1::Prepared,
        InitializationCheckpointV1::RuntimeCommitted,
        InitializationCheckpointV1::ProjectCommitted,
    ] {
        assert!(stale_guide_skip_allowed(&status(
            checkpoint,
            InitializationOutcomeV1::RecoveryRequired,
            "project-guide-preview-stale",
        )));
    }
    assert!(stale_guide_skip_allowed(&status(
        InitializationCheckpointV1::None,
        InitializationOutcomeV1::Ready,
        "project-guide-preview-stale",
    )));
    assert!(stale_guide_skip_allowed(&status(
        InitializationCheckpointV1::None,
        InitializationOutcomeV1::Ready,
        "interrupted-before-commit",
    )));
    assert!(!stale_guide_skip_allowed(&status(
        InitializationCheckpointV1::GuideApplied,
        InitializationOutcomeV1::InProgress,
        "project-guide-preview-stale",
    )));
    assert!(!stale_guide_skip_allowed(&status(
        InitializationCheckpointV1::RuntimeCommitted,
        InitializationOutcomeV1::RecoveryRequired,
        "project-guide-partially-applied",
    )));
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn desktop_authority_rejects_non_utf8_target_paths() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    let temp = Temp::new();
    let project = temp.0.join(OsString::from_vec(b"project-\xff".to_vec()));
    fs::create_dir(&project).unwrap();
    let authority =
        ProductionDesktopAuthority::load(temp.0.join("home"), "test", None, None, 0).unwrap();

    assert!(
        authority
            .validate_target(&project)
            .unwrap_err()
            .to_string()
            .contains("not valid UTF-8")
    );
}

fn private_directory(path: &Path) {
    protect_private_directory(path).unwrap();
}

fn private_file(path: &Path) {
    protect_private_file(path).unwrap();
}

fn write_test_bootstrap_plan(path: &Path, plan: &TestBootstrapPlan) {
    let mut bytes = serde_json::to_vec(plan).unwrap();
    bytes.push(b'\n');
    fs::write(path, bytes).unwrap();
    private_file(path);
}
