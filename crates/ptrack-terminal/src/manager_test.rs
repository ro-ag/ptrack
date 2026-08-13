use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::profile::{
    CwdPolicy, DEFAULT_PROFILE_FONT_FAMILY, DEFAULT_PROFILE_FONT_SIZE, DEFAULT_PROFILE_SCROLLBACK,
    DEFAULT_PROFILE_THEME, ExitBehavior, Profile, ProfileKind,
};
use crate::pty::{PtyFactory, PtyProcess, StartRequest};
use crate::session::SessionState;
use crate::{Manager, ManagerErrorKind, ShellIntegrationQuality};

#[derive(Default)]
struct ProcessState {
    exited: bool,
    killed: bool,
    closed: bool,
}

#[derive(Default)]
struct ManagerProcess {
    state: Mutex<ProcessState>,
    changed: Condvar,
}

impl PtyProcess for ManagerProcess {
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

struct ManagerFactory {
    starts: Arc<Mutex<Vec<StartRequest>>>,
}

struct ExitedFactory;

struct ExitedProcess;

impl PtyFactory for ExitedFactory {
    fn start(&self, _request: StartRequest) -> io::Result<Box<dyn PtyProcess>> {
        Ok(Box::new(ExitedProcess))
    }
}

impl PtyProcess for ExitedProcess {
    fn pid(&self) -> u32 {
        31338
    }
    fn read(&self, _buffer: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
    fn write(&self, _buffer: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "exited"))
    }
    fn resize(&self, _rows: u16, _columns: u16) -> io::Result<()> {
        Ok(())
    }
    fn wait(&self) -> io::Result<i32> {
        Ok(0)
    }
    fn terminate(&self) -> io::Result<()> {
        Ok(())
    }
    fn kill(&self) -> io::Result<()> {
        Ok(())
    }
    fn close(&self) -> io::Result<()> {
        Ok(())
    }
}

struct BlockingFactory {
    entered: Arc<(Mutex<bool>, Condvar)>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

impl PtyFactory for BlockingFactory {
    fn start(&self, _request: StartRequest) -> io::Result<Box<dyn PtyProcess>> {
        let (entered, changed) = &*self.entered;
        *entered.lock().unwrap() = true;
        changed.notify_all();
        let (release, changed) = &*self.release;
        let mut released = release.lock().unwrap();
        while !*released {
            released = changed.wait(released).unwrap();
        }
        Ok(Box::new(ManagerProcess::default()))
    }
}

impl PtyFactory for ManagerFactory {
    fn start(&self, request: StartRequest) -> io::Result<Box<dyn PtyProcess>> {
        self.starts.lock().unwrap().push(request);
        Ok(Box::new(ManagerProcess::default()))
    }
}

fn profile(executable: &str) -> Profile {
    Profile {
        id: "shell-default".to_owned(),
        name: "Shell".to_owned(),
        kind: ProfileKind::Shell,
        provider: String::new(),
        executable: executable.to_owned(),
        args: Vec::new(),
        env: BTreeMap::new(),
        theme: DEFAULT_PROFILE_THEME.to_owned(),
        font_family: DEFAULT_PROFILE_FONT_FAMILY.to_owned(),
        font_size: DEFAULT_PROFILE_FONT_SIZE,
        scrollback: DEFAULT_PROFILE_SCROLLBACK,
        cwd_policy: CwdPolicy::Requested,
        fixed_cwd: String::new(),
        exit_behavior: ExitBehavior::Keep,
    }
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ptrack-terminal-manager-{}-{}",
            std::process::id(),
            getrandom::u64().unwrap()
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

#[tokio::test]
async fn manager_owns_listener_sessions_crypto_values_and_shutdown() {
    let root = TempDirectory::new();
    let starts = Arc::new(Mutex::new(Vec::new()));
    let factory = Arc::new(ManagerFactory {
        starts: Arc::clone(&starts),
    });
    let executable = if cfg!(windows) {
        "cmd.exe"
    } else {
        "/bin/bash"
    };
    let manager = Manager::new(&root.0, vec![profile(executable)], factory)
        .await
        .unwrap();

    let first = manager.create("shell-default", None, 24, 80).unwrap();
    let second = manager.create("shell-default", None, 40, 120).unwrap();
    assert_eq!(first.id().len(), 43);
    assert_eq!(first.stream_token().len(), 43);
    if cfg!(windows) {
        assert_eq!(
            first.shell_integration().quality,
            ShellIntegrationQuality::None
        );
        assert!(first.shell_integration().nonce.is_empty());
        let values = [
            first.id(),
            first.stream_token(),
            second.id(),
            second.stream_token(),
        ];
        let unique: std::collections::BTreeSet<_> = values.into_iter().collect();
        assert_eq!(unique.len(), 4);
    } else {
        assert_eq!(
            first.shell_integration().quality,
            ShellIntegrationQuality::Rich
        );
        assert_eq!(first.shell_integration().nonce.len(), 43);
        let values = [
            first.id(),
            first.stream_token(),
            &first.shell_integration().nonce,
            second.id(),
            second.stream_token(),
            &second.shell_integration().nonce,
        ];
        let unique: std::collections::BTreeSet<_> = values.into_iter().collect();
        assert_eq!(unique.len(), 6);
    }

    let first_url = manager.session_url(first.id()).unwrap();
    let second_url = manager.session_url(second.id()).unwrap();
    assert!(first_url.starts_with("ws://127.0.0.1:"));
    assert!(first_url.contains(&format!(
        "/terminal/{}?token={}",
        first.id(),
        first.stream_token()
    )));
    assert_eq!(first_url.split('/').nth(2), second_url.split('/').nth(2));

    let (snapshot, total) = manager.session_snapshot_bounded(0);
    assert_eq!((snapshot.len(), total), (2, 2));
    assert!(
        snapshot
            .iter()
            .all(|info| info.pid == 31337 && info.state == SessionState::Running)
    );
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(!json.contains(first.stream_token()));
    if !first.shell_integration().nonce.is_empty() {
        assert!(!json.contains(&first.shell_integration().nonce));
    }

    manager.close_session(first.id(), true).unwrap();
    assert_eq!(
        manager.get(first.id()).unwrap_err().kind(),
        ManagerErrorKind::SessionNotFound
    );
    manager.shutdown().await.unwrap();
    manager.shutdown().await.unwrap();
    assert_eq!(
        manager
            .create("shell-default", None, 24, 80)
            .unwrap_err()
            .kind(),
        ManagerErrorKind::Shutdown
    );
}

#[tokio::test]
async fn manager_defensively_owns_launch_data_and_validates_cwd_and_env() {
    let root = TempDirectory::new();
    let child = root.0.join("child");
    std::fs::create_dir(&child).unwrap();
    let starts = Arc::new(Mutex::new(Vec::new()));
    let factory = Arc::new(ManagerFactory {
        starts: Arc::clone(&starts),
    });
    let executable = if cfg!(windows) { "cmd.exe" } else { "/bin/sh" };
    let mut source = profile(executable);
    source.args = vec!["-i".to_owned()];
    source
        .env
        .insert("PROFILE_VALUE".to_owned(), "owned".to_owned());
    let manager = Manager::new(&root.0, vec![source.clone()], factory)
        .await
        .unwrap();
    source.args[0] = "mutated".to_owned();
    source
        .env
        .insert("PROFILE_VALUE".to_owned(), "mutated".to_owned());
    let extra = BTreeMap::from([("HOST_VALUE".to_owned(), "safe".to_owned())]);
    let session = manager
        .create_with_env("shell-default", Some(&child), 24, 80, &extra)
        .unwrap();
    let launch = starts.lock().unwrap()[0].clone();
    assert_eq!(launch.args, vec!["-i"]);
    assert_eq!(launch.cwd, child);
    assert!(
        launch
            .env
            .iter()
            .any(|entry| entry == "PROFILE_VALUE=owned")
    );
    assert!(launch.env.iter().any(|entry| entry == "HOST_VALUE=safe"));

    let unsafe_env = BTreeMap::from([("BAD=KEY".to_owned(), "value".to_owned())]);
    assert!(
        manager
            .create_with_env("shell-default", None, 24, 80, &unsafe_env)
            .is_err()
    );
    assert!(manager.create("missing", None, 24, 80).is_err());
    assert!(
        manager
            .create("shell-default", Some(&root.0.join("missing")), 24, 80)
            .is_err()
    );
    manager.close_session(session.id(), false).unwrap();
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn synchronous_shutdown_request_fences_admission_and_remains_joinable() {
    let root = TempDirectory::new();
    let manager = Manager::new(
        &root.0,
        vec![profile(if cfg!(windows) { "cmd.exe" } else { "/bin/sh" })],
        Arc::new(ManagerFactory {
            starts: Arc::new(Mutex::new(Vec::new())),
        }),
    )
    .await
    .unwrap();
    let session = manager.create("shell-default", None, 24, 80).unwrap();

    manager.request_shutdown();

    assert_eq!(
        manager
            .create("shell-default", None, 24, 80)
            .unwrap_err()
            .kind(),
        ManagerErrorKind::Shutdown
    );
    tokio::time::timeout(Duration::from_secs(1), manager.shutdown())
        .await
        .expect("requested shutdown must remain joinable")
        .unwrap();
    assert_eq!(
        manager.get(session.id()).unwrap_err().kind(),
        ManagerErrorKind::SessionNotFound
    );
}

#[tokio::test]
async fn aborting_first_shutdown_waiter_does_not_abandon_owned_teardown() {
    let root = TempDirectory::new();
    let entered = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let manager = Manager::new(
        &root.0,
        vec![profile(if cfg!(windows) { "cmd.exe" } else { "/bin/sh" })],
        Arc::new(BlockingFactory {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }),
    )
    .await
    .unwrap();
    let creator_manager = Arc::clone(&manager);
    let creator = std::thread::spawn(move || creator_manager.create("shell-default", None, 24, 80));
    {
        let (was_entered, changed) = &*entered;
        let mut was_entered = was_entered.lock().unwrap();
        while !*was_entered {
            was_entered = changed.wait(was_entered).unwrap();
        }
    }

    let first_manager = Arc::clone(&manager);
    let first = tokio::spawn(async move { first_manager.shutdown().await });
    loop {
        if manager.create("missing", None, 24, 80).unwrap_err().kind() == ManagerErrorKind::Shutdown
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    first.abort();
    let (released, changed) = &*release;
    *released.lock().unwrap() = true;
    changed.notify_all();
    assert_eq!(
        creator.join().unwrap().unwrap_err().kind(),
        ManagerErrorKind::Shutdown
    );

    tokio::time::timeout(Duration::from_secs(1), manager.shutdown())
        .await
        .expect("manager-owned shutdown must survive waiter cancellation")
        .unwrap();
}

#[tokio::test]
async fn lifecycle_enumeration_is_not_truncated_at_the_snapshot_cap() {
    const SESSION_COUNT: usize = crate::MAX_RUNTIME_SESSION_CANDIDATES + 1;

    let root = TempDirectory::new();
    let manager = Manager::new(
        &root.0,
        vec![profile(if cfg!(windows) { "cmd.exe" } else { "/bin/sh" })],
        Arc::new(ExitedFactory),
    )
    .await
    .unwrap();
    for _ in 0..SESSION_COUNT {
        manager.create("shell-default", None, 24, 80).unwrap();
    }

    let (bounded, total) = manager.runtime_session_snapshot_bounded(0);
    assert_eq!(bounded.len(), crate::MAX_RUNTIME_SESSION_CANDIDATES);
    assert_eq!(total, SESSION_COUNT);
    assert_eq!(manager.lifecycle_session_ids().len(), SESSION_COUNT);

    manager.shutdown().await.unwrap();
}
