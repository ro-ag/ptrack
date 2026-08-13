use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use ptrack_terminal::{
    CwdPolicy, DEFAULT_PROFILE_FONT_FAMILY, DEFAULT_PROFILE_FONT_SIZE, DEFAULT_PROFILE_SCROLLBACK,
    DEFAULT_PROFILE_THEME, ExitBehavior, ExitResult, Manager, Profile, ProfileKind, PtyFactory,
    PtyProcess, StartRequest,
};

use super::terminal_runtime::{
    PreparedTerminalIdentity, TerminalEventSink, TerminalExitV2, TerminalIdentityAuthority,
    TerminalRuntime, TerminalRuntimeConfig, TerminalStatusV2,
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
struct TestFactory;

impl PtyFactory for TestFactory {
    fn start(&self, _request: StartRequest) -> io::Result<Box<dyn PtyProcess>> {
        Ok(Box::new(TestProcess::default()))
    }
}

#[derive(Default)]
struct TestIdentity {
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

    fn record_exit(&self, _generation: u64, session_id: &str, _result: &ExitResult) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("exit:{session_id}"));
    }
}

#[derive(Default)]
struct TestEvents {
    statuses: Mutex<Vec<TerminalStatusV2>>,
    exits: Mutex<Vec<TerminalExitV2>>,
    changes: Mutex<Vec<u64>>,
}

impl TerminalEventSink for TestEvents {
    fn status(&self, event: TerminalStatusV2) {
        self.statuses.lock().unwrap().push(event);
    }

    fn exited(&self, event: TerminalExitV2) {
        self.exits.lock().unwrap().push(event);
    }

    fn runtime_changed(&self, generation: u64) {
        self.changes.lock().unwrap().push(generation);
    }
}

fn profile(_root: &Path) -> Profile {
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

#[tokio::test]
async fn app_terminal_host_redacts_profiles_fences_generation_and_revokes_before_close() {
    let root = TempDirectory::new();
    let manager = Manager::new(&root.0, vec![profile(&root.0)], Arc::new(TestFactory))
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
    for secret in ["executable", "args", "env", "fixedCwd"] {
        assert!(
            encoded.get(secret).is_none(),
            "unsafe profile field {secret}"
        );
    }
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
