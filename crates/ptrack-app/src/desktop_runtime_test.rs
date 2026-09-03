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
    Commit, IssueStatus, MemoryKind, Meta, Note, NoteTarget, Plan, PlanStatus, ProjectSnapshot,
    Severity, Task, TaskStatus, Timestamp,
};
use ptrack_store::{ActiveBinding, GlobalStore, ProjectStore, StoreKind};
use ptrack_terminal::{Manager, TerminalAssociationPointer};
use serde_json::{Value, json};

use super::desktop_runtime::{
    ActiveResourceSummary, BoundDesktopWorkspace, DesktopCommandRequest, DesktopRuntime,
    DesktopRuntimeConfig, DesktopWorkspace, DesktopWorkspaceFactory,
    RecentProjectOpenAuthorizationV1, RecentProjectRegistryCommitV1, RecentProjectRegistryStatusV1,
    RecentProjectsProvider, ResetApplicationStateResultV1, WorkspaceProject, WorkspaceStatus,
    agent_intelligence_for_task_result, allowed_desktop_commands, apply_preferences, board_view,
    capture_git_snapshot_with, confirm_linked_launch, heatmap_at, project_storage,
    record_last_project_in, repo_stats, reset_application_records, watch_workspace_data,
};
use crate::{
    AppError, AppResult, DesktopEvent, DesktopEventSink, DesktopInitializationService,
    DesktopUpdateService, InitializationCheckpointV1, InitializationOutcomeV1,
    InitializationStatusV1, InitializeProjectRequestV1, LocalApplication, ProjectEndpoint,
    ProjectTargetKindV1, ProjectTargetValidationV1, TerminalRuntime, TerminalRuntimeConfig,
    UnavailableUpdateService, UpdatePhase, UpdateState, WorkspaceBindings, set_identity_name,
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
        ptrack_store::protect_private_directory(&path).unwrap();
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

struct BlockingRecentProjects {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}

struct RecordingRecentRecovery {
    authorization: RecentProjectOpenAuthorizationV1,
    finishes: AtomicUsize,
    fail_finish: AtomicBool,
    completed: AtomicBool,
}

struct FailingSecondRecentAuthorization {
    authorization: RecentProjectOpenAuthorizationV1,
    calls: AtomicUsize,
}

impl RecentProjectsProvider for FailingSecondRecentAuthorization {
    fn recent_projects(&self) -> AppResult<Vec<Value>> {
        Ok(Vec::new())
    }

    fn authorize_recent_project_open(
        &self,
        _entry_id: &str,
        _base: &str,
        _canonical_root: &Path,
        _relocation_confirmation_token: &str,
    ) -> AppResult<RecentProjectOpenAuthorizationV1> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(self.authorization.clone())
        } else {
            Err(AppError::Message("recent-project-entry-stale".to_owned()))
        }
    }
}

impl RecentProjectsProvider for RecordingRecentRecovery {
    fn recent_projects(&self) -> AppResult<Vec<Value>> {
        Ok(Vec::new())
    }

    fn authorize_recent_project_open(
        &self,
        _entry_id: &str,
        _base: &str,
        _canonical_root: &Path,
        _relocation_confirmation_token: &str,
    ) -> AppResult<RecentProjectOpenAuthorizationV1> {
        let mut authorization = self.authorization.clone();
        authorization.already_completed = self.completed.load(Ordering::SeqCst);
        Ok(authorization)
    }

    fn finish_recent_project_open(
        &self,
        authorization: &RecentProjectOpenAuthorizationV1,
    ) -> AppResult<RecentProjectRegistryCommitV1> {
        if authorization.already_completed {
            return Ok(RecentProjectRegistryCommitV1 {
                base: authorization.base.clone(),
                status: RecentProjectRegistryStatusV1::Unchanged,
            });
        }
        self.finishes.fetch_add(1, Ordering::SeqCst);
        self.completed.store(true, Ordering::SeqCst);
        if self.fail_finish.load(Ordering::SeqCst) {
            return Err(AppError::Message("registry unavailable".to_owned()));
        }
        Ok(RecentProjectRegistryCommitV1 {
            base: authorization.base.clone(),
            status: RecentProjectRegistryStatusV1::Unchanged,
        })
    }
}

impl RecentProjectsProvider for BlockingRecentProjects {
    fn recent_projects(&self) -> AppResult<Vec<Value>> {
        self.entered.wait();
        self.release.wait();
        Ok(Vec::new())
    }
}

struct RecordingInitialization {
    root: PathBuf,
    requests: Mutex<Vec<InitializeProjectRequestV1>>,
    status: Mutex<Option<InitializationStatusV1>>,
    fail_mark: AtomicBool,
}

impl RecordingInitialization {
    fn new(root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            root,
            requests: Mutex::new(Vec::new()),
            status: Mutex::new(None),
            fail_mark: AtomicBool::new(false),
        })
    }
}

impl DesktopInitializationService for RecordingInitialization {
    fn validate_target(&self, _selected: &Path) -> AppResult<ProjectTargetValidationV1> {
        Ok(ProjectTargetValidationV1 {
            kind: ProjectTargetKindV1::New,
            canonical_root: self.root.to_string_lossy().into_owned(),
            operation_id: "A".repeat(43),
            reason: String::new(),
            initialization: None,
            goal: None,
            guide_choice: None,
        })
    }

    fn initialize(
        &self,
        request: &InitializeProjectRequestV1,
    ) -> AppResult<InitializationStatusV1> {
        let durable_request = {
            let mut requests = self.requests.lock().unwrap();
            let durable_request = requests.first().cloned();
            requests.push(request.clone());
            durable_request
        };
        if let Some(status) = self.status.lock().unwrap().clone()
            && status.operation_id == request.operation_id
            && status.canonical_root == request.root
            && status.checkpoint == InitializationCheckpointV1::DesktopBound
            && status.outcome == InitializationOutcomeV1::Complete
        {
            if durable_request.as_ref() != Some(request) {
                return Err(AppError::Message(
                    "initialization operation goal does not match its durable request".to_owned(),
                ));
            }
            return Ok(status);
        }
        let status = InitializationStatusV1 {
            operation_id: request.operation_id.clone(),
            canonical_root: self.root.to_string_lossy().into_owned(),
            checkpoint: InitializationCheckpointV1::GuideApplied,
            outcome: InitializationOutcomeV1::InProgress,
            error_kind: String::new(),
        };
        *self.status.lock().unwrap() = Some(status.clone());
        Ok(status)
    }

    fn status(&self, operation_id: &str) -> AppResult<InitializationStatusV1> {
        self.status
            .lock()
            .unwrap()
            .clone()
            .filter(|status| status.operation_id == operation_id)
            .ok_or_else(|| AppError::Message("initialization operation is unknown".to_owned()))
    }

    fn completed_initialization(&self) -> AppResult<Option<InitializationStatusV1>> {
        Ok(self
            .status
            .lock()
            .unwrap()
            .clone()
            .filter(|status| status.outcome == InitializationOutcomeV1::Complete))
    }

    fn mark_desktop_bound(&self, operation_id: &str) -> AppResult<InitializationStatusV1> {
        if self.fail_mark.load(Ordering::SeqCst) {
            return Err(AppError::Message(
                "desktop-bound checkpoint unavailable".to_owned(),
            ));
        }
        let mut status = self.status.lock().unwrap();
        let status = status
            .as_mut()
            .filter(|status| status.operation_id == operation_id)
            .ok_or_else(|| AppError::Message("initialization operation is unknown".to_owned()))?;
        status.checkpoint = InitializationCheckpointV1::DesktopBound;
        status.outcome = InitializationOutcomeV1::Complete;
        Ok(status.clone())
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

struct RecordingUpdates {
    calls: Mutex<Vec<String>>,
    state: Mutex<UpdateState>,
}

impl RecordingUpdates {
    fn new() -> Arc<Self> {
        let unavailable = UnavailableUpdateService::new("1.2.3");
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            state: Mutex::new(unavailable.state()),
        })
    }

    fn record(&self, value: String) -> UpdateState {
        self.calls.lock().unwrap().push(value);
        let mut state = self.state.lock().unwrap();
        state.revision = state.revision.saturating_add(1);
        state.clone()
    }
}

impl DesktopUpdateService for RecordingUpdates {
    fn start(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("start".to_owned());
        Ok(())
    }

    fn state(&self) -> UpdateState {
        self.state.lock().unwrap().clone()
    }

    fn set_automatic_checks(&self, enabled: bool) -> Result<UpdateState, String> {
        self.state.lock().unwrap().automatic_checks = enabled;
        Ok(self.record(format!("automatic:{enabled}")))
    }

    fn check_for_updates(&self) -> Result<UpdateState, String> {
        self.state.lock().unwrap().phase = UpdatePhase::Current;
        Ok(self.record("check".to_owned()))
    }

    fn download_update(&self, expected_version: &str) -> Result<UpdateState, String> {
        Ok(self.record(format!("download:{expected_version}")))
    }

    fn apply_update(&self, expected_version: &str) -> Result<UpdateState, String> {
        Ok(self.record(format!("apply:{expected_version}")))
    }

    fn cancel_operation(&self) -> UpdateState {
        self.record("cancel".to_owned())
    }

    fn shutdown(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("shutdown".to_owned());
        Ok(())
    }
}

fn request(method: &str, arguments: Vec<Value>) -> DesktopCommandRequest {
    DesktopCommandRequest {
        method: method.to_owned(),
        arguments,
    }
}

fn first_run_runtime(
    root: &Path,
    recents: Arc<dyn RecentProjectsProvider>,
) -> (
    Arc<DesktopRuntime>,
    Arc<FakeFactory>,
    Arc<RecordingInitialization>,
    Arc<Events>,
) {
    let factory = Arc::new(FakeFactory::default());
    let initialization = RecordingInitialization::new(root.to_path_buf());
    let events = Arc::new(Events::default());
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: factory.clone(),
        event_sink: Some(events.clone()),
        initial_workspace: None,
        recent_projects: recents,
        initialization: initialization.clone(),
        update_service: UnavailableUpdateService::new("test"),
        confirmation_ttl: Duration::from_secs(60),
    });
    (runtime, factory, initialization, events)
}

fn initialize_request(root: &Path, goal: &str) -> DesktopCommandRequest {
    request(
        "InitializeProjectV1",
        vec![json!({
            "operationId": "A".repeat(43),
            "root": root,
            "goal": goal,
        })],
    )
}

#[test]
fn initialize_project_v1_publishes_exactly_one_workspace_and_complete_status() {
    let directory = TestDirectory::new("initialize-publish");
    let project = directory.0.join("project");
    std::fs::create_dir(&project).unwrap();
    let (runtime, factory, initialization, events) = first_run_runtime(
        &project,
        Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
    );

    let initialize = initialize_request(&project, "ship the first run");
    let result = runtime.invoke(initialize.clone()).unwrap();

    assert_eq!(result["state"]["status"], "open");
    assert_eq!(result["state"]["generation"], 1);
    assert_eq!(result["initialization"]["checkpoint"], "desktop-bound");
    assert_eq!(result["initialization"]["outcome"], "complete");
    assert_eq!(
        factory.builds.lock().unwrap().as_slice(),
        &[(project.clone(), 1)]
    );
    assert_eq!(factory.built.lock().unwrap().len(), 1);
    assert_eq!(initialization.requests.lock().unwrap().len(), 1);
    assert_eq!(
        events.0.lock().unwrap().as_slice(),
        &[DesktopEvent::WorkspaceDataChanged(1)]
    );
    assert_eq!(runtime.invoke(initialize).unwrap(), result);
    assert_eq!(factory.builds.lock().unwrap().len(), 1);
    assert_eq!(initialization.requests.lock().unwrap().len(), 1);
    assert_eq!(events.0.lock().unwrap().len(), 1);

    runtime
        .invoke(request("CloseProject", vec![json!("")]))
        .unwrap();
    let reopened = runtime
        .invoke(request("OpenProject", vec![json!(project), json!("")]))
        .unwrap();
    assert_eq!(reopened["state"]["generation"], 2);
    let builds_before_replay = factory.builds.lock().unwrap().len();
    let events_before_replay = events.0.lock().unwrap().len();
    let replayed = runtime
        .invoke(initialize_request(&project, "ship the first run"))
        .unwrap();
    assert_eq!(replayed["initialization"], result["initialization"]);
    assert_eq!(replayed["state"], reopened["state"]);
    assert_eq!(factory.builds.lock().unwrap().len(), builds_before_replay);
    assert_eq!(events.0.lock().unwrap().len(), events_before_replay);
    assert_eq!(initialization.requests.lock().unwrap().len(), 2);
    assert_eq!(
        runtime
            .invoke(initialize_request(&project, "different goal"))
            .unwrap_err()
            .to_string(),
        "initialization operation goal does not match its durable request"
    );
    assert_eq!(
        runtime
            .invoke(request(
                "GetInitializationStatusV1",
                vec![json!("A".repeat(43))],
            ))
            .unwrap()["outcome"],
        "complete"
    );
}

#[test]
fn desktop_bound_failure_never_publishes_candidate_and_exact_retry_succeeds() {
    let directory = TestDirectory::new("initialize-desktop-bound-retry");
    let project = directory.0.join("project");
    std::fs::create_dir(&project).unwrap();
    let (runtime, factory, initialization, events) = first_run_runtime(
        &project,
        Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
    );
    initialization.fail_mark.store(true, Ordering::SeqCst);

    let error = runtime
        .invoke(initialize_request(&project, "resume desktop binding"))
        .unwrap_err();

    assert_eq!(error.to_string(), "desktop-bound checkpoint unavailable");
    assert_eq!(runtime.workspace_state().status, WorkspaceStatus::Error);
    assert_eq!(runtime.workspace_state().generation, 0);
    assert!(runtime.workspace_state().project.is_none());
    assert_eq!(factory.built.lock().unwrap().len(), 1);
    assert_eq!(
        factory.built.lock().unwrap()[0]
            .shutdowns
            .load(Ordering::SeqCst),
        1
    );
    assert!(events.0.lock().unwrap().is_empty());
    let interrupted = initialization.status(&"A".repeat(43)).unwrap();
    assert_eq!(
        interrupted.checkpoint,
        InitializationCheckpointV1::GuideApplied
    );
    assert_eq!(interrupted.outcome, InitializationOutcomeV1::InProgress);

    initialization.fail_mark.store(false, Ordering::SeqCst);
    let retried = runtime
        .invoke(initialize_request(&project, "resume desktop binding"))
        .unwrap();

    assert_eq!(retried["state"]["status"], "open");
    assert_eq!(retried["state"]["generation"], 1);
    assert_eq!(retried["initialization"]["checkpoint"], "desktop-bound");
    assert_eq!(factory.built.lock().unwrap().len(), 2);
    assert_eq!(
        events.0.lock().unwrap().as_slice(),
        &[DesktopEvent::WorkspaceDataChanged(1)]
    );
}

#[test]
fn initialize_project_v1_enforces_goal_byte_bounds_before_authority_change() {
    for (label, goal, accepted) in [
        ("empty", String::new(), false),
        ("one", "x".to_owned(), true),
        ("max", "x".repeat(4_096), true),
        ("too-long", "x".repeat(4_097), false),
    ] {
        let directory = TestDirectory::new(label);
        let project = directory.0.join("project");
        std::fs::create_dir(&project).unwrap();
        let (runtime, _factory, initialization, _events) = first_run_runtime(
            &project,
            Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
        );

        let result = runtime.invoke(initialize_request(&project, &goal));
        assert_eq!(result.is_ok(), accepted, "goal case {label}");
        assert_eq!(
            initialization.requests.lock().unwrap().len(),
            usize::from(accepted),
            "goal case {label}"
        );
        if !accepted {
            assert_eq!(runtime.workspace_state().status, WorkspaceStatus::Welcome);
            assert_eq!(
                result.unwrap_err().to_string(),
                "project goal must contain 1 to 4096 UTF-8 bytes"
            );
        }
    }
}

#[test]
fn get_pending_initialization_v1_is_strict_and_omits_absent_metadata() {
    let directory = TestDirectory::new("pending-initialization-command");
    let project = directory.0.join("project");
    std::fs::create_dir(&project).unwrap();
    let (runtime, _factory, _initialization, _events) = first_run_runtime(
        &project,
        Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
    );

    assert_eq!(
        runtime
            .invoke(request("GetPendingInitializationV1", Vec::new()))
            .unwrap(),
        json!({ "pending": false })
    );
    assert!(
        runtime
            .invoke(request("GetPendingInitializationV1", vec![Value::Null],))
            .is_err()
    );
}

#[test]
fn concurrent_completed_status_checks_reload_and_bind_exactly_once() {
    let directory = TestDirectory::new("completed-initialization-concurrency");
    let project = directory.0.join("project");
    std::fs::create_dir(&project).unwrap();
    let (runtime, factory, initialization, events) = first_run_runtime(
        &project,
        Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
    );
    *initialization.status.lock().unwrap() = Some(InitializationStatusV1 {
        operation_id: "A".repeat(43),
        canonical_root: project.to_string_lossy().into_owned(),
        checkpoint: InitializationCheckpointV1::DesktopBound,
        outcome: InitializationOutcomeV1::Complete,
        error_kind: String::new(),
    });
    let barrier = Arc::new(Barrier::new(3));
    let invoke = |runtime: Arc<DesktopRuntime>, barrier: Arc<Barrier>, method: &'static str| {
        thread::spawn(move || {
            barrier.wait();
            runtime.invoke(request(
                method,
                if method == "GetInitializationStatusV1" {
                    vec![json!("A".repeat(43))]
                } else {
                    Vec::new()
                },
            ))
        })
    };
    let pending = invoke(
        Arc::clone(&runtime),
        Arc::clone(&barrier),
        "GetPendingInitializationV1",
    );
    let status = invoke(
        Arc::clone(&runtime),
        Arc::clone(&barrier),
        "GetInitializationStatusV1",
    );
    barrier.wait();

    assert_eq!(
        pending.join().unwrap().unwrap(),
        json!({ "pending": false })
    );
    assert_eq!(
        status.join().unwrap().unwrap()["checkpoint"],
        "desktop-bound"
    );
    assert_eq!(factory.built.lock().unwrap().len(), 1);
    assert_eq!(runtime.workspace_state().status, WorkspaceStatus::Open);
    assert_eq!(runtime.workspace_state().generation, 1);
    assert_eq!(
        events.0.lock().unwrap().as_slice(),
        &[DesktopEvent::WorkspaceDataChanged(1)]
    );
}

#[test]
fn initialization_drains_an_active_native_call_and_rejects_shutdown_and_new_calls() {
    let directory = TestDirectory::new("initialize-drain");
    let project = directory.0.join("project");
    std::fs::create_dir(&project).unwrap();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let recents = Arc::new(BlockingRecentProjects {
        entered: entered.clone(),
        release: release.clone(),
    });
    let (runtime, factory, _initialization, _events) = first_run_runtime(&project, recents);

    let recent_runtime = runtime.clone();
    let recent =
        thread::spawn(move || recent_runtime.invoke(request("GetRecentProjects", Vec::new())));
    entered.wait();

    let (finished, completion) = channel();
    let initialize_runtime = runtime.clone();
    let initialize_project = project.clone();
    let initialize = thread::spawn(move || {
        let result = initialize_runtime.invoke(initialize_request(
            &initialize_project,
            "drain before transition",
        ));
        finished.send(()).unwrap();
        result
    });

    let deadline = Instant::now() + Duration::from_secs(1);
    while runtime.workspace_state().status != WorkspaceStatus::Loading && Instant::now() < deadline
    {
        thread::yield_now();
    }
    assert_eq!(runtime.workspace_state().status, WorkspaceStatus::Loading);
    assert!(matches!(
        completion.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert_eq!(
        runtime
            .invoke(request("ValidateProjectTargetV1", vec![json!(project)]))
            .unwrap_err()
            .to_string(),
        "runtime authority is changing"
    );
    assert_eq!(
        runtime.begin_shutdown().unwrap_err().to_string(),
        "runtime authority is changing"
    );
    assert!(factory.builds.lock().unwrap().is_empty());

    release.wait();
    assert!(recent.join().unwrap().is_ok());
    assert!(initialize.join().unwrap().is_ok());
    completion.recv().unwrap();
    assert_eq!(runtime.workspace_state().status, WorkspaceStatus::Open);
    assert_eq!(factory.builds.lock().unwrap().len(), 1);
}

#[test]
fn desktop_update_commands_delegate_exact_arguments_and_return_full_state() {
    let updates = RecordingUpdates::new();
    let mut config = DesktopRuntimeConfig::unavailable("1.2.3");
    config.update_service = updates.clone();
    let runtime = DesktopRuntime::new(config);
    let initial = runtime
        .invoke(request("GetUpdateState", Vec::new()))
        .unwrap();
    assert_eq!(initial["currentVersion"], "1.2.3");
    assert_eq!(initial["downloadedBytes"], 0);
    assert_eq!(initial["checksumVerified"], false);
    runtime
        .invoke(request("SetAutomaticUpdateChecks", vec![json!(true)]))
        .unwrap();
    runtime
        .invoke(request("CheckForUpdates", Vec::new()))
        .unwrap();
    runtime
        .invoke(request("DownloadUpdate", vec![json!("1.2.4")]))
        .unwrap();
    runtime
        .invoke(request("ApplyUpdate", vec![json!("1.2.4")]))
        .unwrap();
    runtime
        .invoke(request("CancelUpdateOperation", Vec::new()))
        .unwrap();
    assert_eq!(
        *updates.calls.lock().unwrap(),
        [
            "start",
            "automatic:true",
            "check",
            "download:1.2.4",
            "apply:1.2.4",
            "cancel"
        ]
    );
    runtime.begin_shutdown().unwrap();
    assert_eq!(updates.calls.lock().unwrap().last().unwrap(), "shutdown");
}

#[test]
#[allow(clippy::too_many_lines)] // Full 93-command freeze fixture is intentionally explicit.
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
            "ClaimTerminalStream",
            "CloseProject",
            "CloseTerminal",
            "CloseTerminalV2",
            "CopyPlanV1",
            "CreateFirstPlanV1",
            "CreateFirstTaskV1",
            "CreateTerminal",
            "CreateTerminalV2",
            "DeletePlanV1",
            "DisableCapabilityV2",
            "DismissAgentWorkflowV2",
            "DownloadUpdate",
            "EnableCapabilityV2",
            "ExpireCapabilityV2",
            "ForgetRecentProjectV1",
            "GetActivityHeatmapV2",
            "GetAgentIntelligenceV2",
            "GetAgentRunsV2",
            "GetBoard",
            "GetBoardV2",
            "GetCapabilitiesV2",
            "GetCapabilityAuditsV2",
            "GetDiagnosticsReport",
            "GetInitializationStatusV1",
            "GetLayoutState",
            "GetPendingInitializationV1",
            "GetPreferences",
            "GetRecentProjects",
            "GetRecentProjectsV1",
            "GetRepoStatsV1",
            "GetTaskDetailV2",
            "GetTerminalProfiles",
            "GetTerminalProfilesV2",
            "GetTerminalWindowTab",
            "GetUpdateState",
            "GetWorkspaceSnapshot",
            "GetWorkspaceState",
            "InitializeProjectV1",
            "InstallShellCommand",
            "LaunchLinkedAgentV2",
            "ListProjectsV1",
            "MovePlanV1",
            "MoveTask",
            "MoveTaskV2",
            "MoveTaskV3",
            "MutateTerminalAssociationV2",
            "OpenHelpDestination",
            "OpenProject",
            "OpenRecentProjectV1",
            "OpenTerminalWindow",
            "PickProjectDirectory",
            "PrepareAgentWorkflowV2",
            "PreviewAgentHandoffV2",
            "PreviewCapabilityV2",
            "PreviewProjectGuideV1",
            "PreviewTerminalWritebackV2",
            "RemoveCapabilityV2",
            "RenamePlanV1",
            "RenameTask",
            "RenameTaskV2",
            "ResetApplicationState",
            "ResetPreferences",
            "ResetWindowLayout",
            "ResizeTerminal",
            "ResizeTerminalV2",
            "ResolveRecentProjectV1",
            "RollbackLinkedAgentLaunchV2",
            "SaveCapabilityV2",
            "SearchV2",
            "SendAgentHandoffV2",
            "SetAgentTaskOwnershipV2",
            "SetAgentWorktreeV2",
            "SetAutomaticUpdateChecks",
            "SetLayoutState",
            "SetPreferences",
            "SetTerminalWindowTab",
            "StartFirstTaskV1",
            "TestCapabilityV2",
            "ValidateProjectTargetV1",
            "ValidateTerminalCWDsV2",
            "WriteTerminalMemoryV2",
        ]
    );
    assert!(commands.windows(2).all(|w| w[0] < w[1]), "not sorted");

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
                vec![json!("project-recovery")],
            ))
            .unwrap(),
        json!("https://ro-ag.github.io/ptrack/help/troubleshooting/")
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
fn preference_and_diagnostics_commands_dispatch_above_the_workspace() {
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig::unavailable("test"));
    // A wrong arity is rejected before any store is opened, which proves the
    // application-scoped commands are answered by the application-level
    // dispatch instead of falling through to a project workspace.
    for (method, arguments) in [
        ("GetPreferences", vec![json!({})]),
        ("SetPreferences", Vec::new()),
        ("ResetPreferences", vec![json!({})]),
        ("GetDiagnosticsReport", vec![json!({})]),
        ("GetLayoutState", vec![json!({})]),
        ("SetLayoutState", Vec::new()),
        ("ResetWindowLayout", vec![json!({})]),
        ("ResetApplicationState", vec![json!({})]),
    ] {
        let error = runtime
            .invoke(request(method, arguments))
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with(&format!("{method} requires exactly")),
            "{error}"
        );
    }
}

fn global_store(directory: &TestDirectory, database_id: &str) -> GlobalStore {
    let database = directory.0.join("global.redb");
    GlobalStore::create_new(
        &database,
        ActiveBinding {
            generation: 1,
            database_id: database_id.to_owned(),
            kind: StoreKind::Global,
            canonical_path: database.clone(),
        },
    )
    .unwrap()
}

fn recorded_root(store: &GlobalStore) -> Option<String> {
    crate::preferences::preferences(store)
        .preferences
        .startup
        .last_project_root
}

/// Ticking the startup opt-in while a project is open must record that project
/// there and then. Otherwise the setting does nothing until the user happens to
/// reopen the same project once, and the next launch lands on Welcome with
/// nothing explaining why.
#[test]
fn opting_in_while_a_project_is_open_records_that_project_immediately() {
    let directory = TestDirectory::new("startup-opt-in");
    let store = global_store(&directory, "startup-opt-in");
    let opt_in = json!({ "startup": { "restoreLastProject": true } });

    // While opted out, an open project is nobody's business to persist.
    apply_preferences(
        &store,
        &json!({ "appearance": { "theme": "dark" } }),
        Some("/work/app"),
    )
    .unwrap();
    assert_eq!(recorded_root(&store), None);

    let document = apply_preferences(&store, &opt_in, Some("/work/app")).unwrap();
    assert_eq!(
        document.preferences.startup.last_project_root.as_deref(),
        Some("/work/app")
    );
    assert_eq!(recorded_root(&store).as_deref(), Some("/work/app"));

    // The opt-in is already on, so a later patch never re-records: it must not
    // resurrect a root an explicit close just cleared.
    record_last_project_in(&store, &Value::Null);
    apply_preferences(&store, &opt_in, Some("/work/other")).unwrap();
    assert_eq!(recorded_root(&store), None);
}

/// Opting in from Welcome has nothing to record, and must not clear a root the
/// user never closed.
#[test]
fn opting_in_with_no_project_open_leaves_the_stored_root_alone() {
    let directory = TestDirectory::new("startup-opt-in-welcome");
    let store = global_store(&directory, "startup-opt-in-welcome");
    crate::preferences::set_preferences(
        &store,
        &json!({ "startup": { "restoreLastProject": true, "lastProjectRoot": "/work/app" } }),
    )
    .unwrap();
    crate::preferences::set_preferences(
        &store,
        &json!({ "startup": { "restoreLastProject": false } }),
    )
    .unwrap();

    apply_preferences(
        &store,
        &json!({ "startup": { "restoreLastProject": true } }),
        None,
    )
    .unwrap();
    assert_eq!(recorded_root(&store).as_deref(), Some("/work/app"));
}

/// The clear is unconditional while the write stays gated: opting out and then
/// closing a project must not strand its root for a later opt-in to reopen.
#[test]
fn an_explicit_close_clears_the_recorded_root_even_while_opted_out() {
    let directory = TestDirectory::new("startup-clear");
    let store = global_store(&directory, "startup-clear");
    crate::preferences::set_preferences(
        &store,
        &json!({ "startup": { "restoreLastProject": true, "lastProjectRoot": "/work/app" } }),
    )
    .unwrap();
    crate::preferences::set_preferences(
        &store,
        &json!({ "startup": { "restoreLastProject": false } }),
    )
    .unwrap();

    assert!(record_last_project_in(&store, &Value::Null).is_some());
    assert_eq!(recorded_root(&store), None);

    // A path is still never persisted without the opt-in.
    assert!(record_last_project_in(&store, &json!("/work/other")).is_none());
    assert_eq!(recorded_root(&store), None);
}

#[test]
fn a_full_reset_clears_every_app_record_and_leaves_project_data_alone() {
    let directory = TestDirectory::new("reset-application-state");
    let database = directory.0.join("global.redb");
    let store = GlobalStore::create_new(
        &database,
        ActiveBinding {
            generation: 1,
            database_id: "reset-test".to_owned(),
            kind: StoreKind::Global,
            canonical_path: database.clone(),
        },
    )
    .unwrap();
    let keys: [&[u8]; 4] = [
        b"preferences",
        crate::update_preference_key(),
        b"window-state",
        b"layout-state",
    ];
    for key in keys {
        store.set_config(key, b"{}").unwrap();
    }
    let project = store
        .register_project("kept", directory.0.join("kept-project"))
        .unwrap();

    let records = reset_application_records(&store).unwrap();

    for key in keys {
        assert!(store.config(key).unwrap().is_empty(), "record survived");
    }
    // Project data is out of scope: the recents registry row is untouched.
    assert_eq!(store.project(&project.path).unwrap(), Some(project));
    // The dialog reports exactly those four keys, in camelCase.
    assert_eq!(
        serde_json::to_value(ResetApplicationStateResultV1 {
            records,
            capability_grants: 2,
        })
        .unwrap(),
        json!({
            "records": ["preferences", "updates.auto-check", "window-state", "layout-state"],
            "capabilityGrants": 2
        })
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
            active_plans: Vec::new(),
            actors: Vec::new(),
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
            actor: None,
            ulid: None,
        }],
        vec![Commit {
            id: 1,
            sha: "abc".to_owned(),
            subject: "boundary".to_owned(),
            plan_id: 0,
            task_id: 0,
            created_at: event,
            actor: None,
            ulid: None,
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
fn board_view_carries_dep_edges_and_their_computed_open_subset() {
    fn plan(id: u64, status: PlanStatus, deps: Vec<u64>) -> Plan {
        Plan {
            id,
            title: format!("Plan {id}"),
            status,
            milestone_id: 0,
            order: 0,
            created_at: Timestamp::Zero,
            updated_at: Timestamp::Zero,
            hold_reason: None,
            actor: None,
            ulid: None,
            claim_owner: None,
            claim_epoch: 0,
            claim_conflict: false,
            deps,
        }
    }
    fn task(id: u64, status: TaskStatus, deps: Vec<u64>) -> Task {
        Task {
            id,
            plan_id: 1,
            title: format!("Task {id}"),
            status,
            order: 0,
            created_at: Timestamp::Zero,
            updated_at: Timestamp::Zero,
            hold_reason: None,
            actor: None,
            ulid: None,
            deps,
        }
    }
    let snapshot = ProjectSnapshot::new(
        Meta {
            goal: String::new(),
            summary: String::new(),
            active_plan: 1,
            created_at: Timestamp::Zero,
            updated_at: Timestamp::Zero,
            format_version: 1,
            last_write_version: String::new(),
            active_plans: Vec::new(),
            actors: Vec::new(),
        },
        Vec::new(),
        vec![
            plan(1, PlanStatus::Active, vec![2, 3]),
            plan(2, PlanStatus::Active, Vec::new()),
            plan(3, PlanStatus::Done, Vec::new()),
        ],
        vec![
            task(1, TaskStatus::Todo, vec![2, 3]),
            task(2, TaskStatus::Todo, Vec::new()),
            task(3, TaskStatus::Done, Vec::new()),
        ],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let board =
        serde_json::to_value(board_view(&snapshot, "example".to_owned(), 1).unwrap()).unwrap();

    // Task 1 lists both edges; only the not-done target stays in the open subset.
    let card = &board["columns"][0]["tasks"][0];
    assert_eq!(card["id"], 1);
    assert_eq!(card["deps"], json!([2, 3]));
    assert_eq!(card["depsOpen"], json!([2]));
    // Dep-free records omit the fields entirely.
    let bare = &board["columns"][0]["tasks"][1];
    assert_eq!(bare["id"], 2);
    assert!(bare.get("deps").is_none(), "{bare}");
    assert!(bare.get("depsOpen").is_none(), "{bare}");

    // Plan 1 waits only on the still-active plan 2.
    assert_eq!(board["plans"][0]["depsOpen"], json!([2]));
    assert!(board["plans"][1].get("depsOpen").is_none(), "{board}");
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
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("0.22.0"),
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
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
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
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
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

/// Contract section 2: the window assignment map is reachable through the
/// bridge with its exact response shapes, and closing the project takes every
/// terminal window with it.
#[test]
#[allow(clippy::too_many_lines)] // One bridge round-trip per contract shape.
fn terminal_window_assignments_answer_the_bridge_and_expire_with_the_project() {
    let welcome = DesktopRuntime::new(DesktopRuntimeConfig::unavailable("test"));
    assert_eq!(
        welcome
            .invoke(request(
                "OpenTerminalWindow",
                vec![json!(["session-a"]), json!({ "id": "tab-1" })]
            ))
            .unwrap_err()
            .to_string(),
        "no project workspace is open"
    );

    let root = TestDirectory::new("terminal-windows");
    let workspace = FakeWorkspace::new(&root.0, 1);
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: Arc::new(FakeFactory::default()),
        event_sink: None,
        initial_workspace: Some(workspace),
        recent_projects: Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
        confirmation_ttl: Duration::from_secs(60),
    });
    assert_eq!(
        runtime
            .invoke(request(
                "OpenTerminalWindow",
                vec![json!(["session-a", "session-b"]), json!({ "id": "tab-1" })]
            ))
            .unwrap(),
        json!({ "label": "terminal-1" })
    );
    assert_eq!(
        runtime
            .invoke(request("GetTerminalWindowTab", vec![json!("terminal-1")]))
            .unwrap(),
        json!({ "sessions": ["session-a", "session-b"], "shape": { "id": "tab-1" } })
    );
    // A split inside the window replaces the assignment through the bridge.
    assert_eq!(
        runtime
            .invoke(request(
                "SetTerminalWindowTab",
                vec![
                    json!("terminal-1"),
                    json!(["session-a", "session-b", "session-c"]),
                    json!({ "id": "tab-1", "split": true })
                ]
            ))
            .unwrap(),
        json!({})
    );
    assert_eq!(
        runtime
            .invoke(request("GetTerminalWindowTab", vec![json!("terminal-1")]))
            .unwrap(),
        json!({
            "sessions": ["session-a", "session-b", "session-c"],
            "shape": { "id": "tab-1", "split": true }
        })
    );
    assert_eq!(
        runtime
            .invoke(request(
                "SetTerminalWindowTab",
                vec![json!("terminal-9"), json!(["session-z"]), json!({})]
            ))
            .unwrap_err()
            .to_string(),
        "no terminal window has that label"
    );
    // A tab shape must be an object; anything else could only render nothing.
    assert_eq!(
        runtime
            .invoke(request(
                "OpenTerminalWindow",
                vec![json!(["session-z"]), json!("shape")]
            ))
            .unwrap_err()
            .to_string(),
        "OpenTerminalWindow requires an object tab shape"
    );
    // An unknown label is null, not an error, so a stale window closes cleanly.
    assert_eq!(
        runtime
            .invoke(request("GetTerminalWindowTab", vec![json!("terminal-9")]))
            .unwrap(),
        json!({ "sessions": null, "shape": null })
    );
    // The assignment is the token the shell pops a session back in on: the
    // window's destruction clears it and reports the session exactly once, so a
    // second destruction — or a drain that already took it — reports nothing
    // and cannot hand the same session to the main window twice.
    assert_eq!(
        runtime
            .close_terminal_window("terminal-1")
            .map(|tab| tab.sessions),
        Some(vec![
            "session-a".to_owned(),
            "session-b".to_owned(),
            "session-c".to_owned()
        ])
    );
    assert_eq!(runtime.close_terminal_window("terminal-1"), None);
    assert_eq!(runtime.close_terminal_window("terminal-9"), None);
    assert_eq!(
        runtime
            .invoke(request("OpenTerminalWindow", Vec::new()))
            .unwrap_err()
            .to_string(),
        "OpenTerminalWindow requires exactly 2 arguments"
    );

    // A live assignment survives every other command and dies with the project.
    runtime
        .invoke(request(
            "OpenTerminalWindow",
            vec![json!(["session-d"]), json!({ "id": "tab-2" })],
        ))
        .unwrap();
    assert!(runtime.expire_terminal_windows().is_empty());
    runtime
        .invoke(request("CloseProject", vec![json!("")]))
        .unwrap();
    assert_eq!(runtime.expire_terminal_windows(), ["terminal-2"]);
    assert_eq!(
        runtime
            .invoke(request("GetTerminalWindowTab", vec![json!("terminal-2")]))
            .unwrap(),
        json!({ "sessions": null, "shape": null })
    );
    // The project switch already took the assignment, so the destruction of the
    // window it closed pops nothing back into a workspace that is gone.
    assert_eq!(runtime.close_terminal_window("terminal-2"), None);
    assert_eq!(runtime.drain_terminal_windows(), Vec::<String>::new());
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
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
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
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
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
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
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
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
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
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
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
fn recent_open_uses_authorized_root_and_registry_failure_preserves_open_success() {
    let requested = TestDirectory::new("recent-requested");
    let authorized = TestDirectory::new("recent-authorized");
    let factory = Arc::new(FakeFactory::default());
    let provider = Arc::new(RecordingRecentRecovery {
        authorization: RecentProjectOpenAuthorizationV1 {
            entry_id: "E".repeat(43),
            base: "B".repeat(43),
            canonical_root: authorized.0.to_string_lossy().into_owned(),
            name: "authorized".to_owned(),
            relocation_confirmation_token: String::new(),
            already_completed: false,
        },
        finishes: AtomicUsize::new(0),
        fail_finish: AtomicBool::new(true),
        completed: AtomicBool::new(false),
    });
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: factory.clone(),
        event_sink: None,
        initial_workspace: None,
        recent_projects: provider.clone(),
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
        confirmation_ttl: Duration::from_secs(60),
    });
    let result = runtime
        .invoke(request(
            "OpenRecentProjectV1",
            vec![
                json!("E".repeat(43)),
                json!("B".repeat(43)),
                json!(requested.0.to_string_lossy()),
                json!(""),
                json!(""),
            ],
        ))
        .unwrap();
    assert_eq!(
        result["open"]["state"]["project"]["root"],
        authorized.0.to_string_lossy().as_ref()
    );
    assert_eq!(result["registryStatus"], "stale");
    assert_eq!(
        result["open"]["warning"],
        "recent-project registry update is incomplete"
    );
    assert_eq!(factory.builds.lock().unwrap()[0].0, authorized.0);
    assert_eq!(provider.finishes.load(Ordering::SeqCst), 1);
}

#[test]
fn concurrent_identical_recent_opens_build_and_commit_once() {
    let target = TestDirectory::new("recent-concurrent");
    let factory = Arc::new(FakeFactory::default());
    let provider = Arc::new(RecordingRecentRecovery {
        authorization: RecentProjectOpenAuthorizationV1 {
            entry_id: "E".repeat(43),
            base: "B".repeat(43),
            canonical_root: target.0.to_string_lossy().into_owned(),
            name: "target".to_owned(),
            relocation_confirmation_token: String::new(),
            already_completed: false,
        },
        finishes: AtomicUsize::new(0),
        fail_finish: AtomicBool::new(false),
        completed: AtomicBool::new(false),
    });
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: factory.clone(),
        event_sink: None,
        initial_workspace: None,
        recent_projects: provider.clone(),
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
        confirmation_ttl: Duration::from_secs(60),
    });
    let arguments = vec![
        json!("E".repeat(43)),
        json!("B".repeat(43)),
        json!(target.0.to_string_lossy()),
        json!(""),
        json!(""),
    ];
    let start = Arc::new(Barrier::new(3));
    let handles = (0..2)
        .map(|_| {
            let runtime = runtime.clone();
            let arguments = arguments.clone();
            let start = start.clone();
            thread::spawn(move || {
                start.wait();
                runtime.invoke(request("OpenRecentProjectV1", arguments))
            })
        })
        .collect::<Vec<_>>();
    start.wait();
    for handle in handles {
        assert_eq!(
            handle.join().unwrap().unwrap()["open"]["state"]["generation"],
            1
        );
    }
    assert_eq!(factory.builds.lock().unwrap().len(), 1);
    assert_eq!(provider.finishes.load(Ordering::SeqCst), 1);
}

#[test]
fn recent_open_resource_challenge_never_mutates_registry_before_confirmation() {
    let current = TestDirectory::new("recent-current");
    let target = TestDirectory::new("recent-target");
    let initial = FakeWorkspace::new(&current.0, 1);
    initial.resources.lock().unwrap().terminals = 1;
    let provider = Arc::new(RecordingRecentRecovery {
        authorization: RecentProjectOpenAuthorizationV1 {
            entry_id: "E".repeat(43),
            base: "B".repeat(43),
            canonical_root: target.0.to_string_lossy().into_owned(),
            name: "target".to_owned(),
            relocation_confirmation_token: "R".repeat(43),
            already_completed: false,
        },
        finishes: AtomicUsize::new(0),
        fail_finish: AtomicBool::new(false),
        completed: AtomicBool::new(false),
    });
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: Arc::new(FakeFactory::default()),
        event_sink: None,
        initial_workspace: Some(initial),
        recent_projects: provider.clone(),
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
        confirmation_ttl: Duration::from_secs(60),
    });
    let arguments = vec![
        json!("E".repeat(43)),
        json!("B".repeat(43)),
        json!(target.0.to_string_lossy()),
        json!("R".repeat(43)),
        json!(""),
    ];
    let challenged = runtime
        .invoke(request("OpenRecentProjectV1", arguments.clone()))
        .unwrap();
    assert_eq!(challenged["open"]["requiresConfirmation"], true);
    assert_eq!(challenged["registryStatus"], "unchanged");
    assert_eq!(provider.finishes.load(Ordering::SeqCst), 0);

    let mut confirmed = arguments;
    confirmed[4] = challenged["open"]["confirmationToken"].clone();
    let opened = runtime
        .invoke(request("OpenRecentProjectV1", confirmed))
        .unwrap();
    assert_eq!(opened["open"]["requiresConfirmation"], false);
    assert_eq!(provider.finishes.load(Ordering::SeqCst), 1);
}

#[test]
fn recent_open_reauthorization_failure_cancels_the_exact_workspace_confirmation() {
    let current = TestDirectory::new("recent-cancel-current");
    let target = TestDirectory::new("recent-cancel-target");
    let initial = FakeWorkspace::new(&current.0, 1);
    initial.resources.lock().unwrap().terminals = 1;
    let provider = Arc::new(FailingSecondRecentAuthorization {
        authorization: RecentProjectOpenAuthorizationV1 {
            entry_id: "E".repeat(43),
            base: "B".repeat(43),
            canonical_root: target.0.to_string_lossy().into_owned(),
            name: "target".to_owned(),
            relocation_confirmation_token: String::new(),
            already_completed: false,
        },
        calls: AtomicUsize::new(0),
    });
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: Arc::new(FakeFactory::default()),
        event_sink: None,
        initial_workspace: Some(initial),
        recent_projects: provider,
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
        confirmation_ttl: Duration::from_secs(60),
    });
    let mut arguments = vec![
        json!("E".repeat(43)),
        json!("B".repeat(43)),
        json!(target.0.to_string_lossy()),
        json!(""),
        json!(""),
    ];
    let challenged = runtime
        .invoke(request("OpenRecentProjectV1", arguments.clone()))
        .unwrap();
    let token = challenged["open"]["confirmationToken"]
        .as_str()
        .unwrap()
        .to_owned();
    arguments[4] = json!(token);
    assert_eq!(
        runtime
            .invoke(request("OpenRecentProjectV1", arguments))
            .unwrap_err()
            .to_string(),
        "recent-project-entry-stale"
    );
    assert_eq!(
        runtime
            .invoke(request("CancelWorkspaceChange", vec![json!(token)]))
            .unwrap_err()
            .to_string(),
        "invalid or expired workspace confirmation"
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
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
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
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
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

fn empty_bound_workspace(directory: &TestDirectory) -> (BoundDesktopWorkspace, WorkspaceBindings) {
    let root = directory.0.join("empty-project");
    let home = directory.0.join("empty-home");
    std::fs::create_dir_all(root.join(".ptrack")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let project_database = root.join(".ptrack/ptrack.redb");
    let global_database = home.join("global.redb");
    let project_binding = binding(&project_database, StoreKind::Project, "empty-project-7");
    let global_binding = binding(&global_database, StoreKind::Global, "empty-global-7");
    drop(ProjectStore::create_new(&project_database, project_binding.clone(), "test").unwrap());
    drop(GlobalStore::create_new(&global_database, global_binding.clone()).unwrap());
    let bindings = WorkspaceBindings {
        current_dir: root.clone(),
        project: Some(ProjectEndpoint {
            root,
            database: project_database,
            binding: project_binding,
        }),
        global_database,
        global_binding,
        global_home: home,
        writer_version: "test".to_owned(),
    };
    (
        BoundDesktopWorkspace::new(
            7,
            0,
            bindings.clone(),
            Box::new(LocalApplication::new(bindings.clone())),
            None,
            None,
            None,
        ),
        bindings,
    )
}

#[test]
#[allow(clippy::too_many_lines)] // One wire lifecycle proves fencing, replay, and CAS semantics.
fn first_run_workspace_mutations_are_exact_fenced_and_idempotent() {
    let directory = TestDirectory::new("first-run-mutations");
    let (workspace, bindings) = empty_bound_workspace(&directory);
    for generation in [0, 8] {
        assert!(
            workspace
                .invoke("CreateFirstPlanV1", &[json!(generation), json!("Plan")],)
                .unwrap_err()
                .to_string()
                .contains("stale workspace generation")
        );
    }
    assert!(
        workspace
            .invoke(
                "CreateFirstPlanV1",
                &[json!(7), json!("Plan"), json!("extra")],
            )
            .is_err()
    );

    let created = workspace
        .invoke("CreateFirstPlanV1", &[json!(7), json!("  First plan  ")])
        .unwrap();
    assert_eq!(created["plan"]["id"], 1);
    assert_eq!(created["plan"]["title"], "First plan");
    assert_eq!(created["plan"]["status"], "active");
    assert_eq!(created["state"]["status"], "open");
    assert_eq!(created["state"]["generation"], 7);
    assert_eq!(
        workspace
            .invoke("CreateFirstPlanV1", &[json!(7), json!("First plan")],)
            .unwrap(),
        created
    );
    assert!(
        workspace
            .invoke("CreateFirstPlanV1", &[json!(7), json!("Different plan")],)
            .is_err()
    );

    assert!(
        workspace
            .invoke("CreateFirstTaskV1", &[json!(7), json!(1), json!("   ")],)
            .is_err()
    );
    let store = ProjectStore::open_existing(
        &bindings.project.as_ref().unwrap().database,
        &bindings.project.as_ref().unwrap().binding,
        "test",
    )
    .unwrap();
    assert_eq!(store.plans().unwrap().len(), 1);
    assert_eq!(store.meta().unwrap().active_plan, 1);
    assert!(store.tasks().unwrap().is_empty());
    drop(store);

    let task = workspace
        .invoke(
            "CreateFirstTaskV1",
            &[json!(7), json!(1), json!("  First task  ")],
        )
        .unwrap();
    assert_eq!(task["task"]["id"], 1);
    assert_eq!(task["task"]["planId"], 1);
    assert_eq!(task["task"]["title"], "First task");
    assert_eq!(task["task"]["status"], "todo");
    assert_eq!(
        workspace
            .invoke(
                "CreateFirstTaskV1",
                &[json!(7), json!(1), json!("First task")],
            )
            .unwrap(),
        task
    );
    assert!(
        workspace
            .invoke(
                "CreateFirstTaskV1",
                &[json!(7), json!(1), json!("Different task")],
            )
            .is_err()
    );

    let expected_updated_at = task["task"]["updatedAt"].as_str().unwrap();
    assert!(
        workspace
            .invoke(
                "StartFirstTaskV1",
                &[json!(0), json!(1), json!(expected_updated_at)],
            )
            .is_err()
    );
    assert!(
        workspace
            .invoke(
                "StartFirstTaskV1",
                &[json!(7), json!(1), json!("2020-01-01T00:00:00Z")],
            )
            .is_err()
    );
    let started = workspace
        .invoke(
            "StartFirstTaskV1",
            &[json!(7), json!(1), json!(expected_updated_at)],
        )
        .unwrap();
    assert_eq!(started["task"]["status"], "doing");
    assert_eq!(started["state"]["generation"], 7);
    assert_eq!(
        workspace
            .invoke(
                "StartFirstTaskV1",
                &[json!(7), json!(1), json!(expected_updated_at)],
            )
            .unwrap(),
        started
    );
    assert_eq!(
        workspace
            .invoke(
                "CreateFirstTaskV1",
                &[json!(7), json!(1), json!("First task")],
            )
            .unwrap()["task"]["status"],
        "doing"
    );

    let store = ProjectStore::open_existing(
        &bindings.project.as_ref().unwrap().database,
        &bindings.project.as_ref().unwrap().binding,
        "test",
    )
    .unwrap();
    store
        .set_task_status(1, ptrack_core::TaskStatus::Done)
        .unwrap();
    drop(store);
    assert!(
        workspace
            .invoke(
                "StartFirstTaskV1",
                &[json!(7), json!(1), started["task"]["updatedAt"].clone()],
            )
            .is_err()
    );
    let store = ProjectStore::open_existing(
        &bindings.project.as_ref().unwrap().database,
        &bindings.project.as_ref().unwrap().binding,
        "test",
    )
    .unwrap();
    assert_eq!(store.task(1).unwrap().status, ptrack_core::TaskStatus::Done);
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
    let plan_search = workspace.invoke("SearchV2", &[json!("desktop")]).unwrap();
    assert_eq!(plan_search[0]["status"], "active");
    let task_search = workspace.invoke("SearchV2", &[json!("wire")]).unwrap();
    assert_eq!(task_search[0]["status"], "todo");
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
    let project_argument = external_command_path(&project.0);
    let run = |arguments: &[&str]| {
        let status = std::process::Command::new("git")
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?}");
    };
    run(&["-C", &project_argument, "init"]);
    run(&[
        "-C",
        &project_argument,
        "config",
        "user.email",
        "test@example.com",
    ]);
    run(&["-C", &project_argument, "config", "user.name", "Test"]);
    std::fs::write(project.0.join("tracked"), "tracked").unwrap();
    run(&["-C", &project_argument, "add", "tracked"]);
    run(&["-C", &project_argument, "commit", "-m", "initial"]);
    let worktree = worktree_parent.0.join("tree");
    let worktree_argument = external_command_path(&worktree);
    run(&[
        "-C",
        &project_argument,
        "worktree",
        "add",
        "--detach",
        &worktree_argument,
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

fn external_command_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(path) = text.strip_prefix("\\\\?\\UNC\\") {
            return format!("\\\\{path}");
        }
        if let Some(path) = text.strip_prefix("\\\\?\\") {
            return path.to_owned();
        }
    }
    text.into_owned()
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
async fn first_task_start_refuses_resource_confirmation_and_preserves_todo() {
    let directory = TestDirectory::new("first-task-resource");
    let (seed, bindings) = empty_bound_workspace(&directory);
    seed.invoke("CreateFirstPlanV1", &[json!(7), json!("Plan")])
        .unwrap();
    let created = seed
        .invoke("CreateFirstTaskV1", &[json!(7), json!(1), json!("Task")])
        .unwrap();
    let expected_updated_at = created["task"]["updatedAt"].as_str().unwrap().to_owned();
    drop(seed);

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
    terminal
        .associate(
            7,
            &session.session_id,
            TerminalAssociationPointer {
                version: 1,
                plan_id: 1,
                task_id: 1,
            },
        )
        .unwrap();
    let workspace = BoundDesktopWorkspace::new(
        7,
        0,
        bindings.clone(),
        Box::new(LocalApplication::new(bindings.clone())),
        Some(terminal.clone()),
        None,
        None,
    );

    assert_eq!(
        workspace
            .invoke(
                "StartFirstTaskV1",
                &[json!(7), json!(1), json!(expected_updated_at)],
            )
            .unwrap_err()
            .to_string(),
        "first task start requires resource confirmation"
    );
    let store = ProjectStore::open_existing(
        &bindings.project.as_ref().unwrap().database,
        &bindings.project.as_ref().unwrap().binding,
        "test",
    )
    .unwrap();
    assert_eq!(store.task(1).unwrap().status, ptrack_core::TaskStatus::Todo);
    drop(store);
    terminal.close(7, &session.session_id, true).unwrap();
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
    let workspace = Arc::new(BoundDesktopWorkspace::new(
        7,
        0,
        bindings.clone(),
        Box::new(LocalApplication::new(bindings)),
        None,
        None,
        None,
    ));
    let pending_admission = workspace.begin_resource_admission().unwrap();
    let runtime = DesktopRuntime::new(DesktopRuntimeConfig {
        version: "test".to_owned(),
        factory: Arc::new(FakeFactory::default()),
        event_sink: None,
        initial_workspace: Some(workspace.clone()),
        recent_projects: Arc::new(super::desktop_runtime::NoRecentProjectsProvider),
        initialization: Arc::new(super::desktop_runtime::NoDesktopInitializationService),
        update_service: super::update_runtime::UnavailableUpdateService::new("test"),
        confirmation_ttl: Duration::from_millis(30),
    });
    let challenge = runtime
        .invoke(request("CloseProject", vec![json!("")]))
        .unwrap();
    assert_eq!(challenge["requiresConfirmation"], true);
    assert_eq!(
        workspace
            .begin_resource_admission()
            .err()
            .expect("confirmation must retain the resource-admission fence")
            .to_string(),
        "workspace resource admission is fenced"
    );
    let expiry_deadline = Instant::now() + Duration::from_secs(2);
    let after_expiry = loop {
        match workspace.begin_resource_admission() {
            Ok(admission) => break admission,
            Err(error) => {
                assert_eq!(error.to_string(), "workspace resource admission is fenced");
                assert!(
                    Instant::now() < expiry_deadline,
                    "confirmation fence did not expire"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    };
    drop(after_expiry);
    drop(pending_admission);
    drop(runtime);
}

#[test]
fn every_allowlisted_capability_method_routes_to_the_broker() {
    // GetCapabilitiesV2 pluralizes, so a "Capability" stem skipped it and left
    // its handler unreachable: the Capabilities page could not load, the
    // diagnostics report always read the counts as absent, and a full reset
    // reported revoking no grants because it never listed any.
    let capability: Vec<&str> = allowed_desktop_commands()
        .iter()
        .copied()
        .filter(|method| method.contains("Capabilit"))
        .collect();
    assert_eq!(
        capability,
        [
            "DisableCapabilityV2",
            "EnableCapabilityV2",
            "ExpireCapabilityV2",
            "GetCapabilitiesV2",
            "GetCapabilityAuditsV2",
            "PreviewCapabilityV2",
            "RemoveCapabilityV2",
            "SaveCapabilityV2",
            "TestCapabilityV2",
        ]
    );
    for method in capability {
        assert!(
            crate::desktop_runtime::routes_to_capability(method),
            "{method}"
        );
    }
    for method in ["GetPreferences", "CreateTerminal", "LaunchLinkedAgentV2"] {
        assert!(
            !crate::desktop_runtime::routes_to_capability(method),
            "{method}"
        );
    }
}

#[test]
fn held_plans_and_tasks_reach_the_board_payload_without_leaving_their_column() {
    let directory = TestDirectory::new("board-hold");
    let (bindings, task_id) = bound_bindings(&directory);
    let store = ProjectStore::open_existing(
        &bindings.project.as_ref().unwrap().database,
        &bindings.project.as_ref().unwrap().binding,
        "test",
    )
    .unwrap();
    let plan_id = store.meta().unwrap().active_plan;
    store
        .set_task_hold(task_id, Some("waiting on review".to_owned()))
        .unwrap();
    store
        .set_plan_hold(plan_id, Some("waiting on design".to_owned()))
        .unwrap();
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

    let board = workspace
        .invoke("GetBoardV2", &[json!(7), json!(0)])
        .unwrap();
    let columns = board["board"]["columns"].as_array().unwrap();
    // Four lanes, and the held task still sits in Todo — hold is a badge, not a lane.
    assert_eq!(columns.len(), 4);
    assert_eq!(columns[0]["status"], "todo");
    assert_eq!(columns[0]["tasks"][0]["id"], task_id);
    assert_eq!(columns[0]["tasks"][0]["holdReason"], "waiting on review");
    assert_eq!(
        board["board"]["plans"][0]["holdReason"],
        "waiting on design"
    );

    // The paged snapshot projection builds its own plan rows and blocker cards.
    let snapshot = workspace
        .invoke("GetWorkspaceSnapshot", &[json!(7), json!(0)])
        .unwrap();
    assert_eq!(
        snapshot["tracking"]["board"]["plans"][0]["holdReason"],
        "waiting on design"
    );
}

#[test]
fn claimed_plans_reach_the_board_and_snapshot_payload_with_the_resolved_name() {
    let directory = TestDirectory::new("board-claim");
    let (bindings, _task_id) = bound_bindings(&directory);
    let project = bindings.project.as_ref().unwrap();
    let global =
        GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding).unwrap();
    let identity = set_identity_name(&global, "Alice").unwrap();
    drop(global);
    let store = ProjectStore::open_existing(&project.database, &project.binding, "test")
        .unwrap()
        .with_actor(Some(identity));
    let plan_id = store.meta().unwrap().active_plan;
    store.use_plan(plan_id, false).unwrap();
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

    let board = workspace
        .invoke("GetBoardV2", &[json!(7), json!(0)])
        .unwrap();
    assert_eq!(board["board"]["plans"][0]["claimedBy"], "Alice");

    let snapshot = workspace
        .invoke("GetWorkspaceSnapshot", &[json!(7), json!(0)])
        .unwrap();
    assert_eq!(
        snapshot["tracking"]["board"]["plans"][0]["claimedBy"],
        "Alice"
    );
}

#[test]
fn unclaimed_plans_omit_claimed_by_from_the_board_payload() {
    let directory = TestDirectory::new("board-unclaimed");
    let (bindings, _task_id) = bound_bindings(&directory);
    let workspace = BoundDesktopWorkspace::new(
        7,
        0,
        bindings.clone(),
        Box::new(LocalApplication::new(bindings)),
        None,
        None,
        None,
    );

    let board = workspace
        .invoke("GetBoardV2", &[json!(7), json!(0)])
        .unwrap();
    assert!(board["board"]["plans"][0].get("claimedBy").is_none());
}

#[test]
fn bounded_workspace_snapshot_follows_the_per_actor_active_plan() {
    // The bounded snapshot path (GetWorkspaceSnapshot) used to read the raw
    // stored singleton via store.meta() instead of resolving it through the
    // configured actor, so a claimed-plan GUI opened the wrong plan and
    // marked the wrong row active once identities existed.
    let directory = TestDirectory::new("bounded-snapshot-per-actor");
    let (bindings, _task_id) = bound_bindings(&directory);
    let project = bindings.project.as_ref().unwrap();
    let global =
        GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding).unwrap();
    let identity = set_identity_name(&global, "Alice").unwrap();
    drop(global);
    let store = ProjectStore::open_existing(&project.database, &project.binding, "test")
        .unwrap()
        .with_actor(Some(identity));
    let first_plan = store.meta().unwrap().active_plan;
    let second_plan = store.add_plan("Alice's plan", 0).unwrap().id;
    // Only Alice's per-actor pointer moves; the legacy singleton stays put.
    store.set_active_plan(second_plan).unwrap();
    assert_eq!(store.meta().unwrap().active_plan, first_plan);
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
    let board = &snapshot["tracking"]["board"];
    assert_eq!(board["planId"], second_plan);
    let plans = board["plans"].as_array().unwrap();
    let first = plans.iter().find(|plan| plan["id"] == first_plan).unwrap();
    let second = plans.iter().find(|plan| plan["id"] == second_plan).unwrap();
    assert_eq!(first["isActive"], false);
    assert_eq!(second["isActive"], true);
}

#[test]
fn desktop_plan_lifecycle_commands_rename_preview_delete_and_copy_within() {
    let directory = TestDirectory::new("plan-lifecycle-commands");
    let (bindings, _task_id) = bound_bindings(&directory); // seeded: plan "Desktop" + task + note
    let root = bindings.project.as_ref().unwrap().root.clone();
    let global =
        GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding).unwrap();
    global.register_project("Desktop", &root).unwrap();
    drop(global);
    let workspace = BoundDesktopWorkspace::new(
        7,
        0,
        bindings.clone(),
        Box::new(LocalApplication::new(bindings)),
        None,
        None,
        None,
    );
    let plan_id = 1_u64;

    // Rename.
    workspace
        .invoke(
            "RenamePlanV1",
            &[json!(7), json!(plan_id), json!("Renamed")],
        )
        .unwrap();
    // Preview (force=false): counts, nothing deleted.
    let preview = workspace
        .invoke("DeletePlanV1", &[json!(7), json!(plan_id), json!(false)])
        .unwrap();
    assert_eq!(preview["preview"], json!(true));
    assert_eq!(preview["summary"]["title"], json!("Renamed"));
    assert_eq!(preview["summary"]["tasks"], json!(1));
    assert_eq!(preview["summary"]["notes"], json!(1));

    // Copy within the project requires a new title; empty target + empty title fails.
    let refusal = workspace
        .invoke(
            "CopyPlanV1",
            &[json!(7), json!(plan_id), json!(""), json!("")],
        )
        .unwrap_err();
    assert!(refusal.to_string().contains("--as"));
    let copied = workspace
        .invoke(
            "CopyPlanV1",
            &[json!(7), json!(plan_id), json!(""), json!("Second")],
        )
        .unwrap();
    assert_eq!(copied["summary"]["title"], json!("Second"));
    assert_eq!(copied["summary"]["moved"], json!(false));

    // Delete (force=true) removes it.
    let deleted = workspace
        .invoke("DeletePlanV1", &[json!(7), json!(plan_id), json!(true)])
        .unwrap();
    assert_eq!(deleted["preview"], json!(false));
    assert_eq!(deleted["summary"]["tasks"], json!(1));
    let missing = workspace
        .invoke("DeletePlanV1", &[json!(7), json!(plan_id), json!(false)])
        .unwrap_err();
    assert!(missing.to_string().contains("not found"));

    // Move to an unregistered project surfaces the guard message verbatim.
    let unknown = workspace
        .invoke(
            "MovePlanV1",
            &[json!(7), json!(2), json!("/no/such/project"), json!("")],
        )
        .unwrap_err();
    assert!(unknown.to_string().contains("ptrack projects"));

    // Move targeting the current project itself surfaces the in-place guard
    // ("rename it in place with 'ptrack plan rename'"), not a transfer.
    let same_project = workspace
        .invoke(
            "MovePlanV1",
            &[
                json!(7),
                json!(plan_id),
                json!(root.to_string_lossy().into_owned()),
                json!(""),
            ],
        )
        .unwrap_err();
    assert!(same_project.to_string().contains("current project"));

    // ListProjectsV1 answers with the registry (possibly empty in this harness).
    let projects = workspace.invoke("ListProjectsV1", &[json!(7)]).unwrap();
    assert!(projects["projects"].is_array());
}

#[test]
fn repo_stats_counts_tracked_files_and_lines_and_fails_soft() {
    let root = std::env::temp_dir().join(format!(
        "ptrack-repo-stats-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();

    // Not a git repository: soft failure, never an error.
    let stats = repo_stats(&root);
    assert!(!stats.available);

    let git = |args: &[&str]| {
        let outcome = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .status()
            .unwrap();
        assert!(outcome.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.email=t@t",
        "-c",
        "user.name=t",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "root",
    ]);

    // A repository whose HEAD holds no files is available and empty.
    let stats = repo_stats(&root);
    assert_eq!((stats.available, stats.files, stats.lines), (true, 0, 0));

    std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(root.join("b.txt"), "four\n").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "user.email=t@t",
        "-c",
        "user.name=t",
        "commit",
        "-q",
        "-m",
        "content",
    ]);

    let stats = repo_stats(&root);
    assert_eq!((stats.available, stats.files, stats.lines), (true, 2, 4));

    std::fs::remove_dir_all(&root).unwrap();
}
