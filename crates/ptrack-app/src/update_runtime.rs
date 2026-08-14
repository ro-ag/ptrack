use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use ptrack_store::{ActiveBinding, GlobalStore};
use ptrack_updater::{
    ApplyResult, Candidate, Client, Installer, Progress, StagedUpdate, Target, UpdateError,
    compare_versions, discard_stage, load_stage, recover_pending_apply,
};
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio_util::sync::CancellationToken;

const UPDATE_PROGRESS_QUANTUM: u64 = 256 << 10;
const UPDATE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const UPDATE_PREFERENCE_KEY: &[u8] = b"updates.auto-check";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdatePhase {
    Idle,
    Recovering,
    RecoveryRequired,
    Checking,
    Current,
    Available,
    Downloading,
    Ready,
    Applying,
    Canceling,
    Installed,
    ActionRequired,
    Unavailable,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRelease {
    pub version: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub published_at: String,
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub page_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct UpdateState {
    pub revision: u64,
    pub phase: UpdatePhase,
    pub current_version: String,
    pub automatic_checks: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<UpdateRelease>,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub checksum_verified: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_checked_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub apply_action: String,
    pub restart_required: bool,
    pub manual_install: bool,
    pub cleanup_pending: bool,
}

impl UpdateState {
    fn idle(current_version: String) -> Self {
        Self {
            revision: 0,
            phase: UpdatePhase::Idle,
            current_version,
            automatic_checks: false,
            release: None,
            downloaded_bytes: 0,
            total_bytes: 0,
            checksum_verified: false,
            last_checked_at: String::new(),
            error: String::new(),
            apply_action: String::new(),
            restart_required: false,
            manual_install: false,
            cleanup_pending: false,
        }
    }
}

#[allow(clippy::missing_errors_doc)]
pub trait UpdatePreferences: Send + Sync {
    fn load_automatic_checks(&self) -> Result<bool, String>;
    fn save_automatic_checks(&self, enabled: bool) -> Result<(), String>;
}

#[derive(Default)]
pub struct NoUpdatePreferences;

impl UpdatePreferences for NoUpdatePreferences {
    fn load_automatic_checks(&self) -> Result<bool, String> {
        Ok(false)
    }

    fn save_automatic_checks(&self, _enabled: bool) -> Result<(), String> {
        Err("update preferences are unavailable".to_owned())
    }
}

/// Exact global.redb-backed updater opt-in preference authority.
pub struct GlobalStoreUpdatePreferences {
    database: PathBuf,
    binding: ActiveBinding,
}

impl GlobalStoreUpdatePreferences {
    #[must_use]
    pub fn new(database: PathBuf, binding: ActiveBinding) -> Arc<Self> {
        Arc::new(Self { database, binding })
    }

    fn open(&self) -> Result<GlobalStore, String> {
        GlobalStore::open_existing(&self.database, &self.binding)
            .map_err(|_| "update preferences are unavailable".to_owned())
    }
}

impl UpdatePreferences for GlobalStoreUpdatePreferences {
    fn load_automatic_checks(&self) -> Result<bool, String> {
        self.open()?
            .config(UPDATE_PREFERENCE_KEY)
            .map(|value| value == b"true")
            .map_err(|_| "update preferences are unavailable".to_owned())
    }

    fn save_automatic_checks(&self, enabled: bool) -> Result<(), String> {
        self.open()?
            .set_config(
                UPDATE_PREFERENCE_KEY,
                if enabled { b"true" } else { b"false" },
            )
            .map_err(|_| "update preferences are unavailable".to_owned())
    }
}

pub trait UpdateEventSink: Send + Sync {
    fn state_changed(&self, state: UpdateState);
}

#[allow(clippy::missing_errors_doc)]
pub trait DesktopUpdateService: Send + Sync {
    fn start(&self) -> Result<(), String>;
    fn state(&self) -> UpdateState;
    fn set_automatic_checks(&self, enabled: bool) -> Result<UpdateState, String>;
    fn check_for_updates(&self) -> Result<UpdateState, String>;
    fn download_update(&self, expected_version: &str) -> Result<UpdateState, String>;
    fn apply_update(&self, expected_version: &str) -> Result<UpdateState, String>;
    fn cancel_operation(&self) -> UpdateState;
    fn shutdown(&self) -> Result<(), String>;
}

pub struct UnavailableUpdateService {
    state: UpdateState,
}

impl UnavailableUpdateService {
    #[must_use]
    pub fn new(current_version: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            state: UpdateState::idle(current_version.into()),
        })
    }
}

impl DesktopUpdateService for UnavailableUpdateService {
    fn start(&self) -> Result<(), String> {
        Ok(())
    }

    fn state(&self) -> UpdateState {
        self.state.clone()
    }

    fn set_automatic_checks(&self, _enabled: bool) -> Result<UpdateState, String> {
        Err("update service is unavailable".to_owned())
    }

    fn check_for_updates(&self) -> Result<UpdateState, String> {
        Err("update service is unavailable".to_owned())
    }

    fn download_update(&self, _expected_version: &str) -> Result<UpdateState, String> {
        Err("update service is unavailable".to_owned())
    }

    fn apply_update(&self, _expected_version: &str) -> Result<UpdateState, String> {
        Err("update service is unavailable".to_owned())
    }

    fn cancel_operation(&self) -> UpdateState {
        self.state.clone()
    }

    fn shutdown(&self) -> Result<(), String> {
        Ok(())
    }
}

pub(crate) type BackendFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, UpdateError>> + Send + 'a>>;

pub(crate) trait UpdateBackend: Send + Sync {
    fn check<'a>(
        &'a self,
        cancellation: &'a CancellationToken,
        current_version: &'a str,
        target: &'a Target,
    ) -> BackendFuture<'a, Candidate>;
    fn stage<'a>(
        &'a self,
        cancellation: &'a CancellationToken,
        candidate: &'a Candidate,
        target: &'a Target,
        root: &'a Path,
        progress: Arc<dyn Fn(Progress) + Send + Sync>,
    ) -> BackendFuture<'a, StagedUpdate>;
    fn apply<'a>(
        &'a self,
        cancellation: &'a CancellationToken,
        stage: &'a StagedUpdate,
    ) -> BackendFuture<'a, ApplyResult>;
    fn load(
        &self,
        cancellation: &CancellationToken,
        root: &Path,
    ) -> Result<StagedUpdate, UpdateError>;
    fn recover(&self, cancellation: &CancellationToken, root: &Path) -> Result<bool, UpdateError>;
    fn discard(&self, root: &Path) -> Result<(), UpdateError>;
}

struct ProductionUpdateBackend {
    client: Client,
    installer: Installer,
}

impl UpdateBackend for ProductionUpdateBackend {
    fn check<'a>(
        &'a self,
        cancellation: &'a CancellationToken,
        current_version: &'a str,
        target: &'a Target,
    ) -> BackendFuture<'a, Candidate> {
        Box::pin(self.client.check(cancellation, current_version, target))
    }

    fn stage<'a>(
        &'a self,
        cancellation: &'a CancellationToken,
        candidate: &'a Candidate,
        target: &'a Target,
        root: &'a Path,
        progress: Arc<dyn Fn(Progress) + Send + Sync>,
    ) -> BackendFuture<'a, StagedUpdate> {
        Box::pin(async move {
            self.client
                .stage(
                    cancellation,
                    candidate,
                    target,
                    root,
                    Some(progress.as_ref()),
                )
                .await
        })
    }

    fn apply<'a>(
        &'a self,
        cancellation: &'a CancellationToken,
        stage: &'a StagedUpdate,
    ) -> BackendFuture<'a, ApplyResult> {
        Box::pin(self.installer.apply(cancellation, stage))
    }

    fn load(
        &self,
        cancellation: &CancellationToken,
        root: &Path,
    ) -> Result<StagedUpdate, UpdateError> {
        load_stage(cancellation, root)
    }

    fn recover(&self, cancellation: &CancellationToken, root: &Path) -> Result<bool, UpdateError> {
        recover_pending_apply(cancellation, root)
    }

    fn discard(&self, root: &Path) -> Result<(), UpdateError> {
        discard_stage(root)
    }
}

#[allow(clippy::struct_excessive_bools)]
struct RuntimeState {
    public: UpdateState,
    candidate: Option<Candidate>,
    stage: Option<StagedUpdate>,
    active: bool,
    automatic_operation: bool,
    operation: u64,
    cancellation: Option<CancellationToken>,
    recovering: bool,
    blocked: bool,
    shutting_down: bool,
}

struct UpdateCore {
    target: Target,
    root: PathBuf,
    backend: Arc<dyn UpdateBackend>,
    preferences: Arc<dyn UpdatePreferences>,
    preference_lock: Mutex<()>,
    event_sink: Option<Arc<dyn UpdateEventSink>>,
    state: Mutex<RuntimeState>,
    idle: Condvar,
}

#[derive(Clone)]
pub struct UpdateRuntime {
    core: Arc<UpdateCore>,
}

impl UpdateRuntime {
    /// Constructs the production updater for the process-global p-track home.
    /// Automatic-check preferences remain fail-closed until the active global
    /// store binding is installed by the cutover coordinator.
    ///
    /// # Errors
    /// Returns an error when the global home or fixed client is unavailable.
    pub fn for_default_home(
        current_version: impl Into<String>,
        event_sink: Option<Arc<dyn UpdateEventSink>>,
    ) -> Result<Arc<Self>, String> {
        let configured = std::env::var_os("PTRACK_HOME").filter(|value| !value.is_empty());
        let home = configured.or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|value| PathBuf::from(value).join(".ptrack").into_os_string())
        });
        let mut home = home
            .map(PathBuf::from)
            .ok_or_else(|| "updates are unavailable".to_owned())?;
        if !home.is_absolute() {
            home = std::env::current_dir()
                .map_err(|_| "updates are unavailable".to_owned())?
                .join(home);
        }
        Self::new(
            current_version,
            Target::host(),
            home.join("updates"),
            Arc::new(NoUpdatePreferences),
            event_sink,
        )
    }

    /// Constructs the production updater from the exact active global binding.
    /// No network request is made until `start` admits an opted-in check or a
    /// caller explicitly requests one.
    ///
    /// # Errors
    /// Returns an error when the global authority or fixed client is invalid.
    pub fn for_bindings(
        bindings: &crate::WorkspaceBindings,
        event_sink: Option<Arc<dyn UpdateEventSink>>,
    ) -> Result<Arc<Self>, String> {
        let preferences = GlobalStoreUpdatePreferences::new(
            bindings.global_database.clone(),
            bindings.global_binding.clone(),
        );
        Self::new(
            bindings.writer_version.clone(),
            Target::host(),
            bindings.global_home.join("updates"),
            preferences,
            event_sink,
        )
    }

    /// Constructs the production updater without performing network I/O.
    ///
    /// # Errors
    /// Returns an error if the fixed discovery client cannot be configured.
    pub fn new(
        current_version: impl Into<String>,
        target: Target,
        root: PathBuf,
        preferences: Arc<dyn UpdatePreferences>,
        event_sink: Option<Arc<dyn UpdateEventSink>>,
    ) -> Result<Arc<Self>, String> {
        if !root.is_absolute() || ptrack_updater::package_name(&target, "VERSION").is_err() {
            return Err("updates are unavailable".to_owned());
        }
        let client = Client::new().map_err(|_| "updates are unavailable".to_owned())?;
        Ok(Self::with_backend(
            current_version.into(),
            target,
            root,
            preferences,
            event_sink,
            Arc::new(ProductionUpdateBackend {
                client,
                installer: Installer::new(),
            }),
        ))
    }

    pub(crate) fn with_backend(
        current_version: String,
        target: Target,
        root: PathBuf,
        preferences: Arc<dyn UpdatePreferences>,
        event_sink: Option<Arc<dyn UpdateEventSink>>,
        backend: Arc<dyn UpdateBackend>,
    ) -> Arc<Self> {
        Arc::new(Self {
            core: Arc::new(UpdateCore {
                target,
                root,
                backend,
                preferences,
                preference_lock: Mutex::new(()),
                event_sink,
                state: Mutex::new(RuntimeState {
                    public: UpdateState::idle(current_version),
                    candidate: None,
                    stage: None,
                    active: false,
                    automatic_operation: false,
                    operation: 0,
                    cancellation: None,
                    recovering: false,
                    blocked: false,
                    shutting_down: false,
                }),
                idle: Condvar::new(),
            }),
        })
    }

    fn check(&self, automatic: bool) -> Result<UpdateState, String> {
        let preference_guard = automatic.then(|| lock(&self.core.preference_lock));
        {
            let state = lock(&self.core.state);
            if state.stage.is_some() {
                return Err("a verified update is already ready".to_owned());
            }
        }
        let (operation, cancellation, current) = self.begin(UpdatePhase::Checking, 0, automatic)?;
        drop(preference_guard);
        let backend = Arc::clone(&self.core.backend);
        let target = self.core.target.clone();
        let result =
            run_async(async move { backend.check(&cancellation, &current, &target).await });
        self.finish_check(operation, result)
    }

    fn begin(
        &self,
        phase: UpdatePhase,
        total: u64,
        automatic: bool,
    ) -> Result<(u64, CancellationToken, String), String> {
        let mut state = lock(&self.core.state);
        if state.shutting_down {
            return Err("update service is not running".to_owned());
        }
        if automatic && !state.public.automatic_checks {
            return Err("automatic update checks are disabled".to_owned());
        }
        if state.blocked {
            return Err("update recovery requires attention".to_owned());
        }
        if state.recovering {
            return Err("update recovery is still running".to_owned());
        }
        if state.active {
            return Err("another update operation is active".to_owned());
        }
        state.active = true;
        state.automatic_operation = automatic;
        state.operation = state.operation.saturating_add(1);
        let operation = state.operation;
        let cancellation = CancellationToken::new();
        state.cancellation = Some(cancellation.clone());
        state.public.phase = phase;
        state.public.error.clear();
        if phase == UpdatePhase::Downloading {
            state.public.downloaded_bytes = 0;
            state.public.total_bytes = total;
            state.public.checksum_verified = false;
        }
        state.public.revision = state.public.revision.saturating_add(1);
        let current = state.public.current_version.clone();
        let published = state.public.clone();
        drop(state);
        self.emit(published);
        Ok((operation, cancellation, current))
    }

    fn finish_check(
        &self,
        operation: u64,
        result: Result<Candidate, UpdateError>,
    ) -> Result<UpdateState, String> {
        let mut state = lock(&self.core.state);
        if state.operation != operation {
            return Err("update check was canceled".to_owned());
        }
        settle_operation(&self.core, &mut state);
        state.public.last_checked_at = now_rfc3339();
        let public_error = match result {
            Ok(candidate) => {
                state.public.phase = UpdatePhase::Available;
                state.public.release = Some(release_view(&candidate));
                state.public.error.clear();
                state.candidate = Some(candidate);
                None
            }
            Err(UpdateError::NoUpdate) => {
                state.public.phase = UpdatePhase::Current;
                state.public.release = None;
                state.candidate = None;
                None
            }
            Err(UpdateError::DevelopmentBuild | UpdateError::UnsupportedTarget) => {
                state.public.phase = UpdatePhase::Unavailable;
                state.public.release = None;
                "Updates are unavailable for this build.".clone_into(&mut state.public.error);
                state.candidate = None;
                None
            }
            Err(UpdateError::Cancelled) => {
                restore_phase(&mut state);
                Some("update operation was canceled".to_owned())
            }
            Err(_) => {
                state.public.phase = UpdatePhase::Error;
                state.public.release = None;
                "The GitHub Release could not be verified.".clone_into(&mut state.public.error);
                state.candidate = None;
                Some(state.public.error.clone())
            }
        };
        state.public.revision = state.public.revision.saturating_add(1);
        let published = state.public.clone();
        drop(state);
        self.emit(published.clone());
        public_error.map_or(Ok(published), Err)
    }

    fn emit(&self, state: UpdateState) {
        if let Some(sink) = &self.core.event_sink {
            sink.state_changed(state);
        }
    }

    fn recover_startup(&self) {
        let cancellation = CancellationToken::new();
        let recovering = {
            let mut state = lock(&self.core.state);
            state.recovering = true;
            state.public.phase = UpdatePhase::Recovering;
            state.public.revision = state.public.revision.saturating_add(1);
            state.public.clone()
        };
        self.emit(recovering);
        let result = self.scan_stages(&cancellation);
        let mut state = lock(&self.core.state);
        state.recovering = false;
        if let Err(message) = result {
            state.blocked = true;
            state.public.phase = UpdatePhase::RecoveryRequired;
            state.public.error = message;
        } else if state.stage.is_none() {
            state.public.phase = UpdatePhase::Idle;
        }
        state.public.revision = state.public.revision.saturating_add(1);
        let published = state.public.clone();
        let automatic = state.public.automatic_checks && !state.blocked && state.stage.is_none();
        drop(state);
        self.emit(published);
        if automatic {
            let runtime = self.clone();
            let _ = std::thread::Builder::new()
                .name("ptrack-update-check".to_owned())
                .spawn(move || {
                    let _ = runtime.check(true);
                });
        }
    }

    fn scan_stages(&self, cancellation: &CancellationToken) -> Result<(), String> {
        let entries = match std::fs::read_dir(&self.core.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Ok(()),
        };
        let mut roots = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .filter(|name| name.starts_with(".stage-"))
                    .map(|_| entry.path())
            })
            .collect::<Vec<_>>();
        roots.sort();
        if roots.len() > 64 {
            return Err("Too many saved updates require manual cleanup.".to_owned());
        }
        let mut valid = Vec::new();
        let mut best: Option<StagedUpdate> = None;
        let current = lock(&self.core.state).public.current_version.clone();
        for root in roots {
            if cancellation.is_cancelled() {
                return Ok(());
            }
            let Ok(stage) = self.core.backend.load(cancellation, &root) else {
                continue;
            };
            if stage.os != self.core.target.os || stage.arch != self.core.target.arch {
                continue;
            }
            match self.core.backend.recover(cancellation, &root) {
                Ok(_) | Err(UpdateError::PendingStageMismatch) => {}
                Err(_) => return Err("A previous update requires manual recovery.".to_owned()),
            }
            if compare_versions(&stage.version, &current).is_ok_and(std::cmp::Ordering::is_gt)
                && best.as_ref().is_none_or(|existing| {
                    compare_versions(&stage.version, &existing.version)
                        .is_ok_and(std::cmp::Ordering::is_gt)
                })
            {
                best = Some(stage.clone());
            }
            valid.push(stage);
        }
        if self.core.target.os == "linux" && pending_journal_exists(&self.core.root) {
            return Err("A previous update requires manual recovery.".to_owned());
        }
        let keep = best.as_ref().map(|stage| stage.root.as_path());
        let mut cleanup_pending = false;
        for stage in &valid {
            if keep != Some(stage.root.as_path()) && self.core.backend.discard(&stage.root).is_err()
            {
                cleanup_pending = true;
            }
        }
        let mut state = lock(&self.core.state);
        state.public.cleanup_pending |= cleanup_pending;
        if let Some(stage) = best {
            state.public.phase = UpdatePhase::Ready;
            state.public.release = Some(UpdateRelease {
                version: stage.version.clone(),
                published_at: String::new(),
                size_bytes: stage.size_bytes,
                notes: String::new(),
                page_url: String::new(),
            });
            state.public.downloaded_bytes = stage.size_bytes;
            state.public.total_bytes = stage.size_bytes;
            state.public.checksum_verified = true;
            state.stage = Some(stage);
        }
        Ok(())
    }
}

impl DesktopUpdateService for UpdateRuntime {
    fn start(&self) -> Result<(), String> {
        let _preference = lock(&self.core.preference_lock);
        let automatic = self
            .core
            .preferences
            .load_automatic_checks()
            .unwrap_or(false);
        lock(&self.core.state).public.automatic_checks = automatic;
        self.recover_startup();
        Ok(())
    }

    fn state(&self) -> UpdateState {
        lock(&self.core.state).public.clone()
    }

    fn set_automatic_checks(&self, enabled: bool) -> Result<UpdateState, String> {
        let _preference = lock(&self.core.preference_lock);
        self.core
            .preferences
            .save_automatic_checks(enabled)
            .map_err(|_| "could not save update preferences".to_owned())?;
        let mut state = lock(&self.core.state);
        if !enabled
            && state.automatic_operation
            && let Some(cancellation) = state.cancellation.take()
        {
            cancellation.cancel();
            state.public.phase = UpdatePhase::Canceling;
            state.public.downloaded_bytes = 0;
        }
        state.public.automatic_checks = enabled;
        state.public.revision = state.public.revision.saturating_add(1);
        let published = state.public.clone();
        drop(state);
        self.emit(published.clone());
        Ok(published)
    }

    fn check_for_updates(&self) -> Result<UpdateState, String> {
        self.check(false)
    }

    fn download_update(&self, expected_version: &str) -> Result<UpdateState, String> {
        let candidate = {
            let state = lock(&self.core.state);
            state
                .candidate
                .as_ref()
                .filter(|candidate| candidate.version == expected_version)
                .cloned()
                .ok_or_else(|| "the selected update is stale".to_owned())?
        };
        let (operation, cancellation, _) = self.begin(
            UpdatePhase::Downloading,
            candidate.package.size_bytes,
            false,
        )?;
        {
            let state = lock(&self.core.state);
            if state.candidate.as_ref() != Some(&candidate) {
                cancellation.cancel();
                return Err("the selected update is stale".to_owned());
            }
        }
        let core = Arc::clone(&self.core);
        let progress =
            Arc::new(move |progress: Progress| publish_progress(&core, operation, &progress));
        let backend = Arc::clone(&self.core.backend);
        let target = self.core.target.clone();
        let root = self.core.root.clone();
        let result = run_async(async move {
            backend
                .stage(&cancellation, &candidate, &target, &root, progress)
                .await
        });
        let mut runtime_state = lock(&self.core.state);
        if runtime_state.operation != operation {
            return Err("update download was canceled".to_owned());
        }
        settle_operation(&self.core, &mut runtime_state);
        let public_error = match result {
            Ok(staged) => {
                runtime_state.public.phase = UpdatePhase::Ready;
                runtime_state.public.downloaded_bytes = staged.size_bytes;
                runtime_state.public.total_bytes = staged.size_bytes;
                runtime_state.public.checksum_verified = true;
                runtime_state.public.error.clear();
                runtime_state.stage = Some(staged);
                None
            }
            Err(UpdateError::Cancelled) => {
                restore_phase(&mut runtime_state);
                Some("update operation was canceled".to_owned())
            }
            Err(_) => {
                runtime_state.public.phase = UpdatePhase::Error;
                "The update could not be downloaded safely."
                    .clone_into(&mut runtime_state.public.error);
                Some(runtime_state.public.error.clone())
            }
        };
        runtime_state.public.revision = runtime_state.public.revision.saturating_add(1);
        let published = runtime_state.public.clone();
        drop(runtime_state);
        self.emit(published.clone());
        public_error.map_or(Ok(published), Err)
    }

    fn apply_update(&self, expected_version: &str) -> Result<UpdateState, String> {
        let stage = {
            let state = lock(&self.core.state);
            state
                .stage
                .as_ref()
                .filter(|stage| stage.version == expected_version)
                .cloned()
                .ok_or_else(|| "the verified update is stale".to_owned())?
        };
        let (operation, cancellation, _) =
            self.begin(UpdatePhase::Applying, stage.size_bytes, false)?;
        let backend = Arc::clone(&self.core.backend);
        let result = run_async(async move { backend.apply(&cancellation, &stage).await });
        let mut runtime_state = lock(&self.core.state);
        if runtime_state.operation != operation {
            return Err("update installation was canceled".to_owned());
        }
        settle_operation(&self.core, &mut runtime_state);
        let public_error = match result {
            Ok(result) => {
                runtime_state.public.phase = if result.manual_install {
                    UpdatePhase::ActionRequired
                } else {
                    UpdatePhase::Installed
                };
                runtime_state.public.apply_action = serde_json::to_value(result.action)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default();
                runtime_state.public.restart_required = result.restart_required;
                runtime_state.public.manual_install = result.manual_install;
                runtime_state.public.cleanup_pending = result.cleanup_pending;
                runtime_state.public.error.clear();
                None
            }
            Err(UpdateError::Cancelled) => {
                restore_phase(&mut runtime_state);
                Some("update operation was canceled".to_owned())
            }
            Err(_) => {
                runtime_state.public.phase = UpdatePhase::Error;
                "The verified update could not be installed safely."
                    .clone_into(&mut runtime_state.public.error);
                Some(runtime_state.public.error.clone())
            }
        };
        runtime_state.public.revision = runtime_state.public.revision.saturating_add(1);
        let published = runtime_state.public.clone();
        drop(runtime_state);
        self.emit(published.clone());
        public_error.map_or(Ok(published), Err)
    }

    fn cancel_operation(&self) -> UpdateState {
        let mut state = lock(&self.core.state);
        if let Some(cancellation) = state.cancellation.take() {
            cancellation.cancel();
            state.public.phase = UpdatePhase::Canceling;
            state.public.downloaded_bytes = 0;
            state.public.error.clear();
            state.public.revision = state.public.revision.saturating_add(1);
        }
        let published = state.public.clone();
        drop(state);
        self.emit(published.clone());
        published
    }

    fn shutdown(&self) -> Result<(), String> {
        let mut state = lock(&self.core.state);
        state.shutting_down = true;
        if let Some(cancellation) = state.cancellation.take() {
            cancellation.cancel();
        }
        let deadline = Instant::now() + UPDATE_SHUTDOWN_TIMEOUT;
        while state.active {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("update operation did not stop before shutdown".to_owned());
            }
            let (next, result) = self
                .core
                .idle
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
            if result.timed_out() && state.active {
                return Err("update operation did not stop before shutdown".to_owned());
            }
        }
        Ok(())
    }
}

fn settle_operation(core: &UpdateCore, state: &mut RuntimeState) {
    state.active = false;
    state.automatic_operation = false;
    state.cancellation = None;
    core.idle.notify_all();
}

fn restore_phase(state: &mut RuntimeState) {
    state.public.phase = if state.stage.is_some() {
        UpdatePhase::Ready
    } else if state.candidate.is_some() {
        UpdatePhase::Available
    } else {
        UpdatePhase::Idle
    };
    state.public.error.clear();
}

fn publish_progress(core: &UpdateCore, operation: u64, progress: &Progress) {
    if progress.asset != "package" {
        return;
    }
    let mut state = lock(&core.state);
    if state.operation != operation || state.public.phase != UpdatePhase::Downloading {
        return;
    }
    if progress.downloaded != progress.total
        && progress
            .downloaded
            .saturating_sub(state.public.downloaded_bytes)
            < UPDATE_PROGRESS_QUANTUM
    {
        return;
    }
    state.public.downloaded_bytes = progress.downloaded;
    state.public.total_bytes = progress.total;
    state.public.revision = state.public.revision.saturating_add(1);
    let published = state.public.clone();
    drop(state);
    if let Some(sink) = &core.event_sink {
        sink.state_changed(published);
    }
}

fn release_view(candidate: &Candidate) -> UpdateRelease {
    UpdateRelease {
        version: candidate.version.clone(),
        published_at: candidate.published_at.clone(),
        size_bytes: candidate.package.size_bytes,
        notes: candidate.notes.clone(),
        page_url: candidate.page_url.clone(),
    }
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

fn pending_journal_exists(base: &Path) -> bool {
    std::fs::read_dir(base).map_or(true, |entries| {
        entries.filter_map(Result::ok).any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            entry.file_type().is_ok_and(|kind| !kind.is_dir())
                && name.starts_with(".pending-apply-")
                && name.ends_with(".json")
        })
    })
}

fn run_async<F, T>(future: F) -> Result<T, UpdateError>
where
    F: Future<Output = Result<T, UpdateError>> + Send + 'static,
    T: Send + 'static,
{
    std::thread::Builder::new()
        .name("ptrack-update-operation".to_owned())
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| UpdateError::Message("start update runtime".to_owned()))?
                .block_on(future)
        })
        .map_err(|_| UpdateError::Message("start update operation".to_owned()))?
        .join()
        .map_err(|_| UpdateError::Message("update operation failed".to_owned()))?
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[must_use]
pub const fn update_preference_key() -> &'static [u8] {
    UPDATE_PREFERENCE_KEY
}
