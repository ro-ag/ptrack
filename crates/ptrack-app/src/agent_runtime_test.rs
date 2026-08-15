use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Barrier, Condvar, Mutex, Weak};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use ptrack_agent::{
    ASSOCIATION_VERSION_V1, AssociationPointer, CoordinationError, CoordinationGit,
    CoordinationGitSnapshot, CoordinationSession, CoordinationSessions, CoordinationStore,
    IntegrationConfig, PROVIDER_EVENT_MODEL_VERSION, ProviderEvent, Registration, Registry,
    WorktreeIdentity,
};
use ptrack_capability::{Broker, BrokerConfig};
use ptrack_git::{
    Branch, Commit, Divergence, ExistingWorktree, PathBounds, RepositoryState, Snapshot, Status,
    WorktreeBounds,
};
use ptrack_store::{ActiveBinding, GlobalStore, ProjectStore, StoreKind};
use ptrack_terminal::{ExitResult, Manager, ProfileKind, SessionInfo};

use super::terminal_runtime_test::{TestEvents, TestFactory, profile};
use crate::agent_runtime::map_git_snapshot;
use crate::{
    AgentIntegration, AgentIntegrationFactory, AgentRuntime, AgentRuntimeConfig,
    AgentRuntimeService, BoundDesktopWorkspace, DesktopWorkspace, LaunchedEventAuthority,
    LinkedAgentRuntimeHooks, LocalApplication, PreparedTerminalIdentity,
    ProductionTerminalIdentityAuthority, ProjectCoordinationStore, ProjectEndpoint,
    TerminalIdentityAuthority, TerminalRuntime, TerminalRuntimeConfig, WorkspaceBindings,
};

struct TestDirectory(PathBuf);

struct CapturingProductionIdentity {
    inner: ProductionTerminalIdentityAuthority,
    tokens: Mutex<(String, String)>,
}

impl TerminalIdentityAuthority for CapturingProductionIdentity {
    fn prepare(
        &self,
        generation: u64,
        project_root: &Path,
        profile: &ptrack_terminal::Profile,
    ) -> crate::AppResult<PreparedTerminalIdentity> {
        let identity = self.inner.prepare(generation, project_root, profile)?;
        *self.tokens.lock().unwrap() = (
            identity.event_token().to_owned(),
            identity.capability_token().to_owned(),
        );
        Ok(identity)
    }

    fn bind(
        &self,
        generation: u64,
        identity: &PreparedTerminalIdentity,
        session: &SessionInfo,
    ) -> crate::AppResult<()> {
        self.inner.bind(generation, identity, session)
    }

    fn bind_linked(
        &self,
        generation: u64,
        identity: &PreparedTerminalIdentity,
        session: &SessionInfo,
        pointer: AssociationPointer,
    ) -> crate::AppResult<ptrack_agent::Association> {
        self.inner
            .bind_linked(generation, identity, session, pointer)
    }

    fn revoke_pending(&self, generation: u64, identity: &PreparedTerminalIdentity) {
        self.inner.revoke_pending(generation, identity);
    }

    fn revoke_session(&self, generation: u64, session_id: &str) {
        self.inner.revoke_session(generation, session_id);
    }

    fn revoke_failed_session(&self, generation: u64, session_id: &str) {
        self.inner.revoke_failed_session(generation, session_id);
    }

    fn remove_linked_session(&self, generation: u64, session_id: &str) {
        self.inner.remove_linked_session(generation, session_id);
    }

    fn record_exit(&self, generation: u64, session_id: &str, result: &ExitResult) {
        self.inner.record_exit(generation, session_id, result);
    }
}

impl TestDirectory {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ptrack-agent-runtime-{name}-{}-{nonce}",
            std::process::id()
        ));
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

#[derive(Default)]
struct TestSessions;

impl CoordinationSessions for TestSessions {
    fn snapshot(&self, _limit: usize) -> (Vec<CoordinationSession>, usize) {
        (Vec::new(), 0)
    }
}

#[derive(Default)]
struct TestGit;

impl CoordinationGit for TestGit {
    fn inspect_worktree(
        &self,
        _project_root: &Path,
        _root: &Path,
    ) -> Result<WorktreeIdentity, CoordinationError> {
        Err(CoordinationError::Message(
            "test worktree unavailable".to_owned(),
        ))
    }

    fn snapshot(&self, _root: &Path) -> Result<CoordinationGitSnapshot, CoordinationError> {
        Ok(CoordinationGitSnapshot::default())
    }
}

struct BlockingGit {
    cancellation: ptrack_git::CancellationToken,
    entered: SyncSender<()>,
    wait: Arc<(Mutex<bool>, Condvar)>,
}

impl CoordinationGit for BlockingGit {
    fn inspect_worktree(
        &self,
        _project_root: &Path,
        _root: &Path,
    ) -> Result<WorktreeIdentity, CoordinationError> {
        Err(CoordinationError::Message("unused".to_owned()))
    }

    fn snapshot(&self, _root: &Path) -> Result<CoordinationGitSnapshot, CoordinationError> {
        let _ = self.entered.try_send(());
        let (released, wake) = &*self.wait;
        let mut released = released.lock().expect("blocking Git lock");
        while !*released && !self.cancellation.is_cancelled() {
            (released, _) = wake
                .wait_timeout(released, Duration::from_millis(5))
                .expect("blocking Git wait");
        }
        if self.cancellation.is_cancelled() {
            Err(CoordinationError::Message("Git cancelled".to_owned()))
        } else {
            Ok(CoordinationGitSnapshot::default())
        }
    }
}

#[derive(Default)]
struct FakeIntegrationFactory {
    callback: Mutex<Option<SyncSender<()>>>,
    registry: Mutex<Option<Weak<Registry>>>,
    shutdown_trace: Mutex<Vec<&'static str>>,
    registry_open_during_server_shutdown: AtomicBool,
    fail_start: AtomicBool,
    fail_shutdown: AtomicBool,
}

impl FakeIntegrationFactory {
    fn callback(&self) -> SyncSender<()> {
        self.callback
            .lock()
            .expect("callback lock")
            .clone()
            .expect("integration callback")
    }

    fn registry(&self) -> Arc<Registry> {
        self.registry
            .lock()
            .expect("registry lock")
            .as_ref()
            .and_then(Weak::upgrade)
            .expect("live registry")
    }
}

struct OwnedFakeIntegration {
    registry: Weak<Registry>,
    owner: Arc<FakeIntegrationFactory>,
}

impl AgentIntegration for OwnedFakeIntegration {
    fn event_endpoint(&self) -> &'static str {
        "http://127.0.0.1:1/v1/events"
    }

    fn shutdown(&self, _timeout: Duration) -> Result<(), String> {
        self.owner
            .shutdown_trace
            .lock()
            .expect("trace lock")
            .push("server");
        let open = self
            .registry
            .upgrade()
            .is_some_and(|registry| registry.issue_launched_event_token().is_ok());
        self.owner
            .registry_open_during_server_shutdown
            .store(open, Ordering::Release);
        if self.owner.fail_shutdown.load(Ordering::Acquire) {
            Err("injected integration shutdown failure".to_owned())
        } else {
            Ok(())
        }
    }
}

struct OwnedFactory(Arc<FakeIntegrationFactory>);

impl AgentIntegrationFactory for OwnedFactory {
    fn start(
        &self,
        registry: Arc<Registry>,
        config: IntegrationConfig,
    ) -> Result<Box<dyn AgentIntegration>, String> {
        *self.0.callback.lock().expect("callback lock") = config.runtime_changed;
        *self.0.registry.lock().expect("registry lock") = Some(Arc::downgrade(&registry));
        if self.0.fail_start.load(Ordering::Acquire) {
            return Err("injected integration startup failure".to_owned());
        }
        Ok(Box::new(OwnedFakeIntegration {
            registry: Arc::downgrade(&registry),
            owner: Arc::clone(&self.0),
        }))
    }
}

#[derive(Default)]
struct DeadlineBlockingFactory {
    shutdown_calls: AtomicUsize,
}

struct DeadlineBlockingIntegration(Arc<DeadlineBlockingFactory>);

impl AgentIntegration for DeadlineBlockingIntegration {
    fn event_endpoint(&self) -> &'static str {
        "http://127.0.0.1:2/v1/events"
    }

    fn shutdown(&self, timeout: Duration) -> Result<(), String> {
        self.0.shutdown_calls.fetch_add(1, Ordering::AcqRel);
        std::thread::sleep(timeout);
        Err("injected integration shutdown timeout".to_owned())
    }
}

struct DeadlineBlockingFactoryAdapter(Arc<DeadlineBlockingFactory>);

impl AgentIntegrationFactory for DeadlineBlockingFactoryAdapter {
    fn start(
        &self,
        _registry: Arc<Registry>,
        _config: IntegrationConfig,
    ) -> Result<Box<dyn AgentIntegration>, String> {
        Ok(Box::new(DeadlineBlockingIntegration(Arc::clone(&self.0))))
    }
}

fn endpoint(directory: &TestDirectory) -> (ProjectEndpoint, u64, u64) {
    let root = directory.0.join("project");
    let database = root.join(".ptrack/ptrack.redb");
    std::fs::create_dir_all(database.parent().expect("database parent"))
        .expect("project directory");
    let binding = ActiveBinding {
        generation: 91,
        database_id: "project-runtime-test".to_owned(),
        kind: StoreKind::Project,
        canonical_path: database.clone(),
    };
    let store =
        ProjectStore::create_new(&database, binding.clone(), "test").expect("create project store");
    let plan = store.add_plan("Plan", 0).expect("add plan");
    let task = store.add_task(plan.id, "Task").expect("add task");
    drop(store);
    (
        ProjectEndpoint {
            root: std::fs::canonicalize(root).expect("canonical project"),
            database,
            binding,
        },
        plan.id,
        task.id,
    )
}

fn start_runtime(
    directory: &TestDirectory,
    generation: u64,
    endpoint: ProjectEndpoint,
    factory: Arc<FakeIntegrationFactory>,
) -> AgentRuntime {
    let (home, global_database, global_binding) = global_attestation(directory);
    AgentRuntime::start(AgentRuntimeConfig {
        generation,
        endpoint,
        global_home: home,
        global_database,
        global_binding,
        writer_version: "test".to_owned(),
        sessions: Arc::new(TestSessions),
        git: Arc::new(TestGit),
        git_cancellation: None,
        integration_factory: Arc::new(OwnedFactory(factory)),
        operation_shutdown_timeout: Duration::from_secs(3),
        integration_shutdown_timeout: Duration::from_secs(2),
        registry_shutdown_timeout: Duration::from_secs(2),
    })
    .expect("start AgentRuntime")
}

fn runtime_config(
    directory: &TestDirectory,
    generation: u64,
    endpoint: ProjectEndpoint,
    factory: Arc<FakeIntegrationFactory>,
) -> AgentRuntimeConfig {
    let (home, global_database, global_binding) = global_attestation(directory);
    AgentRuntimeConfig {
        generation,
        endpoint,
        global_home: home,
        global_database,
        global_binding,
        writer_version: "test".to_owned(),
        sessions: Arc::new(TestSessions),
        git: Arc::new(TestGit),
        git_cancellation: None,
        integration_factory: Arc::new(OwnedFactory(factory)),
        operation_shutdown_timeout: Duration::from_secs(3),
        integration_shutdown_timeout: Duration::from_secs(2),
        registry_shutdown_timeout: Duration::from_secs(2),
    }
}

fn global_attestation(directory: &TestDirectory) -> (PathBuf, PathBuf, ActiveBinding) {
    let home = directory.0.join("home");
    std::fs::create_dir_all(&home).expect("runtime home");
    let home = std::fs::canonicalize(home).expect("canonical runtime home");
    let database = home.join("global.redb");
    let binding = ActiveBinding {
        generation: 91,
        database_id: "global-runtime-test".to_owned(),
        kind: StoreKind::Global,
        canonical_path: database.clone(),
    };
    if !database.exists() {
        drop(GlobalStore::create_new(&database, binding.clone()).expect("create global store"));
    }
    (home, database, binding)
}

fn registration(root: &Path, terminal_id: &str) -> Registration {
    Registration {
        profile: "codex".to_owned(),
        provider: "openai".to_owned(),
        pid: i32::try_from(std::process::id()).expect("process ID"),
        terminal_id: terminal_id.to_owned(),
        cwd: root.to_string_lossy().into_owned(),
    }
}

#[test]
fn generation_store_reopen_invalidation_and_privacy_contracts() {
    let directory = TestDirectory::new("generation");
    let (endpoint, plan_id, task_id) = endpoint(&directory);
    let factory = Arc::new(FakeIntegrationFactory::default());
    let runtime = start_runtime(&directory, 7, endpoint.clone(), Arc::clone(&factory));

    let run = runtime
        .register_launched(7, registration(&endpoint.root, "terminal-1"))
        .expect("register launched run");
    let first = runtime.drain_invalidations(7).expect("drain registration");
    assert_eq!(first.event_count, 1);
    assert_eq!(first.resource_revision, 1);

    let association = runtime
        .associate_run(
            7,
            &run.id,
            AssociationPointer {
                version: ASSOCIATION_VERSION_V1,
                plan_id,
                task_id,
            },
        )
        .expect("associate run");
    assert_eq!(association.generation, 7);
    assert_eq!(association.revision, 1);
    assert_eq!(
        runtime
            .drain_invalidations(0)
            .expect("zero selects active generation")
            .event_count,
        1
    );

    let runs = runtime.agent_runs(7).expect("agent runs");
    assert_eq!(runs.generation, 7);
    assert_eq!(runs.runs.len(), 1);
    assert_eq!(
        runs.runs[0].association.expect("association").task_id,
        task_id
    );
    let encoded = serde_json::to_string(&runs).expect("encode projection");
    for forbidden in [
        "leaseToken",
        "registrationToken",
        "eventToken",
        "cwd",
        "projectRoot",
        "exitCode",
        "result",
        "prompt",
        "authorEmail",
        "fetchUrls",
    ] {
        assert!(
            !encoded.contains(forbidden),
            "projection leaked {forbidden}"
        );
    }

    // Store generation 91 and workspace generation 7 are intentionally
    // independent. A fresh writer can open while AgentRuntime remains idle.
    assert_eq!(endpoint.binding.generation, 91);
    let store = ProjectStore::open_existing(&endpoint.database, &endpoint.binding, "concurrent")
        .expect("runtime retained no store handle");
    store
        .set_task_title(task_id, "changed while runtime lives")
        .expect("concurrent store write");
    drop(store);

    let stale = runtime.agent_runs(8).expect_err("stale generation");
    assert!(stale.to_string().contains("expected 8, active 7"));
    AgentRuntimeService::shutdown(&runtime).expect("shutdown runtime");
}

#[test]
fn workspace_and_task_rows_share_structured_waiting_intelligence() {
    let directory = TestDirectory::new("workspace-intelligence");
    let (endpoint, plan_id, task_id) = endpoint(&directory);
    let factory = Arc::new(FakeIntegrationFactory::default());
    let runtime = Arc::new(start_runtime(
        &directory,
        7,
        endpoint.clone(),
        Arc::clone(&factory),
    ));
    let token = runtime.issue_launched_event_token(7).unwrap();
    let run = runtime
        .register_launched(
            7,
            Registration {
                profile: "codex".to_owned(),
                provider: "codex".to_owned(),
                pid: i32::try_from(std::process::id()).unwrap(),
                terminal_id: "terminal-waiting".to_owned(),
                cwd: endpoint.root.to_string_lossy().into_owned(),
            },
        )
        .unwrap();
    runtime
        .bind_launched_event_token(7, &token, &run.id)
        .unwrap();
    runtime
        .associate_run(
            7,
            &run.id,
            AssociationPointer {
                version: ASSOCIATION_VERSION_V1,
                plan_id,
                task_id,
            },
        )
        .unwrap();
    factory
        .registry()
        .record_launched_provider_event(
            &token,
            ProviderEvent {
                model_version: PROVIDER_EVENT_MODEL_VERSION,
                id: "permission-1".to_owned(),
                sequence: 1,
                event_type: "permissionrequest".to_owned(),
                ..ProviderEvent::default()
            },
        )
        .unwrap();
    let (global_home, global_database, global_binding) = global_attestation(&directory);
    let bindings = WorkspaceBindings {
        current_dir: endpoint.root.clone(),
        project: Some(endpoint),
        global_database,
        global_binding,
        global_home,
        writer_version: "test".to_owned(),
    };
    let workspace = BoundDesktopWorkspace::new(
        7,
        plan_id,
        bindings.clone(),
        Box::new(LocalApplication::new(bindings)),
        None,
        Some(runtime.clone()),
        None,
    );
    let snapshot = workspace
        .invoke(
            "GetWorkspaceSnapshot",
            &[serde_json::json!(7), serde_json::json!(plan_id)],
        )
        .unwrap();
    assert_eq!(snapshot["agentRuns"]["runs"][0]["activityState"], "waiting");
    assert_eq!(
        snapshot["agentRuns"]["runs"][0]["intelligence"]["state"],
        "waiting"
    );
    let detail = workspace
        .invoke(
            "GetTaskDetailV2",
            &[serde_json::json!(7), serde_json::json!(task_id)],
        )
        .unwrap();
    assert_eq!(
        detail["linkedRuntime"]["agents"][0]["activityState"],
        "waiting"
    );
    assert_eq!(
        detail["agentIntelligence"][0]["intelligence"]["state"],
        "waiting"
    );
    drop(workspace);
    AgentRuntimeService::shutdown(runtime.as_ref()).unwrap();
}

#[test]
fn terminal_owned_agent_mutation_advances_revision_without_duplicate_invalidation() {
    let directory = TestDirectory::new("event-owner");
    let (endpoint, _, _) = endpoint(&directory);
    let factory = Arc::new(FakeIntegrationFactory::default());
    let runtime = start_runtime(&directory, 7, endpoint.clone(), factory);
    {
        let _suppression = runtime
            .suppress_runtime_event(7)
            .expect("terminal event suppression");
        runtime
            .register_launched(7, registration(&endpoint.root, "terminal-owned"))
            .expect("terminal-owned registration");
    }
    let drained = runtime.drain_invalidations(7).expect("drain suppression");
    assert_eq!(drained.resource_revision, 1);
    assert_eq!(drained.event_count, 0);

    runtime
        .register_launched(7, registration(&endpoint.root, "agent-owned"))
        .expect("agent-owned registration");
    let drained = runtime.drain_invalidations(7).expect("drain direct event");
    assert_eq!(drained.resource_revision, 2);
    assert_eq!(drained.event_count, 1);
    AgentRuntimeService::shutdown(&runtime).expect("shutdown runtime");
}

#[test]
fn old_generation_callbacks_are_isolated_and_shutdown_is_ordered() {
    let directory = TestDirectory::new("switch");
    let (endpoint, _, _) = endpoint(&directory);
    let first_factory = Arc::new(FakeIntegrationFactory::default());
    let first = start_runtime(&directory, 1, endpoint.clone(), Arc::clone(&first_factory));
    let old_callback = first_factory.callback();
    AgentRuntimeService::shutdown(&first).expect("close first generation");
    assert!(
        first_factory
            .registry_open_during_server_shutdown
            .load(Ordering::Acquire),
        "server must close before registry"
    );
    assert_eq!(
        first_factory
            .shutdown_trace
            .lock()
            .expect("trace")
            .as_slice(),
        ["server"]
    );
    assert!(
        first_factory
            .registry()
            .issue_launched_event_token()
            .is_err(),
        "registry must be closed after server and coordinator"
    );

    let second_factory = Arc::new(FakeIntegrationFactory::default());
    let second = start_runtime(&directory, 2, endpoint, Arc::clone(&second_factory));
    let _ = old_callback.try_send(());
    let drained = second.drain_invalidations(2).expect("drain new generation");
    assert_eq!(drained.event_count, 0);
    assert_eq!(drained.resource_revision, 0);
    assert!(
        first
            .agent_runs(2)
            .unwrap_err()
            .to_string()
            .contains("expected 2, active 1")
    );
    AgentRuntimeService::shutdown(&second).expect("close second generation");
}

#[test]
fn startup_and_shutdown_failures_are_bounded_and_durable() {
    let directory = TestDirectory::new("failures");
    let (endpoint, _, _) = endpoint(&directory);
    let failed = Arc::new(FakeIntegrationFactory::default());
    failed.fail_start.store(true, Ordering::Release);
    let (home, global_database, global_binding) = global_attestation(&directory);
    let result = AgentRuntime::start(AgentRuntimeConfig {
        generation: 3,
        endpoint: endpoint.clone(),
        global_home: home,
        global_database,
        global_binding,
        writer_version: "test".to_owned(),
        sessions: Arc::new(TestSessions),
        git: Arc::new(TestGit),
        git_cancellation: None,
        integration_factory: Arc::new(OwnedFactory(Arc::clone(&failed))),
        operation_shutdown_timeout: Duration::from_secs(3),
        integration_shutdown_timeout: Duration::from_secs(2),
        registry_shutdown_timeout: Duration::from_secs(2),
    });
    let Err(error) = result else {
        panic!("startup unexpectedly succeeded");
    };
    assert!(error.to_string().contains("startup failure"));
    assert!(
        failed
            .registry
            .lock()
            .expect("registry")
            .as_ref()
            .is_some_and(|registry| registry.upgrade().is_none()),
        "failed candidate registry was torn down"
    );

    let factory = Arc::new(FakeIntegrationFactory::default());
    factory.fail_shutdown.store(true, Ordering::Release);
    let runtime = start_runtime(&directory, 4, endpoint, factory);
    let first = AgentRuntimeService::shutdown(&runtime).expect_err("shutdown failure");
    let second = AgentRuntimeService::shutdown(&runtime).expect_err("durable shutdown failure");
    assert_eq!(first.to_string(), second.to_string());
    assert!(first.to_string().contains("integration shutdown"));
    assert!(
        runtime
            .agent_runs(4)
            .unwrap_err()
            .to_string()
            .contains("closing")
    );
}

#[test]
fn store_association_adapter_fails_closed_on_generation_and_live_id() {
    let directory = TestDirectory::new("store-association");
    let (endpoint, plan_id, task_id) = endpoint(&directory);
    let factory = Arc::new(FakeIntegrationFactory::default());
    let runtime = start_runtime(&directory, 7, endpoint.clone(), factory);
    let run = runtime
        .register_linked_launched(
            7,
            registration(&endpoint.root, "terminal-linked"),
            AssociationPointer {
                version: ASSOCIATION_VERSION_V1,
                plan_id,
                task_id,
            },
        )
        .expect("register linked run");
    let association = run.association.expect("linked association");
    let adapter = ProjectCoordinationStore::new(endpoint, "test".to_owned(), 7);
    assert!(adapter.current_association(&run.id, &association).is_some());
    let mut wrong_generation = association.clone();
    wrong_generation.generation = 91;
    assert!(
        adapter
            .current_association(&run.id, &wrong_generation)
            .is_none()
    );
    assert!(
        adapter
            .current_association("other-run", &association)
            .is_none()
    );
    AgentRuntimeService::shutdown(&runtime).expect("shutdown runtime");
}

#[test]
fn git_mapping_is_bounded_content_free_and_go_compatible() {
    let snapshot = Snapshot {
        state: RepositoryState::Ready,
        root: "/project".to_owned(),
        git_dir: "/project/.git".to_owned(),
        common_git_dir: "/project/.git".to_owned(),
        status: Status {
            oid: "a".repeat(40),
            branch: "main".to_owned(),
            upstream: "origin/main".to_owned(),
            ahead: 2,
            behind: 1,
            staged: 1,
            unstaged: 2,
            untracked: 1,
            changed_paths: Some(vec!["src/lib.rs".to_owned()]),
            untracked_paths: Some(vec!["new.txt".to_owned()]),
            changed_path_bounds: PathBounds {
                shown: 1,
                total: 2,
                more: 1,
            },
            untracked_path_bounds: PathBounds {
                shown: 1,
                total: 1,
                more: 0,
            },
            ..Status::default()
        },
        local_branches: Some(vec![Branch {
            name: "main".to_owned(),
            oid: "a".repeat(40),
            ..Branch::default()
        }]),
        recent_commits: Some(vec![Commit {
            sha: "b".repeat(40),
            author_name: "private author".to_owned(),
            author_email: "secret@example.test".to_owned(),
            date: "2026-08-12T12:00:00Z".to_owned(),
            subject: "private subject".to_owned(),
            ..Commit::default()
        }]),
        unpushed_commits: Some(Vec::new()),
        worktrees: Some(vec![ExistingWorktree {
            root: "/project".to_owned(),
            branch: "main".to_owned(),
            head: "a".repeat(40),
        }]),
        worktree_bounds: WorktreeBounds {
            shown: 1,
            total: 1,
            more: 0,
        },
        divergence: Some(Divergence {
            upstream: "origin/main".to_owned(),
            ahead: 2,
            behind: 1,
        }),
        ..Snapshot::default()
    };
    let mapped = map_git_snapshot(snapshot).expect("map Git snapshot");
    assert_eq!(mapped.status.ahead, 2);
    assert_eq!(mapped.changed_more, 1);
    assert_eq!(mapped.recent_commits.len(), 1);
    assert_eq!(mapped.recent_commits[0].sha, "b".repeat(40));
    assert_eq!(
        mapped.recent_commits[0].committed_at.to_string(),
        "2026-08-12T12:00:00Z"
    );
    assert_eq!(
        mapped.worktree_bounds,
        ptrack_agent::BoundedSnapshot::new(1, 1)
    );
    assert_eq!(mapped.branches[0].name, "main");
}

#[test]
#[allow(clippy::too_many_lines)] // One exact #70 lifecycle contract and revision ledger.
fn linked_terminal_hooks_are_opaque_exact_and_revisioned() {
    let directory = TestDirectory::new("terminal-hooks");
    let (endpoint, plan_id, task_id) = endpoint(&directory);
    let factory = Arc::new(FakeIntegrationFactory::default());
    let runtime = start_runtime(&directory, 7, endpoint.clone(), factory);
    let pointer = AssociationPointer {
        version: ASSOCIATION_VERSION_V1,
        plan_id,
        task_id,
    };

    let fence = runtime.fence_admission(7).expect("admission fence");
    assert!(
        runtime
            .register_launched(7, registration(&endpoint.root, "fenced"))
            .is_err()
    );
    fence.release();
    let run = runtime
        .register_linked_launched(7, registration(&endpoint.root, "terminal-linked"), pointer)
        .expect("linked launch");
    assert!(
        runtime
            .has_linked_terminal(7, "terminal-linked")
            .expect("linked provenance")
    );
    assert!(runtime.has_linked_terminal(8, "terminal-linked").is_err());
    let previous = run.association.clone().expect("association");

    let token = runtime
        .issue_launched_event_token(7)
        .expect("issue event token");
    runtime
        .bind_launched_event_token(7, &token, &run.id)
        .expect("bind event token");
    assert!(
        runtime
            .revoke_terminal_event_tokens(7, "terminal-linked")
            .expect("terminal token revoke")
    );
    assert!(
        !runtime
            .revoke_terminal_event_tokens(7, "terminal-linked")
            .expect("idempotent terminal token revoke")
    );

    let activity = runtime
        .record_terminal_activity(
            7,
            "terminal-linked",
            ptrack_agent::Timestamp::from_unix_seconds(2_000_000_000),
        )
        .expect("record activity");
    assert_eq!(
        activity,
        crate::AgentMutationOutcome {
            matched: true,
            changed: true
        }
    );
    assert!(
        !runtime
            .record_terminal_activity(
                7,
                "terminal-linked",
                ptrack_agent::Timestamp::from_unix_seconds(1),
            )
            .expect("older activity")
            .changed
    );

    let mut terminal_next = previous.clone();
    terminal_next.live_id = "terminal-owned-id".to_owned();
    terminal_next.revision += 1;
    let change = runtime
        .prepare_linked_association(
            7,
            "terminal-linked",
            Some(&previous),
            &terminal_next,
            pointer,
        )
        .expect("prepare association")
        .expect("linked run");
    assert_eq!(change.run_id(), run.id);
    assert_eq!(change.terminal_id(), "terminal-linked");
    runtime
        .commit_linked_association(7, &change)
        .expect("commit association");
    runtime
        .rollback_linked_association(7, &change)
        .expect("rollback association");

    assert!(
        runtime
            .record_terminal_exit(7, "terminal-linked", 0, "private result")
            .expect("record exit")
            .changed
    );
    assert!(
        !runtime
            .record_terminal_exit(7, "terminal-linked", 0, "private result")
            .expect("repeat exit")
            .changed
    );
    assert_eq!(
        runtime
            .rollback_linked_terminal(7, "terminal-linked")
            .expect("terminal cleanup"),
        1
    );
    assert_eq!(
        runtime
            .rollback_linked_terminal(7, "terminal-linked")
            .expect("idempotent cleanup"),
        0
    );
    assert_eq!(
        runtime
            .resource_state(7)
            .expect("resource state")
            .resource_revision,
        9,
        "only exact material mutations increment"
    );
    assert!(
        !runtime
            .has_linked_terminal(7, "terminal-linked")
            .expect("cleaned provenance")
    );
    AgentRuntimeService::shutdown(&runtime).expect("shutdown runtime");
    assert!(runtime.has_linked_terminal(7, "terminal-linked").is_err());
}

#[tokio::test]
async fn linked_bind_failure_revokes_production_pending_identities() {
    let directory = TestDirectory::new("linked-pending-cleanup");
    let (endpoint, plan_id, task_id) = endpoint(&directory);
    let generation = endpoint.binding.generation;
    let agent = Arc::new(start_runtime(
        &directory,
        generation,
        endpoint.clone(),
        Arc::new(FakeIntegrationFactory::default()),
    ));
    let broker = Arc::new(
        Broker::new(BrokerConfig {
            project_root: endpoint.root.clone(),
            database: endpoint.database.clone(),
            binding: endpoint.binding.clone(),
            writer_version: "test".to_owned(),
            generation,
        })
        .unwrap(),
    );
    let mut agent_profile = profile(&endpoint.root);
    agent_profile.id = "agent-test".to_owned();
    agent_profile.kind = ProfileKind::Agent;
    agent_profile.provider = "test".to_owned();
    let manager = Manager::new(&endpoint.root, vec![agent_profile], Arc::new(TestFactory))
        .await
        .unwrap();
    let identity = Arc::new(CapturingProductionIdentity {
        inner: ProductionTerminalIdentityAuthority::new(
            Some(Arc::clone(&broker)),
            Some(Arc::clone(&agent) as Arc<dyn crate::TerminalAgentAuthority>),
        ),
        tokens: Mutex::new((String::new(), String::new())),
    });
    let terminal = TerminalRuntime::new(TerminalRuntimeConfig {
        generation,
        project_root: endpoint.root.clone(),
        manager,
        identity: identity.clone(),
        events: Arc::new(TestEvents::default()),
        attachment_lease: Duration::from_secs(30),
    })
    .unwrap();

    let error = terminal
        .create_linked(
            generation,
            "agent-test",
            None,
            24,
            80,
            ptrack_terminal::TerminalAssociationPointer {
                version: 0,
                plan_id,
                task_id,
            },
            "bounded launch context",
        )
        .unwrap_err();
    assert!(error.to_string().contains("association"));
    let (event_token, capability_token) = identity.tokens.lock().unwrap().clone();
    assert!(!event_token.is_empty());
    assert!(!capability_token.is_empty());
    assert!(
        broker
            .bind_session(&capability_token, "leak-check")
            .is_err()
    );
    assert!(
        !agent
            .revoke_launched_event_token(generation, &event_token)
            .unwrap(),
        "prepared event token must already be revoked"
    );

    terminal.shutdown().await.unwrap();
    AgentRuntimeService::shutdown(agent.as_ref()).unwrap();
}

#[test]
fn invalidation_overflow_never_loses_synchronous_revision() {
    let directory = TestDirectory::new("revision-overflow");
    let (endpoint, _, _) = endpoint(&directory);
    let factory = Arc::new(FakeIntegrationFactory::default());
    let runtime = start_runtime(&directory, 7, endpoint, factory);
    for _ in 0..=1_023 {
        runtime
            .issue_launched_event_token(7)
            .expect("bounded pending token");
    }
    let before = runtime.resource_state(7).expect("state before drain");
    assert_eq!(before.resource_revision, 1_024);
    let drained = runtime.drain_invalidations(7).expect("drain overflow");
    assert_eq!(drained.event_count, 1_024);
    assert_eq!(drained.resource_revision, 1_024);
    assert_eq!(
        runtime
            .drain_invalidations(7)
            .expect("empty drain")
            .resource_revision,
        1_024
    );
    AgentRuntimeService::shutdown(&runtime).expect("shutdown runtime");
}

#[test]
fn shutdown_cancels_git_before_bounded_drain_and_server_stop() {
    let directory = TestDirectory::new("shutdown-cancel");
    let (endpoint, _, _) = endpoint(&directory);
    let factory = Arc::new(FakeIntegrationFactory::default());
    let cancellation = ptrack_git::CancellationToken::new();
    let (entered_sender, entered_receiver) = std::sync::mpsc::sync_channel(1);
    let wait = Arc::new((Mutex::new(false), Condvar::new()));
    let mut config = runtime_config(&directory, 8, endpoint, Arc::clone(&factory));
    config.git = Arc::new(BlockingGit {
        cancellation: cancellation.clone(),
        entered: entered_sender,
        wait: Arc::clone(&wait),
    });
    config.git_cancellation = Some(cancellation.clone());
    config.operation_shutdown_timeout = Duration::from_millis(100);
    let runtime = Arc::new(AgentRuntime::start(config).expect("start runtime"));
    let operation_runtime = Arc::clone(&runtime);
    let operation = std::thread::spawn(move || operation_runtime.drift(8));
    entered_receiver.recv().expect("Git operation entered");
    AgentRuntimeService::shutdown(runtime.as_ref()).expect("cancelled shutdown");
    assert!(cancellation.is_cancelled());
    assert!(operation.join().expect("operation thread").is_err());
    assert_eq!(
        factory.shutdown_trace.lock().expect("trace").as_slice(),
        ["server"]
    );
}

#[test]
fn shutdown_times_out_uncooperative_operation_but_remains_idempotent() {
    let directory = TestDirectory::new("shutdown-timeout");
    let (endpoint, _, _) = endpoint(&directory);
    let factory = Arc::new(FakeIntegrationFactory::default());
    let cancellation = ptrack_git::CancellationToken::new();
    let (entered_sender, entered_receiver) = std::sync::mpsc::sync_channel(1);
    let wait = Arc::new((Mutex::new(false), Condvar::new()));
    let mut config = runtime_config(&directory, 9, endpoint, factory);
    config.git = Arc::new(BlockingGit {
        cancellation: ptrack_git::CancellationToken::new(),
        entered: entered_sender,
        wait: Arc::clone(&wait),
    });
    config.git_cancellation = Some(cancellation);
    config.operation_shutdown_timeout = Duration::from_millis(10);
    let runtime = Arc::new(AgentRuntime::start(config).expect("start runtime"));
    let operation_runtime = Arc::clone(&runtime);
    let operation = std::thread::spawn(move || operation_runtime.drift(9));
    entered_receiver.recv().expect("Git operation entered");
    let first = AgentRuntimeService::shutdown(runtime.as_ref()).expect_err("operation timeout");
    let second = AgentRuntimeService::shutdown(runtime.as_ref()).expect_err("durable timeout");
    assert_eq!(first.to_string(), second.to_string());
    assert!(first.to_string().contains("operations: timeout"));
    let (released, wake) = &*wait;
    *released.lock().expect("release lock") = true;
    wake.notify_all();
    let _ = operation.join().expect("operation thread");
}

#[test]
fn runtime_requires_exact_project_and_global_attestations() {
    let directory = TestDirectory::new("attestation");
    let (endpoint, _, _) = endpoint(&directory);
    let factory = Arc::new(FakeIntegrationFactory::default());

    let mut wrong_project = runtime_config(&directory, 10, endpoint.clone(), Arc::clone(&factory));
    wrong_project.endpoint.database = endpoint.root.join(".ptrack/other.redb");
    wrong_project.endpoint.binding.canonical_path = wrong_project.endpoint.database.clone();
    std::fs::copy(&endpoint.database, &wrong_project.endpoint.database).expect("copy project DB");
    assert!(AgentRuntime::start(wrong_project).is_err());

    let mut wrong_home = runtime_config(&directory, 10, endpoint.clone(), Arc::clone(&factory));
    std::fs::create_dir_all(wrong_home.global_home.join("nested")).expect("nested home");
    let mut dirty_home = wrong_home.global_home.as_os_str().to_os_string();
    dirty_home.push(std::path::MAIN_SEPARATOR_STR);
    dirty_home.push("nested");
    dirty_home.push(std::path::MAIN_SEPARATOR_STR);
    dirty_home.push("..");
    wrong_home.global_home = dirty_home.into();
    assert!(AgentRuntime::start(wrong_home).is_err());

    let mut wrong_global = runtime_config(&directory, 10, endpoint, factory);
    wrong_global.global_binding.generation += 1;
    assert!(AgentRuntime::start(wrong_global).is_err());
}

#[test]
fn invalid_git_timestamp_is_skipped_and_marks_only_that_projection_incomplete() {
    let snapshot = Snapshot {
        state: RepositoryState::Ready,
        recent_commits: Some(vec![
            Commit {
                sha: "a".repeat(40),
                date: "not-a-timestamp".to_owned(),
                ..Commit::default()
            },
            Commit {
                sha: "b".repeat(40),
                date: "2026-08-12T12:00:00Z".to_owned(),
                ..Commit::default()
            },
        ]),
        ..Snapshot::default()
    };
    let mapped = map_git_snapshot(snapshot).expect("malformed timestamp is section-local");
    assert_eq!(mapped.recent_commits.len(), 1);
    assert_eq!(mapped.recent_commits[0].sha, "b".repeat(40));
    assert!(mapped.recent_commits_incomplete);
    assert!(!mapped.unpushed_commits_incomplete);
}

#[test]
fn blocked_integration_and_concurrent_shutdown_waiters_are_bounded_and_stable() {
    let directory = TestDirectory::new("integration-deadline");
    let (endpoint, _, _) = endpoint(&directory);
    let fake = Arc::new(DeadlineBlockingFactory::default());
    let placeholder = Arc::new(FakeIntegrationFactory::default());
    let mut config = runtime_config(&directory, 11, endpoint, placeholder);
    config.integration_factory = Arc::new(DeadlineBlockingFactoryAdapter(Arc::clone(&fake)));
    config.integration_shutdown_timeout = Duration::from_millis(20);
    let runtime = Arc::new(AgentRuntime::start(config).expect("start runtime"));
    let start = Arc::new(Barrier::new(3));
    let mut waiters = Vec::new();
    for _ in 0..2 {
        let runtime = Arc::clone(&runtime);
        let start = Arc::clone(&start);
        waiters.push(std::thread::spawn(move || {
            start.wait();
            AgentRuntimeService::shutdown(runtime.as_ref())
                .expect_err("injected integration timeout")
                .to_string()
        }));
    }
    start.wait();
    let first = waiters.remove(0).join().expect("first shutdown waiter");
    let second = waiters.remove(0).join().expect("second shutdown waiter");
    assert_eq!(first, second);
    // The exact message is what proves neither bounded wait was entered: an
    // operation drain or registry timeout would join its own line onto this
    // one. That is what the wall clock used to stand in for, without the
    // clock, which a loaded runner could exceed while nothing was wrong.
    assert_eq!(
        first,
        "AgentRun integration shutdown: injected integration shutdown timeout"
    );
    assert_eq!(fake.shutdown_calls.load(Ordering::Acquire), 1);
}
