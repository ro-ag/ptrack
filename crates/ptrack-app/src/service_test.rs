use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use ptrack_capability::{BrokerConfig, BrokerServer, BrokerServerConfig, McpCancellation};
use ptrack_core::PlanStatus;
use ptrack_store::{ActiveBinding, GlobalStore, PinnedProjectDirectory, ProjectStore, StoreKind};

use crate::{
    ApplicationPort, CapabilityMcpOutcome, CapabilitySessionEnvironment, InitRequest,
    LocalApplication, Mutation, MutationResult, ProjectEndpoint, WorkspaceBindings,
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
