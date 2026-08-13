use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use ptrack_agent::Registration;
use ptrack_capability::Broker;
use ptrack_terminal::{
    CwdPolicy, ExitResult, Manager, ManagerErrorKind, Profile, ProfileKind, Session, SessionInfo,
    SessionState, ShellIntegrationDescriptor, resolve_cwd, sort_profiles,
};
use serde::Serialize;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{AppError, AppResult, LaunchedEventAuthority, LinkedAgentRuntimeHooks};

const DEFAULT_ATTACH_LEASE: Duration = Duration::from_secs(30);
const MAX_CWD_VALIDATIONS: usize = 96;
const MAX_CWD_BYTES: usize = 4_096;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalProfileView {
    pub id: String,
    pub name: String,
    pub kind: ProfileKind,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub provider: String,
    pub theme: String,
    pub font_family: String,
    pub font_size: u16,
    pub scrollback: u32,
    pub cwd_policy: CwdPolicy,
    pub exit_behavior: ptrack_terminal::ExitBehavior,
}

impl From<Profile> for TerminalProfileView {
    fn from(profile: Profile) -> Self {
        Self {
            id: profile.id,
            name: profile.name,
            kind: profile.kind,
            provider: profile.provider,
            theme: profile.theme,
            font_family: profile.font_family,
            font_size: profile.font_size,
            scrollback: profile.scrollback,
            cwd_policy: profile.cwd_policy,
            exit_behavior: profile.exit_behavior,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalProfilesV2 {
    pub generation: u64,
    pub profiles: Vec<TerminalProfileView>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSessionV2 {
    pub generation: u64,
    pub session_id: String,
    pub profile_id: String,
    pub cwd: String,
    pub state: SessionState,
    pub stream_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub association_revision: Option<u64>,
    #[serde(skip_serializing_if = "is_false")]
    pub linked_launch: bool,
    pub shell_integration: ShellIntegrationDescriptor,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalStatusV2 {
    pub generation: u64,
    pub session_id: String,
    pub state: SessionState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalExitV2 {
    pub generation: u64,
    pub session_id: String,
    pub exit_code: i32,
    pub state: SessionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCwdValidation {
    pub requested: String,
    pub cwd: String,
    pub valid: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCwdValidationsV2 {
    pub generation: u64,
    pub results: Vec<TerminalCwdValidation>,
}

pub trait TerminalEventSink: Send + Sync {
    fn status(&self, event: TerminalStatusV2);
    fn exited(&self, event: TerminalExitV2);
    fn runtime_changed(&self, generation: u64);
}

pub trait TerminalAgentAuthority:
    LaunchedEventAuthority + LinkedAgentRuntimeHooks + Send + Sync
{
}

impl<T> TerminalAgentAuthority for T where
    T: LaunchedEventAuthority + LinkedAgentRuntimeHooks + Send + Sync
{
}

pub struct PreparedTerminalIdentity {
    environment: BTreeMap<String, String>,
    capability_token: String,
    event_token: String,
    agent: bool,
}

impl PreparedTerminalIdentity {
    pub(crate) fn empty(agent: bool) -> Self {
        Self {
            environment: BTreeMap::new(),
            capability_token: String::new(),
            event_token: String::new(),
            agent,
        }
    }
}

#[allow(clippy::missing_errors_doc)]
pub trait TerminalIdentityAuthority: Send + Sync {
    fn prepare(
        &self,
        generation: u64,
        project_root: &Path,
        profile: &Profile,
    ) -> AppResult<PreparedTerminalIdentity>;
    fn bind(
        &self,
        generation: u64,
        identity: &PreparedTerminalIdentity,
        session: &SessionInfo,
    ) -> AppResult<()>;
    fn revoke_pending(&self, generation: u64, identity: &PreparedTerminalIdentity);
    fn revoke_session(&self, generation: u64, session_id: &str);
    fn record_exit(&self, generation: u64, session_id: &str, result: &ExitResult);
}

pub struct ProductionTerminalIdentityAuthority {
    broker: Option<Arc<Broker>>,
    agents: Option<Arc<dyn TerminalAgentAuthority>>,
}

impl ProductionTerminalIdentityAuthority {
    #[must_use]
    pub fn new(
        broker: Option<Arc<Broker>>,
        agents: Option<Arc<dyn TerminalAgentAuthority>>,
    ) -> Self {
        Self { broker, agents }
    }
}

impl TerminalIdentityAuthority for ProductionTerminalIdentityAuthority {
    fn prepare(
        &self,
        generation: u64,
        project_root: &Path,
        profile: &Profile,
    ) -> AppResult<PreparedTerminalIdentity> {
        let agent = profile.kind == ProfileKind::Agent;
        let mut identity = PreparedTerminalIdentity::empty(agent);
        if !agent {
            return Ok(identity);
        }
        if let Some(agents) = &self.agents {
            let endpoint = agents.event_endpoint(generation)?;
            let token = agents.issue_launched_event_token(generation)?;
            identity
                .environment
                .insert("PTRACK_AGENT_EVENT_ENDPOINT_V1".to_owned(), endpoint);
            identity
                .environment
                .insert("PTRACK_AGENT_EVENT_TOKEN_V1".to_owned(), token.clone());
            identity.event_token = token;
        }
        if let Some(broker) = &self.broker {
            let token = match broker.issue_session_token(&profile.id) {
                Ok(token) => token,
                Err(error) => {
                    self.revoke_pending(generation, &identity);
                    return Err(AppError::Message(error.to_string()));
                }
            };
            identity.environment.insert(
                "PTRACK_CAPABILITY_PROJECT".to_owned(),
                project_root.to_string_lossy().into_owned(),
            );
            identity.environment.insert(
                "PTRACK_CAPABILITY_GENERATION".to_owned(),
                generation.to_string(),
            );
            identity
                .environment
                .insert("PTRACK_CAPABILITY_PROFILE".to_owned(), profile.id.clone());
            identity
                .environment
                .insert("PTRACK_CAPABILITY_TOKEN".to_owned(), token.clone());
            identity.capability_token = token;
        }
        Ok(identity)
    }

    fn bind(
        &self,
        generation: u64,
        identity: &PreparedTerminalIdentity,
        session: &SessionInfo,
    ) -> AppResult<()> {
        if !identity.agent {
            return Ok(());
        }
        if let Some(broker) = &self.broker
            && !identity.capability_token.is_empty()
        {
            broker
                .bind_session(&identity.capability_token, &session.id)
                .map_err(|error| AppError::Message(error.to_string()))?;
        }
        let Some(agents) = &self.agents else {
            return Ok(());
        };
        let pid = i32::try_from(session.pid)
            .map_err(|_| AppError::Message("terminal process identity is invalid".to_owned()))?;
        let run = match agents.register_launched(
            generation,
            Registration {
                profile: session.profile_id.clone(),
                provider: session.provider.clone(),
                pid,
                terminal_id: session.id.clone(),
                cwd: session.cwd.clone(),
            },
        ) {
            Ok(run) => run,
            Err(error) => {
                self.revoke_session(generation, &session.id);
                return Err(error);
            }
        };
        if !identity.event_token.is_empty()
            && let Err(error) =
                agents.bind_launched_event_token(generation, &identity.event_token, &run.id)
        {
            let _ = agents.rollback_launched(generation, &run.id, &session.id);
            self.revoke_session(generation, &session.id);
            return Err(error);
        }
        Ok(())
    }

    fn revoke_pending(&self, generation: u64, identity: &PreparedTerminalIdentity) {
        if let Some(broker) = &self.broker
            && !identity.capability_token.is_empty()
        {
            broker.revoke_token(&identity.capability_token);
        }
        if let Some(agents) = &self.agents
            && !identity.event_token.is_empty()
        {
            let _ = agents.revoke_launched_event_token(generation, &identity.event_token);
        }
    }

    fn revoke_session(&self, generation: u64, session_id: &str) {
        if let Some(broker) = &self.broker {
            broker.revoke_session(session_id);
        }
        if let Some(agents) = &self.agents {
            let _ = agents.revoke_terminal_event_tokens(generation, session_id);
        }
    }

    fn record_exit(&self, generation: u64, session_id: &str, result: &ExitResult) {
        if let Some(agents) = &self.agents {
            let class = if result.error.is_some() {
                "failed"
            } else {
                "exited"
            };
            let _ = agents.record_terminal_exit(generation, session_id, result.exit_code, class);
        }
    }
}

pub struct TerminalRuntimeConfig {
    pub generation: u64,
    pub project_root: PathBuf,
    pub manager: Arc<Manager>,
    pub identity: Arc<dyn TerminalIdentityAuthority>,
    pub events: Arc<dyn TerminalEventSink>,
    pub attachment_lease: Duration,
}

struct RuntimeGateState {
    closing: bool,
    operations: usize,
}

struct RuntimeGate {
    state: Mutex<RuntimeGateState>,
    idle: Condvar,
}

struct RuntimeOperation(Arc<RuntimeGate>);

impl Drop for RuntimeOperation {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.operations = state.operations.saturating_sub(1);
        if state.operations == 0 {
            self.0.idle.notify_all();
        }
    }
}

pub struct TerminalRuntime {
    generation: u64,
    project_root: PathBuf,
    manager: Arc<Manager>,
    identity: Arc<dyn TerminalIdentityAuthority>,
    events: Arc<dyn TerminalEventSink>,
    attachment_lease: Duration,
    gate: Arc<RuntimeGate>,
    cancellation: CancellationToken,
    monitors: Mutex<Vec<JoinHandle<()>>>,
}

impl TerminalRuntime {
    /// Creates one UI-neutral, generation-scoped terminal host.
    ///
    /// # Errors
    /// Returns an error for a zero generation or mismatched project root.
    pub fn new(config: TerminalRuntimeConfig) -> AppResult<Arc<Self>> {
        if config.generation == 0 {
            return Err(AppError::Message(
                "terminal generation is required".to_owned(),
            ));
        }
        let project_root = resolve_cwd(&config.project_root, None)
            .map_err(|error| AppError::Message(error.to_string()))?;
        if project_root != config.manager.project_root() {
            return Err(AppError::Message(
                "terminal manager project root does not match the workspace".to_owned(),
            ));
        }
        Ok(Arc::new(Self {
            generation: config.generation,
            project_root,
            manager: config.manager,
            identity: config.identity,
            events: config.events,
            attachment_lease: if config.attachment_lease.is_zero() {
                DEFAULT_ATTACH_LEASE
            } else {
                config.attachment_lease
            },
            gate: Arc::new(RuntimeGate {
                state: Mutex::new(RuntimeGateState {
                    closing: false,
                    operations: 0,
                }),
                idle: Condvar::new(),
            }),
            cancellation: CancellationToken::new(),
            monitors: Mutex::new(Vec::new()),
        }))
    }

    fn begin(&self, expected_generation: u64) -> AppResult<RuntimeOperation> {
        if expected_generation != 0 && expected_generation != self.generation {
            return Err(AppError::Message("stale workspace generation".to_owned()));
        }
        let mut state = lock(&self.gate.state);
        if state.closing {
            return Err(AppError::Message(
                "terminal lifecycle is shutting down".to_owned(),
            ));
        }
        state.operations += 1;
        Ok(RuntimeOperation(Arc::clone(&self.gate)))
    }

    /// Returns sorted presentation-only profiles without launch authority.
    ///
    /// # Errors
    /// Returns lifecycle or generation errors.
    pub fn profiles(&self, generation: u64) -> AppResult<TerminalProfilesV2> {
        let _operation = self.begin(generation)?;
        let mut profiles = self.manager.profiles();
        sort_profiles(&mut profiles);
        Ok(TerminalProfilesV2 {
            generation: self.generation,
            profiles: profiles.into_iter().map(Into::into).collect(),
        })
    }

    /// Validates a bounded, duplicate-free set of candidate working directories.
    ///
    /// # Errors
    /// Returns lifecycle, generation, count, size, or duplicate-input errors.
    pub fn validate_cwds(
        &self,
        generation: u64,
        requested: &[String],
    ) -> AppResult<TerminalCwdValidationsV2> {
        let _operation = self.begin(generation)?;
        if requested.len() > MAX_CWD_VALIDATIONS {
            return Err(AppError::Message(
                "too many terminal working directories".to_owned(),
            ));
        }
        let mut seen = BTreeSet::new();
        let mut results = Vec::with_capacity(requested.len());
        for value in requested {
            if value.len() > MAX_CWD_BYTES {
                return Err(AppError::Message(
                    "terminal working directory is too long".to_owned(),
                ));
            }
            if !seen.insert(value) {
                return Err(AppError::Message(
                    "terminal working directories must be unique".to_owned(),
                ));
            }
            if value.is_empty() {
                results.push(TerminalCwdValidation {
                    requested: value.clone(),
                    cwd: String::new(),
                    valid: true,
                });
                continue;
            }
            let resolved = resolve_cwd(&self.project_root, Some(Path::new(value)));
            let (cwd, valid) = match resolved {
                Ok(cwd) if cwd.as_os_str().len() <= MAX_CWD_BYTES => {
                    (cwd.to_string_lossy().into_owned(), true)
                }
                _ => (String::new(), false),
            };
            results.push(TerminalCwdValidation {
                requested: value.clone(),
                cwd,
                valid,
            });
        }
        Ok(TerminalCwdValidationsV2 {
            generation: self.generation,
            results,
        })
    }

    /// Creates one session and binds all agent authority before exposing it.
    ///
    /// # Errors
    /// Fails closed on generation, profile, identity, PTY, bind, or registration errors.
    pub fn create(
        self: &Arc<Self>,
        generation: u64,
        profile_id: &str,
        cwd: Option<&Path>,
        rows: u16,
        columns: u16,
    ) -> AppResult<TerminalSessionV2> {
        let _operation = self.begin(generation)?;
        let profile = self
            .manager
            .profiles()
            .into_iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| {
                AppError::Message(format!("terminal profile {profile_id:?} is unavailable"))
            })?;
        let identity = self
            .identity
            .prepare(self.generation, &self.project_root, &profile)?;
        let session = match self.manager.create_with_env(
            profile_id,
            cwd,
            rows,
            columns,
            &identity.environment,
        ) {
            Ok(session) => session,
            Err(error) => {
                self.identity.revoke_pending(self.generation, &identity);
                return Err(AppError::Message(error.to_string()));
            }
        };
        let info = session.info();
        if identity.agent && info.profile_kind != ProfileKind::Agent {
            self.identity.revoke_pending(self.generation, &identity);
            let _ = self.manager.close_session(session.id(), true);
            return Err(AppError::Message(
                "terminal manager returned a shell for an agent profile".to_owned(),
            ));
        }
        if let Err(error) = self.identity.bind(self.generation, &identity, &info) {
            self.identity.revoke_pending(self.generation, &identity);
            self.identity.revoke_session(self.generation, session.id());
            let _ = self.manager.close_session(session.id(), true);
            return Err(error);
        }
        let stream_url = match self.manager.session_url(session.id()) {
            Ok(url) => url,
            Err(error) => {
                self.identity.revoke_session(self.generation, session.id());
                let _ = self.manager.close_session(session.id(), true);
                return Err(AppError::Message(error.to_string()));
            }
        };
        let result = TerminalSessionV2 {
            generation: self.generation,
            session_id: session.id().to_owned(),
            profile_id: info.profile_id,
            cwd: info.cwd,
            state: info.state,
            stream_url,
            association_revision: None,
            linked_launch: false,
            shell_integration: session.shell_integration().clone(),
        };
        self.events.status(TerminalStatusV2 {
            generation: self.generation,
            session_id: result.session_id.clone(),
            state: result.state,
        });
        self.monitor_exit(&session);
        self.monitor_attachment(&session);
        Ok(result)
    }

    /// Resizes one generation-fenced live terminal.
    ///
    /// # Errors
    /// Returns lifecycle, generation, session, state, or PTY resize errors.
    pub fn resize(
        &self,
        generation: u64,
        session_id: &str,
        rows: u16,
        columns: u16,
    ) -> AppResult<()> {
        let _operation = self.begin(generation)?;
        self.manager
            .resize_session(session_id, rows, columns)
            .map_err(|error| AppError::Message(error.to_string()))
    }

    /// Revokes authority before closing a live session. Unknown IDs are idempotent.
    ///
    /// # Errors
    /// Returns lifecycle, generation, or terminal teardown errors.
    pub fn close(&self, generation: u64, session_id: &str, force: bool) -> AppResult<()> {
        let _operation = self.begin(generation)?;
        self.identity.revoke_session(self.generation, session_id);
        if let Err(error) = self.manager.close_session(session_id, force)
            && error.kind() != ManagerErrorKind::SessionNotFound
        {
            return Err(AppError::Message(error.to_string()));
        }
        self.events.status(TerminalStatusV2 {
            generation: self.generation,
            session_id: session_id.to_owned(),
            state: SessionState::Closed,
        });
        Ok(())
    }

    fn monitor_exit(self: &Arc<Self>, session: &Arc<Session>) {
        let Some(results) = session.take_exit_results() else {
            return;
        };
        let weak = Arc::downgrade(self);
        let cancellation = self.cancellation.clone();
        let session_id = session.id().to_owned();
        self.push_monitor(tokio::spawn(async move {
            let received = tokio::task::spawn_blocking(move || results.recv()).await;
            if cancellation.is_cancelled() {
                return;
            }
            let Ok(Ok(result)) = received else {
                return;
            };
            if let Some(runtime) = weak.upgrade() {
                runtime
                    .identity
                    .revoke_session(runtime.generation, &session_id);
                runtime
                    .identity
                    .record_exit(runtime.generation, &session_id, &result);
                runtime.events.exited(TerminalExitV2 {
                    generation: runtime.generation,
                    session_id,
                    exit_code: result.exit_code,
                    state: result.state,
                    error: result.error,
                });
                runtime.events.runtime_changed(runtime.generation);
            }
        }));
    }

    fn monitor_attachment(self: &Arc<Self>, session: &Arc<Session>) {
        let weak = Arc::downgrade(self);
        let session = Arc::clone(session);
        let cancellation = self.cancellation.clone();
        let lease = self.attachment_lease;
        self.push_monitor(tokio::spawn(async move {
            tokio::select! {
                () = cancellation.cancelled() => return,
                () = tokio::time::sleep(lease) => {}
            }
            if !session.attachment_expiry_wins() {
                return;
            }
            if let Some(runtime) = weak.upgrade() {
                runtime
                    .identity
                    .revoke_session(runtime.generation, session.id());
                let _ = runtime.manager.close_session(session.id(), true);
                runtime.events.status(TerminalStatusV2 {
                    generation: runtime.generation,
                    session_id: session.id().to_owned(),
                    state: SessionState::Closed,
                });
            }
        }));
    }

    fn push_monitor(&self, monitor: JoinHandle<()>) {
        lock(&self.monitors).push(monitor);
    }

    /// Stops admission, revokes session authority, shuts the listener first,
    /// then joins every owned monitor. Idempotent at the manager boundary.
    ///
    /// # Errors
    /// Returns terminal manager shutdown failures.
    pub async fn shutdown(&self) -> AppResult<()> {
        {
            let mut state = lock(&self.gate.state);
            state.closing = true;
            while state.operations != 0 {
                state = self
                    .gate
                    .idle
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
        self.cancellation.cancel();
        for session_id in self.manager.lifecycle_session_ids() {
            self.identity.revoke_session(self.generation, &session_id);
        }
        self.manager
            .shutdown()
            .await
            .map_err(|error| AppError::Message(error.to_string()))?;
        let monitors = std::mem::take(&mut *lock(&self.monitors));
        for monitor in monitors {
            let _ = monitor.await;
        }
        Ok(())
    }
}

impl Drop for TerminalRuntime {
    fn drop(&mut self) {
        lock(&self.gate.state).closing = true;
        self.cancellation.cancel();
        self.manager.request_shutdown();
        let session_ids = self.manager.lifecycle_session_ids();
        for session_id in &session_ids {
            self.identity.revoke_session(self.generation, session_id);
        }
        for session_id in session_ids {
            let _ = self.manager.close_session(&session_id, true);
        }
        for monitor in lock(&self.monitors).drain(..) {
            monitor.abort();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}
