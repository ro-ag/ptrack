//! The desktop coordinator, [`DesktopRuntime`]: it owns which project
//! workspace is open, fences every call against shutdown and authority
//! changes, and answers the application-scoped commands itself.
//!
//! `application` serves settings, terminal windows, and the recent-projects
//! registry; `initialization` serves first-run project initialization;
//! `lifecycle` opens, closes, and switches the workspace; `watcher` runs the
//! per-workspace change watchers.

mod application;
mod initialization;
mod lifecycle;
mod watcher;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

use super::admission::DesktopAdmissionFence;
use super::command::{DesktopCommand, NativeCommand, UpdateCommand};
use super::ports::{
    DesktopEventSink, DesktopInitializationService, DesktopRuntimeConfig, DesktopWorkspace,
    DesktopWorkspaceFactory, RecentProjectsProvider,
};
use super::support::{lock, unavailable, value};
use super::wire::{
    ActiveResourceSummary, DesktopCommandRequest, DesktopEvent, DesktopNotificationSnapshotV1,
    InitializeProjectRequestV1, InitializeProjectResultV1, ScratchpadChangedV1, ShutdownOutcome,
    WorkspaceState, WorkspaceStatus, validate_request,
};
use super::{DEFAULT_CONFIRMATION_TTL, RUNTIME_CALL_TIMEOUT, SHUTDOWN_RETRY_INTERVAL};
use crate::terminal_windows::{OpenedTerminalWindow, TerminalWindowTab, TerminalWindows};
use crate::{AppError, AppResult};

#[cfg(test)]
pub(crate) use application::{
    apply_preferences, record_last_project_in, reset_application_records,
};
use watcher::WorkspaceWatcher;
#[cfg(test)]
pub(crate) use watcher::watch_workspace_data;

struct Confirmation {
    token: String,
    action: ConfirmationAction,
    path: PathBuf,
    generation: u64,
    resource_revision: u64,
    resources: ActiveResourceSummary,
    expires_at: Instant,
    _expiry_cancellation: Sender<()>,
    _admission: DesktopAdmissionFence,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ConfirmationAction {
    Open,
    Close,
}

struct RuntimeState {
    status: WorkspaceStatus,
    generation: u64,
    workspace: Option<Arc<dyn DesktopWorkspace>>,
    error: String,
    confirmation: Option<Confirmation>,
    shutting_down: bool,
    shutdown_retry: bool,
    authority_changing: bool,
    active_calls: usize,
    completed_initialization: Option<CompletedInitializationReplay>,
}

struct CompletedInitializationReplay {
    request: InitializeProjectRequestV1,
    result: InitializeProjectResultV1,
}

/// Lease for a native action that must finish before desktop shutdown.
pub struct DesktopNativeActionLease {
    runtime: Arc<DesktopRuntime>,
}

impl Drop for DesktopNativeActionLease {
    fn drop(&mut self) {
        let mut state = lock(&self.runtime.state);
        state.active_calls = state.active_calls.saturating_sub(1);
        drop(state);
        self.runtime.calls_changed.notify_all();
    }
}

pub struct DesktopRuntime {
    version: String,
    factory: Arc<dyn DesktopWorkspaceFactory>,
    event_sink: Option<Arc<dyn DesktopEventSink>>,
    transition: Mutex<()>,
    recent_mutation: Mutex<()>,
    state: Mutex<RuntimeState>,
    calls_changed: Condvar,
    watcher: Mutex<Option<WorkspaceWatcher>>,
    global_binding: Mutex<Option<CachedGlobalBinding>>,
    terminal_windows: Mutex<TerminalWindows>,
    /// Windows an open expired on its own; the next sweep hands them to the
    /// shell to close.
    expired_terminal_windows: Mutex<Vec<String>>,
    recent_projects: Arc<dyn RecentProjectsProvider>,
    initialization: Arc<dyn DesktopInitializationService>,
    update_service: Arc<dyn crate::DesktopUpdateService>,
    confirmation_ttl: Duration,
}

impl DesktopRuntime {
    #[must_use]
    pub fn new(config: DesktopRuntimeConfig) -> Arc<Self> {
        let initial = config.initial_workspace.clone();
        let (status, generation) = if config.initial_workspace.is_some() {
            (WorkspaceStatus::Open, 1)
        } else {
            (WorkspaceStatus::Welcome, 0)
        };
        let runtime = Arc::new(Self {
            version: config.version,
            factory: config.factory,
            event_sink: config.event_sink,
            transition: Mutex::new(()),
            recent_mutation: Mutex::new(()),
            state: Mutex::new(RuntimeState {
                status,
                generation,
                workspace: config.initial_workspace,
                error: String::new(),
                confirmation: None,
                shutting_down: false,
                shutdown_retry: false,
                authority_changing: false,
                active_calls: 0,
                completed_initialization: None,
            }),
            calls_changed: Condvar::new(),
            watcher: Mutex::new(None),
            global_binding: Mutex::new(None),
            terminal_windows: Mutex::new(TerminalWindows::default()),
            expired_terminal_windows: Mutex::new(Vec::new()),
            recent_projects: config.recent_projects,
            initialization: config.initialization,
            update_service: config.update_service,
            confirmation_ttl: if config.confirmation_ttl.is_zero() {
                DEFAULT_CONFIRMATION_TTL
            } else {
                config.confirmation_ttl
            },
        });
        let _ = runtime.update_service.start();
        if let Some(workspace) = initial {
            let project = workspace.project();
            runtime.start_workspace_watcher(1, PathBuf::from(project.db_path), workspace);
        }
        runtime
    }

    /// Dispatches one size-bounded allowlisted desktop request.
    ///
    /// # Errors
    /// Returns validation, lifecycle, or command-specific errors.
    #[allow(clippy::needless_pass_by_value)]
    pub fn invoke(self: &Arc<Self>, request: DesktopCommandRequest) -> AppResult<Value> {
        validate_request(&request)?;
        match DesktopCommand::parse(&request.method, &request.arguments)? {
            DesktopCommand::Settings(command) => self.settings_command(command),
            DesktopCommand::TerminalWindow(command) => self.terminal_window_command(command),
            DesktopCommand::Initialization(command) => self.initialization_command(command),
            DesktopCommand::Lifecycle(command) => self.lifecycle_command(command),
            DesktopCommand::Update(command) => self.update_command(command),
            DesktopCommand::Native(command) => self.native_command(command),
            DesktopCommand::Recent(command) => self.recent_command(command),
            DesktopCommand::Workspace => {
                let reply = self.with_workspace(&request.method, &request.arguments)?;
                if request.method == "SetScratchpadV1"
                    && let Some(change) = ScratchpadChangedV1::from_reply(&reply)
                {
                    // Only a write that landed: a refused or conflicting one
                    // changed nothing another window needs to re-read.
                    self.emit(DesktopEvent::ScratchpadChanged(change));
                }
                Ok(reply)
            }
        }
    }

    fn update_command(self: &Arc<Self>, command: UpdateCommand<'_>) -> AppResult<Value> {
        let _lease = self.begin_native_action()?;
        match command {
            UpdateCommand::GetState => value(self.update_service.state()),
            UpdateCommand::CancelOperation => value(self.update_service.cancel_operation()),
            UpdateCommand::SetAutomaticChecks { enabled } => value(
                self.update_service
                    .set_automatic_checks(enabled)
                    .map_err(AppError::Message)?,
            ),
            UpdateCommand::Check => value(
                self.update_service
                    .check_for_updates()
                    .map_err(AppError::Message)?,
            ),
            UpdateCommand::Download { expected_version } => value(
                self.update_service
                    .download_update(expected_version)
                    .map_err(AppError::Message)?,
            ),
            UpdateCommand::Apply { expected_version } => value(
                self.update_service
                    .apply_update(expected_version)
                    .map_err(AppError::Message)?,
            ),
        }
    }

    fn native_command(self: &Arc<Self>, command: NativeCommand<'_>) -> AppResult<Value> {
        match command {
            NativeCommand::OpenHelpDestination { destination } => {
                let _lease = self.begin_native_action()?;
                Ok(Value::String(help_destination(destination)?.to_owned()))
            }
            NativeCommand::PickProjectDirectory => Err(unavailable("directory picker")),
            NativeCommand::InstallShellCommand => {
                let _lease = self.begin_native_action()?;
                value(crate::install_shell_command().message)
            }
        }
    }

    /// The generation a terminal-window assignment is fenced by: the open
    /// workspace's generation, and nothing at all while no project is open.
    fn terminal_window_fence(&self) -> Option<u64> {
        let state = lock(&self.state);
        (state.status == WorkspaceStatus::Open).then_some(state.generation)
    }

    /// Records one window assignment and returns its minted label. The shell
    /// builds the window from that label and calls `close_terminal_window` if
    /// the build fails, so a failed pop-out never leaves a session unowned.
    ///
    /// The fence is read inside the window-map lock: read before it, a
    /// project switch landing in between would let this open record the old
    /// generation as current and expire the new workspace's windows.
    ///
    /// # Errors
    /// Returns an error with no project open, without at least one session,
    /// when any session is already shown by a window, or at the window limit.
    pub fn open_terminal_window(&self, tab: TerminalWindowTab) -> AppResult<OpenedTerminalWindow> {
        let mut windows = lock(&self.terminal_windows);
        let fence = self.terminal_window_fence();
        windows.open(fence, tab)
    }

    /// The tab one terminal window owns, or `None` for an unknown label.
    #[must_use]
    pub fn terminal_window_tab(&self, label: &str) -> Option<TerminalWindowTab> {
        lock(&self.terminal_windows).tab(label).cloned()
    }

    /// Replaces one window's tab after a split changed inside it.
    ///
    /// # Errors
    /// Returns an error for an unknown label, without at least one session, or
    /// when any session belongs to a different window.
    pub fn set_terminal_window_tab(&self, label: &str, tab: TerminalWindowTab) -> AppResult<()> {
        lock(&self.terminal_windows).set_tab(label, tab)
    }

    /// Clears one assignment and reports the tab it freed, once: the shell
    /// pops a tab back in exactly when this answers `Some`, so a second call
    /// for the same window must free nothing.
    pub fn close_terminal_window(&self, label: &str) -> Option<TerminalWindowTab> {
        lock(&self.terminal_windows).close(label)
    }

    /// Labels whose workspace is gone — a switched or closed project — so the
    /// shell can close their windows. Empty while the workspace is unchanged.
    pub fn expire_terminal_windows(&self) -> Vec<String> {
        let mut labels = {
            let mut windows = lock(&self.terminal_windows);
            let fence = self.terminal_window_fence();
            windows.expire(fence)
        };
        labels.append(&mut lock(&self.expired_terminal_windows));
        labels
    }

    /// Clears every assignment and reports the labels, for app shutdown.
    pub fn drain_terminal_windows(&self) -> Vec<String> {
        let mut labels = lock(&self.terminal_windows).drain();
        labels.append(&mut lock(&self.expired_terminal_windows));
        labels
    }

    #[must_use]
    pub fn workspace_state(&self) -> WorkspaceState {
        let state = lock(&self.state);
        state_view(&state, &self.version)
    }

    /// Returns the bounded identifier-only state consumed by the native OS
    /// notification policy. Welcome/closed workspaces are an empty baseline.
    ///
    /// # Errors
    /// Returns lifecycle or coordinator projection errors.
    pub fn notification_snapshot(&self) -> AppResult<DesktopNotificationSnapshotV1> {
        let (generation, workspace) = {
            let mut state = lock(&self.state);
            if state.shutting_down {
                return Err(shutting_down());
            }
            if state.authority_changing {
                return Err(AppError::Message(
                    "runtime authority is changing".to_owned(),
                ));
            }
            let generation = state.generation;
            let Some(workspace) = state
                .workspace
                .clone()
                .filter(|_| state.status == WorkspaceStatus::Open)
            else {
                return Ok(DesktopNotificationSnapshotV1 {
                    generation,
                    events: Vec::new(),
                });
            };
            state.active_calls = state.active_calls.saturating_add(1);
            drop(state);
            (generation, workspace)
        };
        let _lease = DesktopCallLease { runtime: self };
        let mut snapshot = workspace.notification_snapshot()?;
        snapshot.generation = generation;
        Ok(snapshot)
    }

    /// Fences new calls, drains active leases, and tears down the workspace.
    ///
    /// # Errors
    /// Returns a bounded drain timeout or workspace teardown error.
    pub fn begin_shutdown(&self) -> AppResult<()> {
        {
            let mut state = lock(&self.state);
            if state.authority_changing {
                return Err(AppError::Message(
                    "runtime authority is changing".to_owned(),
                ));
            }
            if state.shutting_down && state.workspace.is_none() {
                return Ok(());
            }
            if state.shutting_down && !state.shutdown_retry {
                return Err(shutting_down());
            }
            state.shutting_down = true;
            state.shutdown_retry = false;
            state.confirmation = None;
        }
        // An in-flight download or install holds a native-action lease, so it
        // is asked to stop before the drain; a cancel is not permanent, and a
        // refused close below leaves the update service exactly as usable as
        // it was. The permanent update shutdown only runs once every call has
        // drained and the close can no longer be refused by them.
        let _ = self.update_service.cancel_operation();
        self.drain_calls_for_close()?;
        if let Err(error) = self.update_service.shutdown() {
            lock(&self.state).shutting_down = false;
            return Err(AppError::Message(error));
        }
        let _transition = lock(&self.transition);
        let workspace = lock(&self.state).workspace.clone();
        self.stop_workspace_watcher();
        if let Some(workspace) = workspace
            && let Err(error) = workspace.shutdown()
        {
            lock(&self.state).shutdown_retry = true;
            return Err(error);
        }
        let mut state = lock(&self.state);
        state.workspace = None;
        state.status = WorkspaceStatus::Closed;
        drop(state);
        Ok(())
    }

    /// Runs [`Self::begin_shutdown`] on its own thread and waits at most
    /// `bound` for it, so neither a window close nor an app quit can hang on a
    /// slow teardown. With `retry` a refusal is retried until the bound: the
    /// app is quitting and a call still in flight must not keep it alive, so
    /// the teardown keeps trying until the calls drain or the time is up.
    ///
    /// A teardown still running at the bound keeps running on its thread;
    /// the caller decides whether to wait for the process to end anyway.
    pub fn shutdown_within(self: &Arc<Self>, bound: Duration, retry: bool) -> ShutdownOutcome {
        let deadline = Instant::now() + bound;
        let (sender, receiver) = channel();
        let runtime = Arc::clone(self);
        let spawned = thread::Builder::new()
            .name("ptrack-shutdown".to_owned())
            .spawn(move || {
                let result = loop {
                    match runtime.begin_shutdown() {
                        Ok(()) => break Ok(runtime.drain_terminal_windows()),
                        Err(_) if retry && Instant::now() + SHUTDOWN_RETRY_INTERVAL < deadline => {
                            thread::sleep(SHUTDOWN_RETRY_INTERVAL);
                        }
                        Err(error) => break Err(error.to_string()),
                    }
                };
                let _ = sender.send(result);
            });
        if let Err(error) = spawned {
            return ShutdownOutcome::Refused(format!("runtime shutdown could not start: {error}"));
        }
        match receiver.recv_timeout(bound) {
            Ok(Ok(windows)) => ShutdownOutcome::Completed(windows),
            Ok(Err(message)) => ShutdownOutcome::Refused(message),
            Err(_) => ShutdownOutcome::TimedOut,
        }
    }

    /// Waits a bounded time for every admitted call to finish. A timeout
    /// reopens admission and reports the refused close.
    fn drain_calls_for_close(&self) -> AppResult<()> {
        let mut state = lock(&self.state);
        let deadline = Instant::now() + RUNTIME_CALL_TIMEOUT;
        while state.active_calls != 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (next, _) = self
                .calls_changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
        }
        let drained = state.active_calls == 0;
        if !drained {
            state.shutting_down = false;
        }
        drop(state);
        if drained {
            Ok(())
        } else {
            Err(AppError::Message(
                "runtime calls did not finish before close".to_owned(),
            ))
        }
    }

    /// Acquires one shutdown-fenced lease for a native menu, dialog, browser,
    /// or clipboard action.
    ///
    /// # Errors
    /// Returns the exact shutdown fence error once close has begun.
    pub fn begin_native_action(self: &Arc<Self>) -> AppResult<DesktopNativeActionLease> {
        let mut state = lock(&self.state);
        if state.shutting_down {
            return Err(shutting_down());
        }
        if state.authority_changing {
            return Err(AppError::Message(
                "runtime authority is changing".to_owned(),
            ));
        }
        state.active_calls = state.active_calls.saturating_add(1);
        drop(state);
        Ok(DesktopNativeActionLease {
            runtime: Arc::clone(self),
        })
    }

    fn with_workspace(&self, method: &str, arguments: &[Value]) -> AppResult<Value> {
        self.with_open_workspace(|workspace| workspace.invoke(method, arguments))
    }

    /// Runs one call against the open workspace under the same admission and
    /// call lease as an IPC command.
    fn with_open_workspace<T>(
        &self,
        call: impl FnOnce(&dyn DesktopWorkspace) -> AppResult<T>,
    ) -> AppResult<T> {
        let workspace = {
            let mut state = lock(&self.state);
            if state.shutting_down {
                return Err(shutting_down());
            }
            if state.authority_changing {
                return Err(AppError::Message(
                    "runtime authority is changing".to_owned(),
                ));
            }
            if state.status != WorkspaceStatus::Open {
                return Err(AppError::Message("no project workspace is open".to_owned()));
            }
            let workspace = state
                .workspace
                .clone()
                .ok_or_else(|| AppError::Message("no project workspace is open".to_owned()))?;
            state.active_calls = state.active_calls.saturating_add(1);
            workspace
        };
        let _lease = DesktopCallLease { runtime: self };
        call(workspace.as_ref())
    }

    fn emit(&self, event: DesktopEvent) {
        if let Some(sink) = &self.event_sink {
            sink.emit(event);
        }
    }

    fn require_not_shutting_down(&self) -> AppResult<()> {
        if lock(&self.state).shutting_down {
            Err(shutting_down())
        } else {
            Ok(())
        }
    }
}

/// The attested global-store binding and the marker it was read under.
#[derive(Clone)]
struct CachedGlobalBinding {
    home: PathBuf,
    stamp: Option<MarkerStamp>,
    database: PathBuf,
    binding: ptrack_store::ActiveBinding,
}

/// Identifies one version of the generation marker file. Every publication
/// replaces the file, so a new marker changes its identity or modification
/// time even when its length happens to match.
#[derive(Clone, Debug, Eq, PartialEq)]
struct MarkerStamp {
    length: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    inode: u64,
}

fn marker_stamp(home: &Path) -> Option<MarkerStamp> {
    let metadata = fs::metadata(
        home.join("runtime")
            .join(ptrack_store::ACTIVE_GENERATION_MARKER),
    )
    .ok()?;
    Some(MarkerStamp {
        length: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        inode: std::os::unix::fs::MetadataExt::ino(&metadata),
    })
}

impl Drop for DesktopRuntime {
    fn drop(&mut self) {
        if let Some(watcher) = self
            .watcher
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            watcher.stop();
        }
        let workspace = self
            .state
            .get_mut()
            .ok()
            .and_then(|state| state.workspace.take());
        if let Some(workspace) = workspace {
            let _ = workspace.shutdown();
        }
    }
}

struct DesktopCallLease<'a> {
    runtime: &'a DesktopRuntime,
}

impl Drop for DesktopCallLease<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.runtime.state);
        state.active_calls = state.active_calls.saturating_sub(1);
        drop(state);
        self.runtime.calls_changed.notify_all();
    }
}

fn shutting_down() -> AppError {
    AppError::Message("terminal lifecycle is shutting down".to_owned())
}

fn state_view(state: &RuntimeState, version: &str) -> WorkspaceState {
    WorkspaceState {
        status: state.status,
        generation: state.generation,
        version: version.to_owned(),
        project: state
            .workspace
            .as_ref()
            .map(|workspace| workspace.project()),
        error: state.error.clone(),
    }
}

fn help_destination(name: &str) -> AppResult<&'static str> {
    match name {
        "help-center" => Ok("https://ro-ag.github.io/ptrack/help/"),
        "keyboard-shortcuts" => Ok("https://ro-ag.github.io/ptrack/help/reference/shortcuts/"),
        "terminals" => Ok("https://ro-ag.github.io/ptrack/help/terminals/"),
        "project-recovery" => Ok("https://ro-ag.github.io/ptrack/help/troubleshooting/"),
        "capabilities" => {
            Ok("https://ro-ag.github.io/ptrack/help/agents-and-capabilities/#capability-model")
        }
        "report-issue" => Ok("https://github.com/ro-ag/ptrack/issues/new"),
        _ => Err(AppError::Message("unknown Help destination".to_owned())),
    }
}
