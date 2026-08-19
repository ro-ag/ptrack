use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use ptrack_capability::{BrokerConfig, BrokerServer, BrokerServerConfig, McpCancellation};
use ptrack_core::{NoteTarget, PlanStatus, TaskStatus};
use ptrack_store::{
    ActiveBinding, GlobalStore, PinnedProjectDirectory, ProjectStore, StoreError, StoreKind,
};

use crate::{
    AppError, ApplicationPort, CapabilityMcpOutcome, CapabilitySessionEnvironment,
    INVALID_HOLD_PREFIX, InitRequest, LocalApplication, Mutation, MutationResult,
    PlanLifecycleOutcome, PlanLifecycleRequest, ProjectEndpoint, WorkspaceBindings,
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
        ptrack_store::protect_private_directory(&path).expect("protect test directory");
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
    ptrack_store::protect_private_directory(&root.join(".ptrack"))
        .expect("protect project directory");
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
fn hold_mutations_reach_the_store_and_keep_the_underlying_status() {
    let directory = TestDirectory::new("hold");
    let (mut application, _) = configured(&directory, true);
    let MutationResult::Plan(plan) = application
        .mutate(Mutation::AddPlan {
            title: "one".to_owned(),
            milestone_id: 0,
        })
        .expect("add plan")
    else {
        panic!("wrong mutation result");
    };
    let MutationResult::Task(task) = application
        .mutate(Mutation::AddTask {
            plan_id: plan.id,
            title: "work".to_owned(),
        })
        .expect("add task")
    else {
        panic!("wrong mutation result");
    };

    application
        .mutate(Mutation::SetTaskStatus {
            id: task.id,
            status: TaskStatus::Doing,
        })
        .expect("start task");
    application
        .mutate(Mutation::SetTaskHold {
            id: task.id,
            reason: Some("waiting on review".to_owned()),
        })
        .expect("hold task");
    application
        .mutate(Mutation::SetPlanHold {
            id: plan.id,
            reason: Some("paused".to_owned()),
        })
        .expect("hold plan");

    let snapshot = application.snapshot().expect("snapshot");
    assert_eq!(
        snapshot.tasks[0].hold_reason.as_deref(),
        Some("waiting on review")
    );
    assert_eq!(snapshot.tasks[0].status, TaskStatus::Doing);
    assert_eq!(snapshot.plans[0].hold_reason.as_deref(), Some("paused"));
    assert_eq!(snapshot.plans[0].status, PlanStatus::Active);

    application
        .mutate(Mutation::SetTaskStatus {
            id: task.id,
            status: TaskStatus::Done,
        })
        .expect("finish task");
    let error = application
        .mutate(Mutation::SetTaskHold {
            id: task.id,
            reason: Some("too late".to_owned()),
        })
        .expect_err("a done task cannot be put on hold");
    assert_eq!(
        error.to_string(),
        format!(
            "{INVALID_HOLD_PREFIX}task #{} is done and cannot be put on hold",
            task.id
        )
    );

    application
        .mutate(Mutation::SetTaskHold {
            id: task.id,
            reason: None,
        })
        .expect("resume task");
    application
        .mutate(Mutation::SetPlanHold {
            id: plan.id,
            reason: None,
        })
        .expect("resume plan");
    let snapshot = application.snapshot().expect("snapshot");
    assert!(snapshot.tasks[0].hold_reason.is_none());
    assert!(snapshot.plans[0].hold_reason.is_none());
}

/// The CLI strips [`INVALID_HOLD_PREFIX`] off an [`AppError`] to show the
/// store's own sentence. Pin the constant against the real `StoreError`
/// rendering so a reworded `Display` fails here instead of leaking the layer
/// prefix to a person.
#[test]
fn the_hold_prefix_constant_is_what_the_store_error_actually_renders() {
    let error = AppError::from(StoreError::InvalidHold(
        "task #1 is done and cannot be put on hold".to_owned(),
    ));
    assert_eq!(
        error.to_string().strip_prefix(INVALID_HOLD_PREFIX),
        Some("task #1 is done and cannot be put on hold")
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

#[test]
fn capability_calls_and_mcp_reopen_store_fence_environment_and_never_leak_token() {
    let directory = TestDirectory::new("capability-service");
    let (application, endpoint) = configured(&directory, true);
    let home = directory.0.join("home");
    let server = BrokerServer::start(BrokerServerConfig {
        global_home: home,
        broker: BrokerConfig {
            project_root: endpoint.root.clone(),
            database: endpoint.database.clone(),
            binding: endpoint.binding.clone(),
            writer_version: "test".to_owned(),
            generation: 9,
        },
    })
    .unwrap();
    let token = server.broker().issue_session_token("agent-codex").unwrap();
    server
        .broker()
        .bind_session(&token, "service-test")
        .unwrap();

    let mut application =
        application.with_capability_environment(CapabilitySessionEnvironment::new(
            token.clone(),
            Some(endpoint.root.clone()),
            Some("8".to_owned()),
        ));
    let mismatch = application
        .capability_call("unknown", "{}")
        .unwrap_err()
        .to_string();
    assert_eq!(
        mismatch,
        "capability broker generation does not match the launched session"
    );
    assert!(!mismatch.contains(&token));

    application = application.with_capability_environment(CapabilitySessionEnvironment::new(
        token.clone(),
        Some(endpoint.root.clone()),
        Some("9".to_owned()),
    ));
    let denied = application
        .capability_call("unknown-secret-bearing-tool", "{}")
        .unwrap_err()
        .to_string();
    assert!(denied.contains("unknown capability tool"));
    assert!(!denied.contains(&token));

    let mut output = Vec::new();
    assert_eq!(
        application
            .capability_mcp(
                Box::new(std::io::Cursor::new(
                    b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\"}}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n".to_vec(),
                )),
                &mut output,
                &McpCancellation::new(),
            )
            .unwrap(),
        CapabilityMcpOutcome::Complete
    );
    let lines: Vec<_> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 3);

    let concurrent =
        ProjectStore::open_existing(&endpoint.database, &endpoint.binding, "concurrent").unwrap();
    drop(concurrent);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let peer = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (input, _) = listener.accept().unwrap();
    let cancellation = McpCancellation::new();
    let worker_cancellation = cancellation.clone();
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let mut output = Vec::new();
        let result = application.capability_mcp(Box::new(input), &mut output, &worker_cancellation);
        result_tx.send((result, output)).unwrap();
    });
    std::thread::sleep(Duration::from_millis(50));
    cancellation.cancel();
    let (outcome, output) = result_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("application MCP cancellation stayed blocked on open input");
    assert_eq!(outcome.unwrap(), CapabilityMcpOutcome::Cancelled);
    assert!(output.is_empty());
    drop(peer);
    worker.join().unwrap();
    server.shutdown().unwrap();
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
    assert_eq!(error.to_string(), "project-guide-preview-stale");
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
fn guide_install_uses_the_shared_project_root_publication_lock() {
    let directory = TestDirectory::new("guide-root-lock");
    let (mut application, endpoint) = configured(&directory, true);
    let retained = PinnedProjectDirectory::prepare(&endpoint.root).expect("retain root lock");

    let error = application
        .guide(GuideAction::Install)
        .expect_err("concurrent ptrack publisher must be fenced");
    assert!(error.to_string().contains("busy"));
    assert!(!endpoint.root.join("AGENTS.md").exists());
    assert!(!endpoint.root.join("CLAUDE.md").exists());

    drop(retained);
    application
        .guide(GuideAction::Install)
        .expect("guide install after lock release");
}

#[cfg(unix)]
#[test]
fn guide_install_publishes_through_retained_root_after_path_replacement() {
    let directory = TestDirectory::new("guide-root-replacement");
    let (mut application, endpoint) = configured(&directory, true);
    let moved = directory.0.join("moved-project");
    let root = endpoint.root.clone();
    let root_for_hook = root.clone();
    let moved_for_hook = moved.clone();
    crate::production::set_guide_before_publish_hook(move || {
        std::fs::rename(&root_for_hook, &moved_for_hook).expect("move retained root");
        std::fs::create_dir(&root_for_hook).expect("replacement root");
        std::fs::write(root_for_hook.join("marker"), "replacement\n").expect("replacement marker");
    });

    application
        .guide(GuideAction::Install)
        .expect_err("replaced root path must fail closed");
    assert_eq!(
        std::fs::read_to_string(root.join("marker")).unwrap(),
        "replacement\n"
    );
    assert!(!root.join("AGENTS.md").exists());
    assert!(!root.join("CLAUDE.md").exists());
    assert!(!moved.join("AGENTS.md").exists());
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

#[test]
fn plan_lifecycle_delete_previews_then_deletes_with_summary() {
    let test = TestDirectory::new("lifecycle-delete");
    let (mut application, _endpoint) = configured(&test, true);
    let MutationResult::Plan(plan) = application
        .mutate(Mutation::AddPlan {
            title: "Doomed".to_owned(),
            milestone_id: 0,
        })
        .unwrap()
    else {
        panic!("plan result");
    };
    let MutationResult::Task(task) = application
        .mutate(Mutation::AddTask {
            plan_id: plan.id,
            title: "t".to_owned(),
        })
        .unwrap()
    else {
        panic!("task result");
    };
    application
        .mutate(Mutation::AddNote {
            target: NoteTarget::Task,
            target_id: task.id,
            body: "n".to_owned(),
        })
        .unwrap();
    application
        .mutate(Mutation::AddIssue {
            title: "bug".to_owned(),
            body: String::new(),
            severity: None,
            task_id: task.id,
        })
        .unwrap();
    application
        .mutate(Mutation::SetActivePlan(plan.id))
        .unwrap();

    let preview = application
        .plan_lifecycle(PlanLifecycleRequest::DeletePreview { plan_id: plan.id })
        .unwrap();
    let PlanLifecycleOutcome::Preview(summary) = preview else {
        panic!("preview outcome");
    };
    assert_eq!(
        (summary.tasks, summary.notes, summary.issues.len()),
        (1, 1, 1)
    );
    assert!(
        application
            .snapshot()
            .unwrap()
            .plans
            .iter()
            .any(|p| p.id == plan.id)
    );

    let deleted = application
        .plan_lifecycle(PlanLifecycleRequest::Delete { plan_id: plan.id })
        .unwrap();
    let PlanLifecycleOutcome::Deleted(summary) = deleted else {
        panic!("deleted outcome");
    };
    assert_eq!((summary.tasks, summary.notes), (1, 1));
    let snapshot = application.snapshot().unwrap();
    assert!(snapshot.plans.iter().all(|p| p.id != plan.id));
    assert_eq!(snapshot.meta.active_plan, 0);
}

#[test]
fn plan_lifecycle_move_to_current_project_is_refused_pointing_at_rename() {
    let test = TestDirectory::new("lifecycle-move-self");
    let (mut application, _endpoint) = configured(&test, true);
    let MutationResult::Plan(plan) = application
        .mutate(Mutation::AddPlan {
            title: "Stay".to_owned(),
            milestone_id: 0,
        })
        .unwrap()
    else {
        panic!("plan result");
    };
    let error = application
        .plan_lifecycle(PlanLifecycleRequest::Move {
            plan_id: plan.id,
            to: "project".to_owned(),
            rename: None,
        })
        .unwrap_err();
    assert!(error.to_string().contains("ptrack plan rename"));
}

#[test]
fn plan_lifecycle_copy_without_target_requires_rename_and_duplicates_with_it() {
    let test = TestDirectory::new("lifecycle-copy-self");
    let (mut application, _endpoint) = configured(&test, true);
    let MutationResult::Plan(plan) = application
        .mutate(Mutation::AddPlan {
            title: "Original".to_owned(),
            milestone_id: 0,
        })
        .unwrap()
    else {
        panic!("plan result");
    };
    let refusal = application
        .plan_lifecycle(PlanLifecycleRequest::Copy {
            plan_id: plan.id,
            to: None,
            rename: None,
        })
        .unwrap_err();
    assert!(refusal.to_string().contains("--as"));

    let outcome = application
        .plan_lifecycle(PlanLifecycleRequest::Copy {
            plan_id: plan.id,
            to: None,
            rename: Some("Second".to_owned()),
        })
        .unwrap();
    let PlanLifecycleOutcome::Transferred(summary) = outcome else {
        panic!("transfer outcome");
    };
    assert!(!summary.moved);
    assert_eq!(summary.title, "Second");
    let titles: Vec<String> = application
        .snapshot()
        .unwrap()
        .plans
        .iter()
        .map(|p| p.title.clone())
        .collect();
    assert!(titles.contains(&"Original".to_owned()));
    assert!(titles.contains(&"Second".to_owned()));
}

#[test]
fn plan_lifecycle_unknown_target_is_refused_with_projects_hint() {
    let test = TestDirectory::new("lifecycle-unknown-target");
    let (mut application, _endpoint) = configured(&test, true);
    let MutationResult::Plan(plan) = application
        .mutate(Mutation::AddPlan {
            title: "Lost".to_owned(),
            milestone_id: 0,
        })
        .unwrap()
    else {
        panic!("plan result");
    };
    let error = application
        .plan_lifecycle(PlanLifecycleRequest::Move {
            plan_id: plan.id,
            to: "no-such-project".to_owned(),
            rename: None,
        })
        .unwrap_err();
    assert!(error.to_string().contains("ptrack projects"));
}

#[test]
fn plan_lifecycle_ambiguous_target_name_is_refused_but_the_exact_path_resolves() {
    let test = TestDirectory::new("lifecycle-ambiguous");
    let (mut application, endpoint) = configured(&test, true);
    application
        .mutate(Mutation::AddPlan {
            title: "Shared".to_owned(),
            milestone_id: 0,
        })
        .unwrap();

    // Registry names are directory basenames: two different roots can both be
    // called "twin", and picking either by registry order would be a guess.
    let first = test.0.join("a/twin");
    let second = test.0.join("b/twin");
    std::fs::create_dir_all(&first).expect("first twin");
    std::fs::create_dir_all(&second).expect("second twin");
    let global_database = test.0.join("home/global.redb");
    let registry = GlobalStore::open_existing(
        &global_database,
        &binding(&global_database, StoreKind::Global, "global-9"),
    )
    .expect("open registry");
    registry
        .register_project("twin", &first)
        .expect("register first");
    registry
        .register_project("twin", &second)
        .expect("register second");
    drop(registry);

    let error = application
        .plan_lifecycle(PlanLifecycleRequest::Copy {
            plan_id: 1,
            to: Some("twin".to_owned()),
            rename: None,
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("ambiguous"), "{error}");
    assert!(
        error.contains(&first.to_string_lossy().into_owned()),
        "{error}"
    );
    assert!(
        error.contains(&second.to_string_lossy().into_owned()),
        "{error}"
    );

    // The exact path is unambiguous, so lookup succeeds and the refusal that
    // follows comes from the marker, not from the registry.
    let resolved = application
        .plan_lifecycle(PlanLifecycleRequest::Copy {
            plan_id: 1,
            to: Some(second.to_string_lossy().into_owned()),
            rename: None,
        })
        .unwrap_err()
        .to_string();
    assert!(!resolved.contains("ambiguous"), "{resolved}");
    assert!(!resolved.contains("unknown target project"), "{resolved}");
    assert_ne!(endpoint.root, second);
}

#[test]
fn target_open_failures_are_fail_closed_and_only_stale_schemas_get_the_upgrade_hint() {
    let root = Path::new("/tmp/some-target-project");
    let stale = crate::service::target_open_error(
        root,
        &StoreError::InvalidManifest("activation generation is missing".to_owned()),
    )
    .to_string();
    assert!(stale.contains("/tmp/some-target-project"), "{stale}");
    assert!(stale.contains("upgrade ptrack for that project"), "{stale}");

    // A busy target is a retry-later condition, not a version problem: the
    // upgrade hint would send the caller off to reinstall for nothing.
    let busy = crate::service::target_open_error(root, &StoreError::Busy).to_string();
    assert!(busy.contains("/tmp/some-target-project"), "{busy}");
    assert!(!busy.contains("upgrade ptrack"), "{busy}");
}
