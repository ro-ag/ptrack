use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use ptrack_agent::{Association, AssociationPointer, AssociationTarget};
use ptrack_terminal::{
    CwdPolicy, DEFAULT_PROFILE_FONT_FAMILY, DEFAULT_PROFILE_FONT_SIZE, DEFAULT_PROFILE_SCROLLBACK,
    DEFAULT_PROFILE_THEME, ExitBehavior, ExitResult, Manager, Profile, ProfileKind, PtyFactory,
    PtyProcess, StartRequest,
};

use super::terminal_runtime::{
    PreparedTerminalIdentity, TerminalEventSink, TerminalExitV2, TerminalIdentityAuthority,
    TerminalRuntime, TerminalRuntimeConfig, TerminalStatusV2, revoke_prepared_tokens,
};
use crate::AppResult;

struct TempDirectory(PathBuf);

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ptrack-app-terminal-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct ProcessState {
    exited: bool,
    killed: bool,
    closed: bool,
}

#[derive(Default)]
struct TestProcess {
    state: Mutex<ProcessState>,
    changed: Condvar,
}

impl PtyProcess for TestProcess {
    fn pid(&self) -> u32 {
        31337
    }

    fn read(&self, _buffer: &mut [u8]) -> io::Result<usize> {
        let mut state = self.state.lock().unwrap();
        while !state.exited && !state.closed {
            state = self.changed.wait(state).unwrap();
        }
        Ok(0)
    }

    fn write(&self, buffer: &[u8]) -> io::Result<usize> {
        Ok(buffer.len())
    }

    fn resize(&self, _rows: u16, _columns: u16) -> io::Result<()> {
        Ok(())
    }

    fn wait(&self) -> io::Result<i32> {
        let mut state = self.state.lock().unwrap();
        while !state.exited {
            state = self.changed.wait(state).unwrap();
        }
        Ok(i32::from(state.killed))
    }

    fn terminate(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.exited = true;
        self.changed.notify_all();
        Ok(())
    }

    fn kill(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.exited = true;
        state.killed = true;
        self.changed.notify_all();
        Ok(())
    }

    fn close(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        self.changed.notify_all();
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct TestFactory;

impl PtyFactory for TestFactory {
    fn start(&self, _request: StartRequest) -> io::Result<Box<dyn PtyProcess>> {
        Ok(Box::new(TestProcess::default()))
    }
}

#[derive(Default)]
struct CapturingFactory(Mutex<Vec<StartRequest>>);

impl PtyFactory for CapturingFactory {
    fn start(&self, request: StartRequest) -> io::Result<Box<dyn PtyProcess>> {
        self.0.lock().unwrap().push(request);
        Ok(Box::new(TestProcess::default()))
    }
}

#[derive(Default)]
struct KillErrorFactory;

impl PtyFactory for KillErrorFactory {
    fn start(&self, _request: StartRequest) -> io::Result<Box<dyn PtyProcess>> {
        Ok(Box::new(KillErrorProcess::default()))
    }
}

#[derive(Default)]
struct KillErrorProcess(TestProcess);

impl PtyProcess for KillErrorProcess {
    fn pid(&self) -> u32 {
        self.0.pid()
    }
    fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
    }
    fn write(&self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }
    fn resize(&self, rows: u16, columns: u16) -> io::Result<()> {
        self.0.resize(rows, columns)
    }
    fn wait(&self) -> io::Result<i32> {
        self.0.wait()
    }
    fn terminate(&self) -> io::Result<()> {
        self.0.terminate()
    }
    fn kill(&self) -> io::Result<()> {
        self.0.kill()?;
        Err(io::Error::other("forced cleanup failed"))
    }
    fn close(&self) -> io::Result<()> {
        self.0.close()
    }
}

#[derive(Default)]
pub(super) struct TestIdentity {
    calls: Mutex<Vec<String>>,
}

impl TerminalIdentityAuthority for TestIdentity {
    fn prepare(
        &self,
        _generation: u64,
        _project_root: &Path,
        profile: &Profile,
    ) -> AppResult<PreparedTerminalIdentity> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("prepare:{}", profile.id));
        Ok(PreparedTerminalIdentity::empty(false))
    }

    fn bind(
        &self,
        _generation: u64,
        _identity: &PreparedTerminalIdentity,
        session: &ptrack_terminal::SessionInfo,
    ) -> AppResult<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("bind:{}", session.id));
        Ok(())
    }

    fn revoke_pending(&self, _generation: u64, _identity: &PreparedTerminalIdentity) {
        self.calls.lock().unwrap().push("revoke-pending".to_owned());
    }

    fn revoke_session(&self, _generation: u64, session_id: &str) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("revoke:{session_id}"));
    }

    fn revoke_failed_session(&self, _generation: u64, session_id: &str) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("revoke-failed:{session_id}"));
    }

    fn record_exit(&self, _generation: u64, session_id: &str, _result: &ExitResult) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("exit:{session_id}"));
    }
}

#[derive(Default)]
struct LinkedIdentity {
    calls: Mutex<Vec<String>>,
}

impl TerminalIdentityAuthority for LinkedIdentity {
    fn prepare(
        &self,
        _generation: u64,
        _project_root: &Path,
        _profile: &Profile,
    ) -> AppResult<PreparedTerminalIdentity> {
        Ok(PreparedTerminalIdentity::empty(true))
    }

    fn bind(
        &self,
        _generation: u64,
        _identity: &PreparedTerminalIdentity,
        _session: &ptrack_terminal::SessionInfo,
    ) -> AppResult<()> {
        panic!("linked launch must not use the unlinked binding path")
    }

    fn bind_linked(
        &self,
        generation: u64,
        _identity: &PreparedTerminalIdentity,
        session: &ptrack_terminal::SessionInfo,
        pointer: AssociationPointer,
    ) -> AppResult<Association> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("bind-linked:{}", session.id));
        Ok(Association {
            version: pointer.version,
            project_root: String::new(),
            generation,
            live_id: "agent-run".to_owned(),
            target: AssociationTarget {
                plan_id: pointer.plan_id,
                task_id: pointer.task_id,
            },
            revision: 1,
        })
    }

    fn revoke_pending(&self, _generation: u64, _identity: &PreparedTerminalIdentity) {}

    fn revoke_session(&self, _generation: u64, session_id: &str) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("revoke:{session_id}"));
    }

    fn revoke_failed_session(&self, _generation: u64, session_id: &str) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("revoke-failed:{session_id}"));
    }

    fn remove_linked_session(&self, _generation: u64, session_id: &str) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("remove:{session_id}"));
    }

    fn record_exit(&self, _generation: u64, _session_id: &str, _result: &ExitResult) {}
}

#[derive(Default)]
pub(super) struct TestEvents {
    statuses: Mutex<Vec<TerminalStatusV2>>,
    exits: Mutex<Vec<TerminalExitV2>>,
    changes: Mutex<Vec<u64>>,
    order: Mutex<Vec<&'static str>>,
}

impl TerminalEventSink for TestEvents {
    fn status(&self, event: TerminalStatusV2) {
        self.statuses.lock().unwrap().push(event);
        self.order.lock().unwrap().push("status");
    }

    fn exited(&self, event: TerminalExitV2) {
        self.exits.lock().unwrap().push(event);
        self.order.lock().unwrap().push("exit");
    }

    fn runtime_changed(&self, generation: u64) {
        self.changes.lock().unwrap().push(generation);
        self.order.lock().unwrap().push("runtime");
    }
}

pub(super) fn profile(_root: &Path) -> Profile {
    Profile {
        id: "shell-default".to_owned(),
        name: "Default shell".to_owned(),
        kind: ProfileKind::Shell,
        provider: String::new(),
        executable: if cfg!(windows) {
            "cmd.exe".to_owned()
        } else {
            "/bin/sh".to_owned()
        },
        args: vec!["-l".to_owned()],
        env: BTreeMap::from([("VISIBLE_ONLY_TO_CHILD".to_owned(), "value".to_owned())]),
        theme: DEFAULT_PROFILE_THEME.to_owned(),
        font_family: DEFAULT_PROFILE_FONT_FAMILY.to_owned(),
        font_size: DEFAULT_PROFILE_FONT_SIZE,
        scrollback: DEFAULT_PROFILE_SCROLLBACK,
        cwd_policy: CwdPolicy::Requested,
        fixed_cwd: String::new(),
        exit_behavior: ExitBehavior::Keep,
    }
}

fn agent_profile(root: &Path) -> Profile {
    let mut profile = profile(root);
    profile.id = "agent-test".to_owned();
    profile.name = "Test agent".to_owned();
    profile.kind = ProfileKind::Agent;
    profile.provider = "test".to_owned();
    profile
}

#[test]
fn prepared_failure_authority_is_revoked_event_before_capability() {
    let order = Mutex::new(Vec::new());
    revoke_prepared_tokens(
        "event-token",
        "capability-token",
        |token| order.lock().unwrap().push(format!("event:{token}")),
        |token| order.lock().unwrap().push(format!("capability:{token}")),
    );
    assert_eq!(
        order.into_inner().unwrap(),
        ["event:event-token", "capability:capability-token"]
    );
}

#[tokio::test]
async fn app_terminal_host_redacts_profiles_fences_generation_and_revokes_before_close() {
    let root = TempDirectory::new();
    let mut unavailable = profile(&root.0);
    unavailable.id = "shell-missing".to_owned();
    unavailable.name = "Missing shell".to_owned();
    unavailable.executable = root.0.join("missing").to_string_lossy().into_owned();
    let manager = Manager::new(
        &root.0,
        vec![profile(&root.0), unavailable],
        Arc::new(TestFactory),
    )
    .await
    .unwrap();
    let identity = Arc::new(TestIdentity::default());
    let events = Arc::new(TestEvents::default());
    let runtime = TerminalRuntime::new(TerminalRuntimeConfig {
        generation: 7,
        project_root: root.0.clone(),
        manager,
        identity: identity.clone(),
        events: events.clone(),
        attachment_lease: Duration::from_secs(30),
    })
    .unwrap();

    let profiles = runtime.profiles(7).unwrap();
    assert_eq!(profiles.generation, 7);
    assert_eq!(profiles.profiles.len(), 1);
    let encoded = serde_json::to_value(&profiles.profiles[0]).unwrap();
    assert_eq!(encoded["executable"], "");
    assert_eq!(encoded["args"], serde_json::json!([]));
    assert_eq!(encoded["env"], serde_json::json!({}));
    assert!(encoded.get("fixedCwd").is_none());
    assert_eq!(
        runtime
            .create(7, "shell-missing", None, 24, 80)
            .unwrap_err()
            .to_string(),
        "terminal profile \"shell-missing\" is unavailable"
    );
    assert!(
        runtime
            .profiles(8)
            .unwrap_err()
            .to_string()
            .contains("stale")
    );

    let cwd = runtime
        .validate_cwds(7, &[String::new(), root.0.to_string_lossy().into_owned()])
        .unwrap();
    assert!(cwd.results.iter().all(|result| result.valid));

    let session = runtime.create(7, "shell-default", None, 24, 80).unwrap();
    assert_eq!(session.generation, 7);
    assert!(session.stream_url.starts_with("ws://127.0.0.1:"));
    runtime.close(7, &session.session_id, true).unwrap();

    let calls = identity.calls.lock().unwrap().clone();
    assert_eq!(calls[0], "prepare:shell-default");
    assert!(calls[1].starts_with("bind:"));
    assert_eq!(calls[2], format!("revoke:{}", session.session_id));
    assert_eq!(
        events.statuses.lock().unwrap().last().unwrap().state,
        ptrack_terminal::SessionState::Closed
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn unattached_session_lease_revokes_authority_and_closes() {
    let root = TempDirectory::new();
    let manager = Manager::new(&root.0, vec![profile(&root.0)], Arc::new(TestFactory))
        .await
        .unwrap();
    let identity = Arc::new(TestIdentity::default());
    let events = Arc::new(TestEvents::default());
    let runtime = TerminalRuntime::new(TerminalRuntimeConfig {
        generation: 3,
        project_root: root.0.clone(),
        manager,
        identity: identity.clone(),
        events: events.clone(),
        attachment_lease: Duration::from_millis(10),
    })
    .unwrap();
    let session = runtime.create(3, "shell-default", None, 24, 80).unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(
        identity
            .calls
            .lock()
            .unwrap()
            .contains(&format!("revoke:{}", session.session_id))
    );
    assert_eq!(
        events.statuses.lock().unwrap().last().unwrap().state,
        ptrack_terminal::SessionState::Closed
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_released_renderer_keeps_the_session_and_re_claims_it_with_a_fresh_ticket() {
    let root = TempDirectory::new();
    let manager = Manager::new(&root.0, vec![profile(&root.0)], Arc::new(TestFactory))
        .await
        .unwrap();
    let identity = Arc::new(TestIdentity::default());
    let runtime = TerminalRuntime::new(TerminalRuntimeConfig {
        generation: 5,
        project_root: root.0.clone(),
        manager: Arc::clone(&manager),
        identity: identity.clone(),
        events: Arc::new(TestEvents::default()),
        attachment_lease: Duration::from_secs(30),
    })
    .unwrap();
    let created = runtime.create(5, "shell-default", None, 24, 80).unwrap();
    let session = manager.get(&created.session_id).unwrap();

    let attachment = session.attach_output(0).unwrap();
    assert!(session.release_output(attachment.lease));

    // Releasing a renderer is not terminating a session: the PTY keeps running
    // and no session authority is revoked.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        manager.get(&created.session_id).unwrap().state(),
        ptrack_terminal::SessionState::Running
    );
    assert!(
        !identity
            .calls
            .lock()
            .unwrap()
            .contains(&format!("revoke:{}", created.session_id))
    );

    // Re-attaching requires a freshly minted single-use ticket.
    let ticket = runtime
        .claim_stream_ticket(5, &created.session_id, 0)
        .unwrap();
    assert!(!ticket.gap);
    assert_eq!(ticket.from_sequence, 0);
    assert_ne!(ticket.url, created.stream_url);
    let reclaimed = session.attach_output(ticket.from_sequence).unwrap();
    assert!(reclaimed.lease > attachment.lease);

    // The released renderer's lease no longer resizes the terminal.
    assert!(
        runtime
            .resize(5, &created.session_id, Some(attachment.lease), 30, 100)
            .is_err()
    );
    runtime
        .resize(5, &created.session_id, Some(reclaimed.lease), 30, 100)
        .unwrap();
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_attached_session_outlives_the_grace_window_and_a_released_one_does_not() {
    let root = TempDirectory::new();
    let manager = Manager::new(&root.0, vec![profile(&root.0)], Arc::new(TestFactory))
        .await
        .unwrap();
    let identity = Arc::new(TestIdentity::default());
    let runtime = TerminalRuntime::new(TerminalRuntimeConfig {
        generation: 9,
        project_root: root.0.clone(),
        manager: Arc::clone(&manager),
        identity: identity.clone(),
        events: Arc::new(TestEvents::default()),
        attachment_lease: Duration::from_millis(20),
    })
    .unwrap();
    let created = runtime.create(9, "shell-default", None, 24, 80).unwrap();
    let session = manager.get(&created.session_id).unwrap();
    let attachment = session.attach_output(0).unwrap();

    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(manager.get(&created.session_id).is_ok());

    // An unclaimed session cannot leak: the grace window closes it.
    assert!(session.release_output(attachment.lease));
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(
        manager.get(&created.session_id).unwrap_err().kind(),
        ptrack_terminal::ManagerErrorKind::SessionNotFound
    );
    assert!(
        identity
            .calls
            .lock()
            .unwrap()
            .contains(&format!("revoke:{}", created.session_id))
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn runtime_drop_revokes_authority_closes_sessions_and_preserves_join() {
    let root = TempDirectory::new();
    let manager = Manager::new(&root.0, vec![profile(&root.0)], Arc::new(TestFactory))
        .await
        .unwrap();
    let identity = Arc::new(TestIdentity::default());
    let runtime = TerminalRuntime::new(TerminalRuntimeConfig {
        generation: 11,
        project_root: root.0.clone(),
        manager: Arc::clone(&manager),
        identity: identity.clone(),
        events: Arc::new(TestEvents::default()),
        attachment_lease: Duration::from_secs(30),
    })
    .unwrap();
    let session = runtime.create(11, "shell-default", None, 24, 80).unwrap();

    drop(runtime);

    assert!(
        identity
            .calls
            .lock()
            .unwrap()
            .contains(&format!("revoke:{}", session.session_id))
    );
    assert_eq!(
        manager.get(&session.session_id).unwrap_err().kind(),
        ptrack_terminal::ManagerErrorKind::SessionNotFound
    );
    assert_eq!(
        manager
            .create("shell-default", None, 24, 80)
            .unwrap_err()
            .kind(),
        ptrack_terminal::ManagerErrorKind::Shutdown
    );
    tokio::time::timeout(Duration::from_secs(1), manager.shutdown())
        .await
        .expect("drop-requested shutdown must remain joinable")
        .unwrap();
}

#[tokio::test]
async fn terminal_runtime_rejects_a_manager_for_another_project_root() {
    let root = TempDirectory::new();
    let other = TempDirectory::new();
    let manager = Manager::new(&root.0, vec![profile(&root.0)], Arc::new(TestFactory))
        .await
        .unwrap();

    let error = TerminalRuntime::new(TerminalRuntimeConfig {
        generation: 12,
        project_root: other.0.clone(),
        manager,
        identity: Arc::new(TestIdentity::default()),
        events: Arc::new(TestEvents::default()),
        attachment_lease: Duration::from_secs(30),
    })
    .err()
    .expect("mismatched manager root must be rejected");

    assert!(error.to_string().contains("does not match"));
}

#[tokio::test]
async fn linked_launch_publishes_paired_revision_and_rolls_back_after_close() {
    let root = TempDirectory::new();
    let factory = Arc::new(CapturingFactory::default());
    let manager = Manager::new(
        &root.0,
        vec![profile(&root.0), agent_profile(&root.0)],
        factory.clone(),
    )
    .await
    .unwrap();
    let identity = Arc::new(LinkedIdentity::default());
    let events = Arc::new(TestEvents::default());
    let runtime = TerminalRuntime::new(TerminalRuntimeConfig {
        generation: 17,
        project_root: root.0.clone(),
        manager: manager.clone(),
        identity: identity.clone(),
        events: events.clone(),
        attachment_lease: Duration::from_secs(30),
    })
    .unwrap();

    let linked = runtime
        .create_linked(
            17,
            "agent-test",
            None,
            24,
            80,
            ptrack_terminal::TerminalAssociationPointer {
                version: 1,
                plan_id: 4,
                task_id: 9,
            },
            "bounded launch context",
        )
        .unwrap();
    assert!(linked.linked_launch);
    assert_eq!(linked.association_revision, Some(1));
    assert_eq!(
        events.order.lock().unwrap().as_slice(),
        &["status", "runtime"]
    );
    assert!(
        factory.0.lock().unwrap()[0]
            .env
            .contains(&"PTRACK_LAUNCH_CONTEXT_V1=bounded launch context".to_owned())
    );
    let info = manager.get(&linked.session_id).unwrap().info();
    assert_eq!(info.association.unwrap().pointer.task_id, 9);

    runtime.rollback_linked(17, &linked.session_id).unwrap();
    assert_eq!(
        manager.get(&linked.session_id).unwrap_err().kind(),
        ptrack_terminal::ManagerErrorKind::SessionNotFound
    );
    {
        let calls = identity.calls.lock().unwrap();
        assert_eq!(calls[0], format!("bind-linked:{}", linked.session_id));
        assert_eq!(calls[1], format!("revoke:{}", linked.session_id));
        assert_eq!(calls[2], format!("remove:{}", linked.session_id));
    }
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn linked_post_spawn_validation_joins_forced_cleanup_failure() {
    let root = TempDirectory::new();
    let outside = TempDirectory::new();
    let mut selected = agent_profile(&root.0);
    selected.cwd_policy = CwdPolicy::Fixed;
    selected.fixed_cwd = outside.0.to_string_lossy().into_owned();
    let manager = Manager::new(&root.0, vec![selected], Arc::new(KillErrorFactory))
        .await
        .unwrap();
    let identity = Arc::new(LinkedIdentity::default());
    let runtime = TerminalRuntime::new(TerminalRuntimeConfig {
        generation: 17,
        project_root: root.0.clone(),
        manager: manager.clone(),
        identity,
        events: Arc::new(TestEvents::default()),
        attachment_lease: Duration::from_secs(30),
    })
    .unwrap();
    let error = runtime
        .create_linked(
            17,
            "agent-test",
            Some(&root.0),
            24,
            80,
            ptrack_terminal::TerminalAssociationPointer {
                version: 1,
                plan_id: 4,
                task_id: 9,
            },
            "bounded launch context",
        )
        .unwrap_err()
        .to_string();
    assert!(
        error.starts_with("launched terminal working directory does not match validated CWD\n")
    );
    assert!(error.contains("kill terminal process: forced cleanup failed"));
    assert_eq!(manager.runtime_session_snapshot_bounded(1).1, 0);
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn published_linked_failure_uses_failure_order_and_surfaces_force_close_error() {
    let root = TempDirectory::new();
    let manager = Manager::new(
        &root.0,
        vec![agent_profile(&root.0)],
        Arc::new(KillErrorFactory),
    )
    .await
    .unwrap();
    let identity = Arc::new(LinkedIdentity::default());
    let runtime = TerminalRuntime::new(TerminalRuntimeConfig {
        generation: 19,
        project_root: root.0.clone(),
        manager,
        identity: identity.clone(),
        events: Arc::new(TestEvents::default()),
        attachment_lease: Duration::from_secs(30),
    })
    .unwrap();
    let linked = runtime
        .create_linked(
            19,
            "agent-test",
            None,
            24,
            80,
            ptrack_terminal::TerminalAssociationPointer {
                version: 1,
                plan_id: 4,
                task_id: 9,
            },
            "bounded launch context",
        )
        .unwrap();

    let error = runtime
        .rollback_failed_linked(19, &linked.session_id)
        .unwrap_err()
        .to_string();
    assert!(error.contains("kill terminal process: forced cleanup failed"));
    assert_eq!(
        identity.calls.lock().unwrap().as_slice(),
        [
            format!("bind-linked:{}", linked.session_id),
            format!("revoke-failed:{}", linked.session_id),
            format!("remove:{}", linked.session_id),
        ]
    );
    runtime.shutdown().await.unwrap();
}
