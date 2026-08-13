use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ptrack_agent::{
    ActivityState, AgentIntelligenceDetail, AgentIntelligenceV2, AgentRuntimeSummary,
    BoundedSnapshot, IntelligenceConfidence, IntelligenceState, LeaseState, ProcessState,
    RegistrationKind, RunState, RuntimeAssociation,
};
use ptrack_core::{
    Commit, IssueStatus, MemoryKind, Meta, Note, NoteTarget, ProjectSnapshot, Severity, Timestamp,
};
use ptrack_store::{ActiveBinding, GlobalStore, ProjectStore, StoreKind};
use ptrack_terminal::{Manager, TerminalAssociationPointer};
use serde_json::{Value, json};

use super::desktop_runtime::{
    ActiveResourceSummary, BoundDesktopWorkspace, DesktopCommandRequest, DesktopRuntime,
    DesktopRuntimeConfig, DesktopWorkspace, DesktopWorkspaceFactory, RecentProjectsProvider,
    WorkspaceProject, WorkspaceStatus, agent_intelligence_for_task_result,
    allowed_desktop_commands, capture_git_snapshot_with, confirm_linked_launch, heatmap_at,
    project_storage, watch_workspace_data,
};
use crate::{
    AppError, AppResult, DesktopEvent, DesktopEventSink, LocalApplication, ProjectEndpoint,
    TerminalRuntime, TerminalRuntimeConfig, WorkspaceBindings,
};

use super::terminal_runtime_test::{TestEvents, TestFactory, TestIdentity, profile};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ptrack-desktop-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct FakeWorkspace {
    project: WorkspaceProject,
    resources: Mutex<ActiveResourceSummary>,
    shutdowns: AtomicUsize,
}

impl FakeWorkspace {
    fn new(root: &Path, generation: u64) -> Arc<Self> {
        Arc::new(Self {
            project: WorkspaceProject {
                name: root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("project")
                    .to_owned(),
                root: root.to_string_lossy().into_owned(),
                db_path: root.join("project.redb").to_string_lossy().into_owned(),
            },
            resources: Mutex::new(ActiveResourceSummary {
                resource_revision: generation,
                ..ActiveResourceSummary::default()
            }),
            shutdowns: AtomicUsize::new(0),
        })
    }
}

impl DesktopWorkspace for FakeWorkspace {
    fn project(&self) -> WorkspaceProject {
        self.project.clone()
    }

    fn invoke(&self, method: &str, arguments: &[Value]) -> AppResult<Value> {
        Ok(json!({ "method": method, "arguments": arguments }))
    }

    fn active_resources(&self) -> AppResult<ActiveResourceSummary> {
        Ok(*self.resources.lock().unwrap())
    }

    fn shutdown(&self) -> AppResult<()> {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct BlockingWorkspace {
    inner: Arc<FakeWorkspace>,
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}

struct RetryWorkspace {
    inner: Arc<FakeWorkspace>,
    attempts: AtomicUsize,
}

impl DesktopWorkspace for RetryWorkspace {
    fn project(&self) -> WorkspaceProject {
        self.inner.project()
    }

    fn invoke(&self, method: &str, arguments: &[Value]) -> AppResult<Value> {
        self.inner.invoke(method, arguments)
    }

    fn active_resources(&self) -> AppResult<ActiveResourceSummary> {
        self.inner.active_resources()
    }

    fn shutdown(&self) -> AppResult<()> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            Err(AppError::Message("cleanup failed".to_owned()))
        } else {
            self.inner.shutdown()
        }
    }
}

struct FixedRecentProjects(Vec<Value>);

impl RecentProjectsProvider for FixedRecentProjects {
    fn recent_projects(&self) -> AppResult<Vec<Value>> {
        Ok(self.0.clone())
    }
}

impl DesktopWorkspace for BlockingWorkspace {
    fn project(&self) -> WorkspaceProject {
        self.inner.project()
    }

    fn invoke(&self, _method: &str, _arguments: &[Value]) -> AppResult<Value> {
        self.entered.wait();
        self.release.wait();
        Ok(Value::Null)
    }

    fn active_resources(&self) -> AppResult<ActiveResourceSummary> {
        self.inner.active_resources()
    }

    fn shutdown(&self) -> AppResult<()> {
        self.inner.shutdown()
    }
}

struct FakeFactory {
    fail: AtomicBool,
    builds: Mutex<Vec<(PathBuf, u64)>>,
    built: Mutex<Vec<Arc<FakeWorkspace>>>,
}

struct BlockingFactory {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}

impl DesktopWorkspaceFactory for BlockingFactory {
    fn build(&self, root: &Path, generation: u64) -> AppResult<Arc<dyn DesktopWorkspace>> {
        self.entered.wait();
        self.release.wait();
        Ok(FakeWorkspace::new(root, generation))
    }
}

impl Default for FakeFactory {
    fn default() -> Self {
        Self {
            fail: AtomicBool::new(false),
            builds: Mutex::new(Vec::new()),
            built: Mutex::new(Vec::new()),
        }
    }
}

impl DesktopWorkspaceFactory for FakeFactory {
    fn build(&self, root: &Path, generation: u64) -> AppResult<Arc<dyn DesktopWorkspace>> {
        self.builds
            .lock()
            .unwrap()
            .push((root.to_path_buf(), generation));
        if self.fail.load(Ordering::SeqCst) {
            return Err(AppError::Message("candidate rejected".to_owned()));
        }
        let workspace = FakeWorkspace::new(root, generation);
        self.built.lock().unwrap().push(Arc::clone(&workspace));
        Ok(workspace)
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<DesktopEvent>>);

impl DesktopEventSink for Events {
    fn emit(&self, event: DesktopEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn request(method: &str, arguments: Vec<Value>) -> DesktopCommandRequest {
    DesktopCommandRequest {
        method: method.to_owned(),
        arguments,
    }
}

#[test]
#[allow(clippy::too_many_lines)] // Full 64-command freeze fixture is intentionally explicit.
fn desktop_command_allowlist_is_exact_sorted_unique_and_byte_bounded() {
    let commands = allowed_desktop_commands();
    assert_eq!(
        commands,
        [
            "AcknowledgeAgentHandoffV2",
            "AddTask",
            "AddTaskNote",
            "AddTaskNoteV2",
            "AddTaskV2",
            "ApplyUpdate",
            "ApproveAgentWorkflowV2",
            "AssociateAgentRunV2",
            "AssociateTerminalV2",
            "CancelUpdateOperation",
            "CancelWorkspaceChange",
            "CheckForUpdates",
            "CloseProject",
            "CloseTerminal",
            "CloseTerminalV2",
            "CreateTerminal",
            "CreateTerminalV2",
            "DisableCapabilityV2",
            "DismissAgentWorkflowV2",
            "DownloadUpdate",
            "EnableCapabilityV2",
            "ExpireCapabilityV2",
            "GetActivityHeatmapV2",
            "GetAgentIntelligenceV2",
            "GetAgentRunsV2",
            "GetBoard",
            "GetBoardV2",
            "GetCapabilitiesV2",
            "GetCapabilityAuditsV2",
            "GetRecentProjects",
            "GetTaskDetailV2",
            "GetTerminalProfiles",
            "GetTerminalProfilesV2",
            "GetUpdateState",
            "GetWorkspaceSnapshot",
            "GetWorkspaceState",
            "InstallShellCommand",
            "LaunchLinkedAgentV2",
            "MoveTask",
            "MoveTaskV2",
            "MoveTaskV3",
            "MutateTerminalAssociationV2",
            "OpenHelpDestination",
            "OpenProject",
            "PickProjectDirectory",
            "PrepareAgentWorkflowV2",
            "PreviewAgentHandoffV2",
            "PreviewCapabilityV2",
            "PreviewTerminalWritebackV2",
            "RemoveCapabilityV2",
            "RenameTask",
            "RenameTaskV2",
            "ResizeTerminal",
            "ResizeTerminalV2",
            "RollbackLinkedAgentLaunchV2",
            "SaveCapabilityV2",
            "SearchV2",
            "SendAgentHandoffV2",
            "SetAgentTaskOwnershipV2",
            "SetAgentWorktreeV2",
            "SetAutomaticUpdateChecks",
            "TestCapabilityV2",
            "ValidateTerminalCWDsV2",
            "WriteTerminalMemoryV2",
        ]
    );

    let runtime = DesktopRuntime::new(DesktopRuntimeConfig::unavailable("test"));
    assert_eq!(
        runtime
            .invoke(request("not-authorized", Vec::new()))
            .unwrap_err()
            .to_string(),
        "desktop command is not allowed"
    );
    assert_eq!(
        runtime
            .invoke(request(
                "SearchV2",
                vec![Value::String("x".repeat(1024 * 1024))],
            ))
            .unwrap_err()
            .to_string(),
        "desktop command exceeds its byte limit"
    );
    assert_eq!(
        runtime
            .invoke(request("OpenHelpDestination", vec![json!("terminals")],))
            .unwrap(),
        json!("https://ro-ag.github.io/ptrack/help/terminals/")
    );
    assert_eq!(
        runtime
            .invoke(request(
                "OpenHelpDestination",
                vec![json!("https://evil.example")],
            ))
            .unwrap_err()
            .to_string(),
        "unknown Help destination"
    );
}

#[test]
fn workspace_watcher_debounces_changes_and_joins_on_cancel() {
    let directory = TestDirectory::new("watcher");
    let database = directory.0.join("project.redb");
    std::fs::write(&database, b"one").unwrap();
    let emitted = Arc::new(AtomicUsize::new(0));
    let observed = emitted.clone();
    let (cancel, cancellation) = channel();
    let watched = database.clone();
    let handle = thread::spawn(move || {
        watch_workspace_data(
            &cancellation,
            &watched,
            Duration::from_millis(10),
            Duration::from_millis(100),
            || {
                observed.fetch_add(1, Ordering::SeqCst);
            },
        );
    });
    thread::sleep(Duration::from_millis(50));
    std::fs::write(&database, b"two").unwrap();
    std::fs::write(&database, b"three").unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while emitted.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(emitted.load(Ordering::SeqCst), 1);
    cancel.send(()).unwrap();
    handle.join().unwrap();
}

#[test]
fn heatmap_buckets_instants_in_the_host_local_calendar_day() {
    let event = Timestamp::Fixed {
        seconds: 88_200,
        nanoseconds: 0,
        offset_seconds: 9 * 60 * 60,
    };
    let snapshot = ProjectSnapshot::new(
        Meta {
            goal: String::new(),
            summary: String::new(),
            active_plan: 0,
            created_at: Timestamp::Zero,
            updated_at: Timestamp::Zero,
            format_version: 1,
            last_write_version: String::new(),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![Note {
            id: 1,
            target: NoteTarget::Project,
            target_id: 0,
            kind: MemoryKind::Legacy,
            body: "boundary".to_owned(),
            created_at: event,
        }],
        vec![Commit {
            id: 1,
            sha: "abc".to_owned(),
            subject: "boundary".to_owned(),
            plan_id: 0,
            task_id: 0,
            created_at: event,
        }],
    );
    let now = time::OffsetDateTime::from_unix_timestamp(90_000).unwrap();
    let days = heatmap_at(&snapshot, 1, now, |_| {
        time::UtcOffset::from_hms(-2, 0, 0).unwrap()
    });
    assert_eq!(
        days.last().unwrap(),
        &json!({ "date": "1970-01-01", "count": 2 })
    );
}

#[test]
fn workspace_publication_is_monotonic_and_failed_candidate_preserves_active_generation() {
    let root = TestDirectory::new("publication");
    let first_path = root.0.join("first");
    let second_path = root.0.join("second");
    std::fs::create_dir_all(&first_path).unwrap();
    std::fs::create_dir_all(&second_path).unwrap();
    let factory = Arc::new(FakeFactory::default());
    let events = Arc::new(Events::default());
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "0.22.0".to_owned(),
        factory: factory.clone(),
        event_sink: Some(events.clone()),
        initial_workspace: None,
        recent_projects: Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
        confirmation_ttl: Duration::from_secs(60),
    });

    let welcome = runtime.workspace_state();
    assert_eq!(welcome.status, WorkspaceStatus::Welcome);
    assert_eq!(welcome.generation, 0);
    let opened = runtime
        .invoke(request("OpenProject", vec![json!(first_path), json!("")]))
        .unwrap();
    assert_eq!(opened["state"]["generation"], 1);
    assert_eq!(runtime.workspace_state().status, WorkspaceStatus::Open);
    assert_eq!(
        events.0.lock().unwrap().as_slice(),
        &[DesktopEvent::WorkspaceDataChanged(1)]
    );

    factory.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        runtime
            .invoke(request("OpenProject", vec![json!(second_path), json!("")],))
            .unwrap_err()
            .to_string(),
        "candidate rejected"
    );
    let preserved = runtime.workspace_state();
    assert_eq!(preserved.status, WorkspaceStatus::Open);
    assert_eq!(preserved.generation, 1);
    assert_eq!(
        preserved.project.unwrap().root,
        first_path.to_string_lossy()
    );
    assert_eq!(factory.builds.lock().unwrap()[1].1, 2);
}

#[test]
fn workspace_confirmation_is_random_single_use_and_revision_fenced() {
    let root = TestDirectory::new("confirmation");
    let next = root.0.join("next");
    std::fs::create_dir_all(&next).unwrap();
    let current = FakeWorkspace::new(&root.0, 1);
    *current.resources.lock().unwrap() = ActiveResourceSummary {
        terminals: 2,
        agent_runs: 1,
        pending_admissions: 0,
        resource_revision: 41,
    };
    let factory = Arc::new(FakeFactory::default());
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory,
        event_sink: None,
        initial_workspace: Some(current.clone()),
        recent_projects: Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
        confirmation_ttl: Duration::from_secs(60),
    });

    let challenge = runtime
        .invoke(request("OpenProject", vec![json!(next), json!("")]))
        .unwrap();
    assert_eq!(challenge["requiresConfirmation"], true);
    let token = challenge["confirmationToken"].as_str().unwrap().to_owned();
    assert_eq!(token.len(), 43);
    assert!(!token.contains('='));
    current.resources.lock().unwrap().resource_revision = 42;
    assert_eq!(
        runtime
            .invoke(request("OpenProject", vec![json!(next), json!(token)],))
            .unwrap_err()
            .to_string(),
        "invalid or expired workspace confirmation"
    );

    let challenge = runtime
        .invoke(request("CloseProject", vec![json!("")]))
        .unwrap();
    let token = challenge["confirmationToken"].as_str().unwrap();
    runtime
        .invoke(request("CancelWorkspaceChange", vec![json!(token)]))
        .unwrap();
    assert_eq!(
        runtime
            .invoke(request("CancelWorkspaceChange", vec![json!(token)]))
            .unwrap_err()
            .to_string(),
        "invalid or expired workspace confirmation"
    );
}

#[test]
fn shutdown_is_idempotent_and_fences_future_calls() {
    let root = TestDirectory::new("shutdown");
    let workspace = FakeWorkspace::new(&root.0, 1);
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: Arc::new(FakeFactory::default()),
        event_sink: None,
        initial_workspace: Some(workspace.clone()),
        recent_projects: Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
        confirmation_ttl: Duration::from_secs(60),
    });
    runtime.begin_shutdown().unwrap();
    runtime.begin_shutdown().unwrap();
    assert_eq!(workspace.shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.workspace_state().status, WorkspaceStatus::Closed);
    assert_eq!(
        runtime
            .invoke(request("GetBoard", vec![json!(0)]))
            .unwrap_err()
            .to_string(),
        "terminal lifecycle is shutting down"
    );
    let unopened = root.0.join("unopened");
    std::fs::create_dir(&unopened).unwrap();
    assert_eq!(
        runtime
            .invoke(request("OpenProject", vec![json!(unopened), json!("")]))
            .unwrap_err()
            .to_string(),
        "terminal lifecycle is shutting down"
    );
}

#[test]
fn close_is_idempotent_returns_closed_then_settles_to_welcome() {
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig::unavailable("test"));
    let already_closed = runtime
        .invoke(request("CloseProject", vec![json!("")]))
        .unwrap();
    assert_eq!(already_closed["state"]["status"], "welcome");

    let root = TestDirectory::new("close-welcome");
    let workspace = FakeWorkspace::new(&root.0, 1);
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: Arc::new(FakeFactory::default()),
        event_sink: None,
        initial_workspace: Some(workspace),
        recent_projects: Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
        confirmation_ttl: Duration::from_secs(60),
    });
    let closed = runtime
        .invoke(request("CloseProject", vec![json!("")]))
        .unwrap();
    assert_eq!(closed["state"]["status"], "closed");
    assert_eq!(runtime.workspace_state().status, WorkspaceStatus::Welcome);
    assert_eq!(
        runtime
            .invoke(request("CloseProject", vec![json!("")]))
            .unwrap()["state"]["status"],
        "welcome"
    );
}

#[test]
fn failed_shutdown_remains_fenced_and_retries_cleanup() {
    let root = TestDirectory::new("shutdown-retry");
    let inner = FakeWorkspace::new(&root.0, 1);
    let workspace: Arc<dyn DesktopWorkspace> = Arc::new(RetryWorkspace {
        inner: inner.clone(),
        attempts: AtomicUsize::new(0),
    });
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: Arc::new(FakeFactory::default()),
        event_sink: None,
        initial_workspace: Some(workspace),
        recent_projects: Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
        confirmation_ttl: Duration::from_secs(60),
    });
    assert_eq!(
        runtime.begin_shutdown().unwrap_err().to_string(),
        "cleanup failed"
    );
    assert_eq!(
        runtime.begin_native_action().err().unwrap().to_string(),
        "terminal lifecycle is shutting down"
    );
    runtime.begin_shutdown().unwrap();
    assert_eq!(inner.shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.workspace_state().status, WorkspaceStatus::Closed);
}

#[test]
fn project_change_cleanup_warnings_preserve_exact_context() {
    let root = TestDirectory::new("cleanup-warnings");
    let next = root.0.join("next");
    std::fs::create_dir(&next).unwrap();
    let open_runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: Arc::new(FakeFactory::default()),
        event_sink: None,
        initial_workspace: Some(Arc::new(RetryWorkspace {
            inner: FakeWorkspace::new(&root.0, 1),
            attempts: AtomicUsize::new(0),
        })),
        recent_projects: Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
        confirmation_ttl: Duration::from_secs(60),
    });
    assert_eq!(
        open_runtime
            .invoke(request("OpenProject", vec![json!(next), json!("")]))
            .unwrap()["warning"],
        "previous project cleanup incomplete: cleanup failed"
    );
    let close_runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: Arc::new(FakeFactory::default()),
        event_sink: None,
        initial_workspace: Some(Arc::new(RetryWorkspace {
            inner: FakeWorkspace::new(&root.0, 1),
            attempts: AtomicUsize::new(0),
        })),
        recent_projects: Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
        confirmation_ttl: Duration::from_secs(60),
    });
    assert_eq!(
        close_runtime
            .invoke(request("CloseProject", vec![json!("")]))
            .unwrap()["warning"],
        "project cleanup incomplete: cleanup failed"
    );
}

#[test]
fn recent_projects_are_available_without_an_open_workspace_and_fenced_on_close() {
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: Arc::new(FakeFactory::default()),
        event_sink: None,
        initial_workspace: None,
        recent_projects: Arc::new(FixedRecentProjects(vec![json!({
            "name": "Recent",
            "path": "/project",
            "lastSeen": "2026-08-13T00:00:00Z",
            "available": true
        })])),
        confirmation_ttl: Duration::from_secs(60),
    });
    let recent = runtime
        .invoke(request("GetRecentProjects", Vec::new()))
        .unwrap();
    assert_eq!(recent[0]["name"], "Recent");
    runtime.begin_shutdown().unwrap();
    assert_eq!(
        runtime
            .invoke(request("GetRecentProjects", Vec::new()))
            .unwrap_err()
            .to_string(),
        "terminal lifecycle is shutting down"
    );
}

#[test]
fn close_timeout_restores_admission_and_retry_finishes_after_runtime_call() {
    let root = TestDirectory::new("close-timeout");
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let inner = FakeWorkspace::new(&root.0, 1);
    let workspace: Arc<dyn DesktopWorkspace> = Arc::new(BlockingWorkspace {
        inner: inner.clone(),
        entered: entered.clone(),
        release: release.clone(),
    });
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: Arc::new(FakeFactory::default()),
        event_sink: None,
        initial_workspace: Some(workspace),
        recent_projects: Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
        confirmation_ttl: Duration::from_secs(60),
    });
    let caller = {
        let runtime = runtime.clone();
        std::thread::spawn(move || runtime.invoke(request("GetBoard", vec![json!(0)])))
    };
    entered.wait();
    assert_eq!(
        runtime.begin_shutdown().unwrap_err().to_string(),
        "runtime calls did not finish before close"
    );
    assert_eq!(runtime.workspace_state().status, WorkspaceStatus::Open);
    release.wait();
    caller.join().unwrap().unwrap();
    runtime.begin_shutdown().unwrap();
    assert_eq!(inner.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn in_flight_open_is_bounded_by_shutdown_and_cannot_republish_after_success() {
    let root = TestDirectory::new("open-close-race");
    let project = root.0.join("project");
    std::fs::create_dir(&project).unwrap();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: Arc::new(BlockingFactory {
            entered: entered.clone(),
            release: release.clone(),
        }),
        event_sink: None,
        initial_workspace: None,
        recent_projects: Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
        confirmation_ttl: Duration::from_secs(60),
    });
    let opener = {
        let runtime = runtime.clone();
        std::thread::spawn(move || {
            runtime.invoke(request("OpenProject", vec![json!(project), json!("")]))
        })
    };
    entered.wait();
    assert_eq!(
        runtime.begin_shutdown().unwrap_err().to_string(),
        "runtime calls did not finish before close"
    );
    release.wait();
    opener.join().unwrap().unwrap();
    assert_eq!(runtime.workspace_state().status, WorkspaceStatus::Open);
    runtime.begin_shutdown().unwrap();
    assert_eq!(runtime.workspace_state().status, WorkspaceStatus::Closed);
}

fn binding(path: &Path, kind: StoreKind, id: &str) -> ActiveBinding {
    ActiveBinding {
        generation: 7,
        database_id: id.to_owned(),
        kind,
        canonical_path: path.to_path_buf(),
    }
}

fn bound_bindings(directory: &TestDirectory) -> (WorkspaceBindings, u64) {
    let root = directory.0.join("project");
    let home = directory.0.join("home");
    std::fs::create_dir_all(root.join(".ptrack")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let project_database = root.join(".ptrack/ptrack.redb");
    let global_database = home.join("global.redb");
    let project_binding = binding(&project_database, StoreKind::Project, "project-7");
    let global_binding = binding(&global_database, StoreKind::Global, "global-7");
    let project =
        ProjectStore::create_new(&project_database, project_binding.clone(), "test").unwrap();
    project.set_goal("Ship parity").unwrap();
    let plan = project.add_plan("Desktop", 0).unwrap();
    project.set_active_plan(plan.id).unwrap();
    let task = project.add_task(plan.id, "Wire bridge").unwrap();
    project
        .add_note(
            ptrack_core::NoteTarget::Task,
            task.id,
            "Remember generation fences",
        )
        .unwrap();
    drop(project);
    drop(GlobalStore::create_new(&global_database, global_binding.clone()).unwrap());
    let endpoint = ProjectEndpoint {
        root: root.clone(),
        database: project_database,
        binding: project_binding,
    };
    (
        WorkspaceBindings {
            current_dir: root,
            project: Some(endpoint),
            global_database,
            global_binding,
            global_home: home,
            writer_version: "test".to_owned(),
        },
        task.id,
    )
}

fn bound_workspace(directory: &TestDirectory) -> BoundDesktopWorkspace {
    let (bindings, _) = bound_bindings(directory);
    BoundDesktopWorkspace::new(
        7,
        0,
        bindings.clone(),
        Box::new(LocalApplication::new(bindings)),
        None,
        None,
        None,
    )
}

#[test]
#[allow(clippy::too_many_lines)] // One end-to-end bounded workspace projection contract.
fn bound_workspace_projects_board_search_mutations_and_capability_preview() {
    let directory = TestDirectory::new("bound");
    let workspace = bound_workspace(&directory);
    let board = workspace
        .invoke("GetBoardV2", &[json!(7), json!(0)])
        .unwrap();
    assert_eq!(board["generation"], 7);
    assert_eq!(board["board"]["goal"], "Ship parity");
    assert_eq!(board["board"]["columns"][0]["tasks"][0]["noteCount"], 1);
    assert_eq!(
        workspace
            .invoke("GetBoardV2", &[json!(8), json!(0)])
            .unwrap_err()
            .to_string(),
        "stale workspace generation: expected 8, active 7"
    );

    let search = workspace
        .invoke("SearchV2", &[json!("generation")])
        .unwrap();
    assert_eq!(search[0]["kind"], "note");
    let added = workspace
        .invoke("AddTaskV2", &[json!(7), json!(1), json!("  Audit menus  ")])
        .unwrap();
    assert_eq!(added["task"]["title"], "Audit menus");

    let preview = workspace
        .invoke(
            "PreviewCapabilityV2",
            &[
                json!(7),
                json!({
                    "name": "Docs",
                    "kind": "http",
                    "agent_profile": "agent-codex",
                    "http": {
                        "base_url": "https://example.com/docs/",
                        "methods": ["GET"],
                        "path_prefixes": ["/docs"]
                    }
                }),
            ],
        )
        .unwrap();
    assert_eq!(preview["generation"], 7);
    assert_eq!(preview["view"]["state"], "draft");
    assert_eq!(preview["view"]["capability"]["enabled"], false);
    assert!(
        preview["view"]["effective_scope"]
            .as_str()
            .is_some_and(|scope| scope.contains("example.com"))
    );

    let saved = workspace
        .invoke(
            "SaveCapabilityV2",
            &[
                json!(7),
                json!({
                    "name": "Docs",
                    "kind": "http",
                    "agent_profile": "agent-codex",
                    "http": {
                        "base_url": "https://example.com/docs/",
                        "methods": ["GET"],
                        "path_prefixes": ["/docs"]
                    }
                }),
            ],
        )
        .unwrap();
    let id = saved["view"]["capability"]["id"].as_u64().unwrap();
    let updated = workspace
        .invoke(
            "SaveCapabilityV2",
            &[
                json!(7),
                json!({
                    "id": id,
                    "revision": 0,
                    "name": "Renamed docs",
                    "kind": "http",
                    "agent_profile": "agent-codex",
                    "http": {
                        "base_url": "https://example.com/docs/",
                        "methods": ["GET"],
                        "path_prefixes": ["/docs"]
                    }
                }),
            ],
        )
        .unwrap();
    assert_eq!(updated["view"]["capability"]["revision"], 2);
    assert_eq!(updated["view"]["capability"]["name"], "Renamed docs");

    let snapshot = workspace
        .invoke("GetWorkspaceSnapshot", &[json!(7), json!(1)])
        .unwrap();
    assert_eq!(snapshot["tracking"]["bounds"]["notes"]["total"], 1);
    assert_eq!(snapshot["terminals"]["sessions"], json!([]));
    assert_eq!(snapshot["agentRuns"]["runs"], json!([]));
    assert!(
        snapshot["capturedAt"]
            .as_str()
            .is_some_and(|value| value.ends_with('Z'))
    );
}

#[test]
fn workspace_snapshot_uses_bounded_store_reads_and_open_issue_rows() {
    let directory = TestDirectory::new("snapshot-bounds");
    let (bindings, _) = bound_bindings(&directory);
    let endpoint = bindings.project.as_ref().unwrap();
    let store = ProjectStore::open_existing(&endpoint.database, &endpoint.binding, "test").unwrap();
    for index in 0..104 {
        store.add_plan(format!("Plan {index}"), 0).unwrap();
    }
    for index in 0..304 {
        let task = store.add_task(1, format!("Task {index}")).unwrap();
        if index < 55 {
            store
                .set_task_status(task.id, ptrack_core::TaskStatus::Blocked)
                .unwrap();
        }
    }
    for index in 0..55 {
        store
            .add_note(NoteTarget::Project, 0, format!("Note {index}"))
            .unwrap();
    }
    for index in 0..60 {
        let issue = store
            .add_issue(format!("Issue {index}"), "body", Some(Severity::Medium), 1)
            .unwrap();
        if index < 5 {
            store
                .set_issue_status(issue.id, IssueStatus::Closed)
                .unwrap();
        }
    }
    drop(store);
    let workspace = BoundDesktopWorkspace::new(
        7,
        0,
        bindings.clone(),
        Box::new(LocalApplication::new(bindings)),
        None,
        None,
        None,
    );
    let snapshot = workspace
        .invoke("GetWorkspaceSnapshot", &[json!(7), json!(1)])
        .unwrap();
    let task_rows = snapshot["tracking"]["board"]["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|column| column["tasks"].as_array().unwrap().len())
        .sum::<usize>();
    assert_eq!(task_rows, 300);
    assert_eq!(
        snapshot["tracking"]["bounds"]["plans"],
        json!({"shown":100,"total":105,"more":5})
    );
    assert_eq!(
        snapshot["tracking"]["bounds"]["tasks"],
        json!({"shown":300,"total":305,"more":5})
    );
    assert_eq!(
        snapshot["tracking"]["bounds"]["blockers"],
        json!({"shown":50,"total":55,"more":5})
    );
    assert_eq!(
        snapshot["tracking"]["bounds"]["notes"],
        json!({"shown":50,"total":56,"more":6})
    );
    assert_eq!(
        snapshot["tracking"]["bounds"]["issues"],
        json!({"shown":50,"total":55,"more":5})
    );
    assert!(
        snapshot["tracking"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .all(|issue| issue.get("status").is_none())
    );
    let blocker = &snapshot["tracking"]["blockers"][0];
    assert_eq!(blocker["noteCount"], 0);
    assert_eq!(blocker["commitCount"], 0);
    assert_eq!(blocker["issueCount"], 0);
    assert_eq!(blocker["latestNote"], "");
}

#[test]
fn workspace_snapshot_allows_no_active_plan_and_reports_missing_storage() {
    let directory = TestDirectory::new("snapshot-no-plan");
    let (bindings, _) = bound_bindings(&directory);
    let endpoint = bindings.project.as_ref().unwrap();
    let store = ProjectStore::open_existing(&endpoint.database, &endpoint.binding, "test").unwrap();
    store.set_active_plan(0).unwrap();
    let meta = store.meta().unwrap();
    drop(store);
    let workspace = BoundDesktopWorkspace::new(
        7,
        0,
        bindings.clone(),
        Box::new(LocalApplication::new(bindings)),
        None,
        None,
        None,
    );
    let snapshot = workspace
        .invoke("GetWorkspaceSnapshot", &[json!(7), json!(0)])
        .unwrap();
    assert_eq!(snapshot["tracking"]["board"]["planId"], 0);
    assert_eq!(
        snapshot["tracking"]["board"]["columns"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert!(
        snapshot["tracking"]["board"]["columns"]
            .as_array()
            .unwrap()
            .iter()
            .all(|column| column["tasks"] == json!([]))
    );
    let missing = project_storage(&directory.0.join("missing.redb").to_string_lossy(), &meta);
    assert_eq!(missing["status"], "error");
    assert_eq!(missing["exists"], false);
    assert_eq!(missing["error"], "p-track database is missing");
}

#[test]
fn git_snapshot_worker_is_deadline_bounded_and_owns_cancellation() {
    let directory = TestDirectory::new("git-deadline");
    let started = Instant::now();
    let captured = capture_git_snapshot_with(
        directory.0.clone(),
        Instant::now() + Duration::from_millis(20),
        |cancellation, _| {
            while !cancellation.is_cancelled() {
                thread::sleep(Duration::from_millis(1));
            }
            Err(ptrack_git::RepositoryError::Cancelled)
        },
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(captured.wire["state"], "error");
    assert!(captured.wire["snapshot"].get("status").is_some());
    assert_eq!(
        captured.agent,
        ptrack_agent::CoordinationGitSnapshot::default()
    );
}

#[test]
fn linked_ownership_check_errors_still_run_and_join_failure_cleanup() {
    let cleaned = AtomicBool::new(false);
    let error = confirm_linked_launch(
        Err(AppError::Message("AgentRun registry is closing".to_owned())),
        || {
            cleaned.store(true, Ordering::SeqCst);
            Err(AppError::Message("forced close failed".to_owned()))
        },
    )
    .unwrap_err();
    assert!(cleaned.load(Ordering::SeqCst));
    assert_eq!(
        error.to_string(),
        "AgentRun registry is closing\nforced close failed"
    );
    assert!(confirm_linked_launch(Ok(true), || panic!("must not clean success")).is_ok());
}

#[test]
fn task_intelligence_skips_disappeared_and_reassociated_runs() {
    let association = RuntimeAssociation {
        plan_id: 1,
        task_id: 9,
        revision: 4,
    };
    let run = AgentRuntimeSummary {
        run_id: "run-1".to_owned(),
        registration_kind: RegistrationKind::External,
        terminal_id: String::new(),
        terminal_backed: false,
        terminal_present: false,
        corresponding_terminal: false,
        state: RunState::Running,
        process_state: ProcessState::Running,
        lease_state: LeaseState::Active,
        live: true,
        activity_state: ActivityState::Waiting,
        association: Some(association),
        intelligence: None,
    };
    let intelligence = |association| AgentIntelligenceV2 {
        generation: 7,
        run_id: "run-1".to_owned(),
        association: Some(association),
        intelligence: AgentIntelligenceDetail {
            state: IntelligenceState::Waiting,
            confidence: IntelligenceConfidence::High,
            evidence: Vec::new(),
            event_count: 1,
            last_event_at: None,
        },
        event_bounds: BoundedSnapshot::new(1, 1),
        suggestions: Vec::new(),
        bounds: BoundedSnapshot::new(0, 0),
    };
    assert!(
        agent_intelligence_for_task_result(
            &run,
            9,
            Err(AppError::Message("AgentRun not found".to_owned())),
        )
        .unwrap()
        .is_none()
    );
    assert!(
        agent_intelligence_for_task_result(
            &run,
            9,
            Ok(intelligence(RuntimeAssociation {
                revision: 5,
                ..association
            })),
        )
        .unwrap()
        .is_none()
    );
    assert!(
        agent_intelligence_for_task_result(&run, 9, Ok(intelligence(association)))
            .unwrap()
            .is_some()
    );
}

#[test]
fn workspace_shutdown_drains_its_own_calls_and_fences_late_invocations() {
    let directory = TestDirectory::new("workspace-call-drain");
    let workspace = Arc::new(bound_workspace(&directory));
    let lease = workspace.begin_workspace_call().unwrap();
    let (finished_tx, finished_rx) = channel();
    let shutdown_workspace = Arc::clone(&workspace);
    let handle = thread::spawn(move || {
        let result = shutdown_workspace.shutdown();
        finished_tx.send(result).unwrap();
    });
    assert!(finished_rx.recv_timeout(Duration::from_millis(50)).is_err());
    drop(lease);
    finished_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    handle.join().unwrap();
    assert_eq!(
        workspace
            .invoke("GetBoardV2", &[json!(7), json!(1)])
            .unwrap_err()
            .to_string(),
        "workspace is closing"
    );
}

#[test]
fn linked_launch_cwd_preserves_a_verified_worktree_subdirectory() {
    let project = TestDirectory::new("linked-project");
    let worktree_parent = TestDirectory::new("linked-worktree");
    let run = |arguments: &[&str]| {
        let status = std::process::Command::new("git")
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?}");
    };
    run(&["-C", project.0.to_str().unwrap(), "init"]);
    run(&[
        "-C",
        project.0.to_str().unwrap(),
        "config",
        "user.email",
        "test@example.com",
    ]);
    run(&[
        "-C",
        project.0.to_str().unwrap(),
        "config",
        "user.name",
        "Test",
    ]);
    std::fs::write(project.0.join("tracked"), "tracked").unwrap();
    run(&["-C", project.0.to_str().unwrap(), "add", "tracked"]);
    run(&["-C", project.0.to_str().unwrap(), "commit", "-m", "initial"]);
    let worktree = worktree_parent.0.join("tree");
    run(&[
        "-C",
        project.0.to_str().unwrap(),
        "worktree",
        "add",
        "--detach",
        worktree.to_str().unwrap(),
        "HEAD",
    ]);
    let subdir = worktree.join("nested");
    std::fs::create_dir(&subdir).unwrap();
    let (mut bindings, _) = bound_bindings(&project);
    bindings.project.as_mut().unwrap().root = project.0.clone();
    bindings.current_dir = project.0.clone();
    let workspace = BoundDesktopWorkspace::new(
        7,
        0,
        bindings.clone(),
        Box::new(LocalApplication::new(bindings)),
        None,
        None,
        None,
    );
    assert_eq!(
        workspace
            .resolve_linked_launch_cwd(subdir.to_str().unwrap())
            .unwrap(),
        std::fs::canonicalize(subdir).unwrap()
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One exact task resource CAS lifecycle.
async fn task_transition_challenge_is_opaque_single_use_and_resource_revision_fenced() {
    let directory = TestDirectory::new("task-transition");
    let (bindings, task_id) = bound_bindings(&directory);
    let root = bindings.project.as_ref().unwrap().root.clone();
    let manager = Manager::new(&root, vec![profile(&root)], Arc::new(TestFactory))
        .await
        .unwrap();
    let terminal = TerminalRuntime::new(TerminalRuntimeConfig {
        generation: 7,
        project_root: root,
        manager,
        identity: Arc::new(TestIdentity::default()),
        events: Arc::new(TestEvents::default()),
        attachment_lease: std::time::Duration::from_secs(30),
    })
    .unwrap();
    let session = terminal.create(7, "shell-default", None, 24, 80).unwrap();
    terminal
        .associate(
            7,
            &session.session_id,
            TerminalAssociationPointer {
                version: 1,
                plan_id: 1,
                task_id,
            },
        )
        .unwrap();
    let workspace = BoundDesktopWorkspace::new(
        7,
        0,
        bindings.clone(),
        Box::new(LocalApplication::new(bindings)),
        Some(terminal.clone()),
        None,
        None,
    );

    let board = workspace
        .invoke("GetBoardV2", &[json!(7), json!(1)])
        .unwrap();
    assert_eq!(
        board["board"]["columns"][0]["tasks"][0]["linkedRuntime"]["terminals"],
        1
    );
    let detail = workspace
        .invoke("GetTaskDetailV2", &[json!(7), json!(task_id)])
        .unwrap();
    assert_eq!(detail["linkedRuntime"]["summary"]["liveTerminals"], 1);
    assert_eq!(
        detail["linkedRuntime"]["terminals"][0]["sessionId"],
        session.session_id
    );

    let pending_admission = workspace.begin_resource_admission().unwrap();
    assert_eq!(
        workspace
            .invoke(
                "MoveTaskV3",
                &[json!(7), json!(task_id), json!("doing"), json!("")],
            )
            .unwrap_err()
            .to_string(),
        "task transition must retry after resource admission completes"
    );
    drop(pending_admission);

    let challenge = workspace
        .invoke(
            "MoveTaskV3",
            &[json!(7), json!(task_id), json!("doing"), json!("")],
        )
        .unwrap();
    assert_eq!(challenge["applied"], false);
    assert_eq!(challenge["requiresConfirmation"], true);
    assert_eq!(challenge["confirmation"]["activeTerminals"], 1);
    assert_eq!(challenge["confirmation"]["activeAgents"], 0);
    let token = challenge["confirmation"]["token"].as_str().unwrap();
    assert_eq!(token.len(), 43);
    assert!(
        !serde_json::to_string(&challenge)
            .unwrap()
            .contains(&session.session_id)
    );

    let admission_attempt = workspace.begin_resource_admission().unwrap();
    drop(admission_attempt);
    assert_eq!(
        workspace
            .invoke(
                "MoveTaskV3",
                &[json!(7), json!(task_id), json!("doing"), json!(token)],
            )
            .unwrap_err()
            .to_string(),
        "task transition confirmation is invalid or stale"
    );
    let replacement = workspace
        .invoke(
            "MoveTaskV3",
            &[json!(7), json!(task_id), json!("doing"), json!("")],
        )
        .unwrap();
    let replacement_token = replacement["confirmation"]["token"].as_str().unwrap();
    let confirmed = workspace
        .invoke(
            "MoveTaskV3",
            &[
                json!(7),
                json!(task_id),
                json!("doing"),
                json!(replacement_token),
            ],
        )
        .unwrap();
    assert_eq!(confirmed["applied"], true);
    assert!(confirmed.get("confirmation").is_none());
    assert_eq!(
        workspace
            .invoke(
                "MoveTaskV3",
                &[json!(7), json!(task_id), json!("doing"), json!(token)],
            )
            .unwrap_err()
            .to_string(),
        "task transition confirmation is invalid or stale"
    );

    let stale = workspace
        .invoke(
            "MoveTaskV3",
            &[json!(7), json!(task_id), json!("done"), json!("")],
        )
        .unwrap();
    terminal.close(7, &session.session_id, true).unwrap();
    assert_eq!(
        workspace
            .invoke(
                "MoveTaskV3",
                &[
                    json!(7),
                    json!(task_id),
                    json!("done"),
                    stale["confirmation"]["token"].clone(),
                ],
            )
            .unwrap_err()
            .to_string(),
        "task transition confirmation is invalid or stale"
    );
    terminal.shutdown().await.unwrap();
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One table verifies exact preflight precedence.
async fn linked_launch_preflight_requires_an_exact_installed_agent_profile() {
    let directory = TestDirectory::new("linked-preflight");
    let (bindings, task_id) = bound_bindings(&directory);
    let root = bindings.project.as_ref().unwrap().root.clone();
    let mut agent_profile = profile(&root);
    agent_profile.id = "agent-test".to_owned();
    agent_profile.name = "Test agent".to_owned();
    agent_profile.kind = ptrack_terminal::ProfileKind::Agent;
    agent_profile.provider = "test".to_owned();
    let manager = Manager::new(
        &root,
        vec![profile(&root), agent_profile],
        Arc::new(TestFactory),
    )
    .await
    .unwrap();
    let terminal = TerminalRuntime::new(TerminalRuntimeConfig {
        generation: 7,
        project_root: root,
        manager,
        identity: Arc::new(TestIdentity::default()),
        events: Arc::new(TestEvents::default()),
        attachment_lease: Duration::from_secs(30),
    })
    .unwrap();
    let workspace = BoundDesktopWorkspace::new(
        7,
        0,
        bindings.clone(),
        Box::new(LocalApplication::new(bindings)),
        Some(terminal.clone()),
        None,
        None,
    );
    let pointer = json!({"version":1,"planId":1,"taskId":task_id});
    for profile_id in ["", " agent-test"] {
        assert_eq!(
            workspace
                .invoke(
                    "LaunchLinkedAgentV2",
                    &[
                        json!(7),
                        json!(profile_id),
                        json!(""),
                        json!(24),
                        json!(80),
                        pointer.clone(),
                    ],
                )
                .unwrap_err()
                .to_string(),
            "an installed agent profile is required"
        );
    }
    assert_eq!(
        workspace
            .invoke(
                "LaunchLinkedAgentV2",
                &[
                    json!(7),
                    json!("shell-default"),
                    json!(""),
                    json!(24),
                    json!(80),
                    pointer.clone(),
                ],
            )
            .unwrap_err()
            .to_string(),
        "terminal profile \"shell-default\" is not an agent"
    );
    assert_eq!(
        workspace
            .invoke(
                "LaunchLinkedAgentV2",
                &[
                    json!(7),
                    json!("agent-missing"),
                    json!(""),
                    json!(24),
                    json!(80),
                    pointer.clone(),
                ],
            )
            .unwrap_err()
            .to_string(),
        "installed agent profile \"agent-missing\" is unavailable"
    );
    assert_eq!(
        workspace
            .invoke(
                "LaunchLinkedAgentV2",
                &[
                    json!(7),
                    json!("agent-test"),
                    json!(""),
                    json!(24),
                    json!(80),
                    pointer,
                ],
            )
            .unwrap_err()
            .to_string(),
        "AgentRun registry is unavailable"
    );
    terminal.shutdown().await.unwrap();
}

#[tokio::test]
async fn workspace_confirmation_owns_and_expires_the_resource_admission_fence() {
    let directory = TestDirectory::new("admission-expiry");
    let (bindings, _) = bound_bindings(&directory);
    let root = bindings.project.as_ref().unwrap().root.clone();
    let manager = Manager::new(&root, vec![profile(&root)], Arc::new(TestFactory))
        .await
        .unwrap();
    let terminal = TerminalRuntime::new(TerminalRuntimeConfig {
        generation: 7,
        project_root: root,
        manager,
        identity: Arc::new(TestIdentity::default()),
        events: Arc::new(TestEvents::default()),
        attachment_lease: Duration::from_secs(30),
    })
    .unwrap();
    let session = terminal.create(7, "shell-default", None, 24, 80).unwrap();
    let workspace = Arc::new(BoundDesktopWorkspace::new(
        7,
        0,
        bindings.clone(),
        Box::new(LocalApplication::new(bindings)),
        Some(terminal.clone()),
        None,
        None,
    ));
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: Arc::new(FakeFactory::default()),
        event_sink: None,
        initial_workspace: Some(workspace.clone()),
        recent_projects: Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
        confirmation_ttl: Duration::from_millis(30),
    });
    let challenge = runtime
        .invoke(request("CloseProject", vec![json!("")]))
        .unwrap();
    assert_eq!(challenge["requiresConfirmation"], true);
    assert_eq!(
        workspace
            .invoke(
                "CreateTerminalV2",
                &[json!(7), json!("missing"), json!(""), json!(24), json!(80)],
            )
            .unwrap_err()
            .to_string(),
        "workspace resource admission is fenced"
    );
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        workspace
            .invoke(
                "CreateTerminalV2",
                &[json!(7), json!("missing"), json!(""), json!(24), json!(80)],
            )
            .unwrap_err()
            .to_string()
            .contains("profile \"missing\" is unavailable")
    );
    terminal.close(7, &session.session_id, true).unwrap();
    terminal.shutdown().await.unwrap();
    drop(runtime);
}
