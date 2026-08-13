use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use ptrack_agent::{Association, AssociationPointer, Registration};
use ptrack_capability::Broker;
use ptrack_terminal::{
    CwdPolicy, ExitResult, MAX_RUNTIME_SESSION_CANDIDATES, Manager, ManagerErrorKind, Profile,
    ProfileKind, Session, SessionInfo, SessionState, ShellIntegrationDescriptor,
    TerminalAssociation, TerminalAssociationChange, TerminalAssociationPointer, resolve_cwd,
    sort_profiles,
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
    pub executable: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub theme: String,
    pub font_family: String,
    pub font_size: u16,
    pub scrollback: u32,
    pub cwd_policy: CwdPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_cwd: Option<String>,
    pub exit_behavior: ptrack_terminal::ExitBehavior,
}

impl From<Profile> for TerminalProfileView {
    fn from(profile: Profile) -> Self {
        Self {
            id: profile.id,
            name: profile.name,
            kind: profile.kind,
            provider: profile.provider,
            executable: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            theme: profile.theme,
            font_family: profile.font_family,
            font_size: profile.font_size,
            scrollback: profile.scrollback,
            cwd_policy: profile.cwd_policy,
            fixed_cwd: None,
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

    pub(crate) fn insert_environment(&mut self, key: &str, value: String) {
        self.environment.insert(key.to_owned(), value);
    }

    pub(crate) fn capability_token(&self) -> &str {
        &self.capability_token
    }

    pub(crate) fn event_token(&self) -> &str {
        &self.event_token
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
    fn bind_linked(
        &self,
        _generation: u64,
        _identity: &PreparedTerminalIdentity,
        _session: &SessionInfo,
        _pointer: AssociationPointer,
    ) -> AppResult<Association> {
        Err(AppError::Message(
            "linked agent identity authority is unavailable".to_owned(),
        ))
    }
    fn revoke_pending(&self, generation: u64, identity: &PreparedTerminalIdentity);
    fn revoke_session(&self, generation: u64, session_id: &str);
    fn revoke_failed_session(&self, generation: u64, session_id: &str) {
        self.revoke_session(generation, session_id);
    }
    fn rollback_linked_session(&self, generation: u64, session_id: &str) {
        self.revoke_session(generation, session_id);
        self.remove_linked_session(generation, session_id);
    }
    fn remove_linked_session(&self, _generation: u64, _session_id: &str) {}
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
        let _event_suppression = self
            .agents
            .as_ref()
            .map(|agents| agents.suppress_runtime_event(generation))
            .transpose()?;
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
        let _event_suppression = agents.suppress_runtime_event(generation)?;
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

    fn bind_linked(
        &self,
        generation: u64,
        identity: &PreparedTerminalIdentity,
        session: &SessionInfo,
        pointer: AssociationPointer,
    ) -> AppResult<Association> {
        if !identity.agent {
            return Err(AppError::Message(
                "linked launch requires an agent profile".to_owned(),
            ));
        }
        let agents = self
            .agents
            .as_ref()
            .ok_or_else(|| AppError::Message("AgentRun registry is unavailable".to_owned()))?;
        let _event_suppression = agents.suppress_runtime_event(generation)?;
        let pid = i32::try_from(session.pid)
            .map_err(|_| AppError::Message("terminal process identity is invalid".to_owned()))?;
        let run = agents.register_linked_launched(
            generation,
            Registration {
                profile: session.profile_id.clone(),
                provider: session.provider.clone(),
                pid,
                terminal_id: session.id.clone(),
                cwd: session.cwd.clone(),
            },
            pointer,
        )?;
        let association = run.association.clone().ok_or_else(|| {
            AppError::Message("linked terminal and AgentRun associations differ".to_owned())
        })?;
        if !identity.event_token.is_empty()
            && let Err(error) =
                agents.bind_launched_event_token(generation, &identity.event_token, &run.id)
        {
            return Err(error);
        }
        if let Some(broker) = &self.broker
            && !identity.capability_token.is_empty()
            && let Err(error) = broker.bind_session(&identity.capability_token, &session.id)
        {
            return Err(AppError::Message(error.to_string()));
        }
        Ok(association)
    }

    fn revoke_pending(&self, generation: u64, identity: &PreparedTerminalIdentity) {
        let _event_suppression = self
            .agents
            .as_ref()
            .and_then(|agents| agents.suppress_runtime_event(generation).ok());
        revoke_prepared_tokens(
            identity.event_token(),
            identity.capability_token(),
            |token| {
                if let Some(agents) = &self.agents {
                    let _ = agents.revoke_launched_event_token(generation, token);
                }
            },
            |token| {
                if let Some(broker) = &self.broker {
                    broker.revoke_token(token);
                }
            },
        );
    }

    fn revoke_session(&self, generation: u64, session_id: &str) {
        let _event_suppression = self
            .agents
            .as_ref()
            .and_then(|agents| agents.suppress_runtime_event(generation).ok());
        if let Some(broker) = &self.broker {
            broker.revoke_session(session_id);
        }
        if let Some(agents) = &self.agents {
            let _ = agents.revoke_terminal_event_tokens(generation, session_id);
        }
    }

    fn revoke_failed_session(&self, generation: u64, session_id: &str) {
        let _event_suppression = self
            .agents
            .as_ref()
            .and_then(|agents| agents.suppress_runtime_event(generation).ok());
        if let Some(agents) = &self.agents {
            let _ = agents.revoke_terminal_event_tokens(generation, session_id);
        }
        if let Some(broker) = &self.broker {
            broker.revoke_session(session_id);
        }
    }

    fn rollback_linked_session(&self, generation: u64, session_id: &str) {
        self.revoke_session(generation, session_id);
        self.remove_linked_session(generation, session_id);
    }

    fn remove_linked_session(&self, generation: u64, session_id: &str) {
        let _event_suppression = self
            .agents
            .as_ref()
            .and_then(|agents| agents.suppress_runtime_event(generation).ok());
        if let Some(agents) = &self.agents {
            let _ = agents.rollback_linked_terminal(generation, session_id);
        }
    }

    fn record_exit(&self, generation: u64, session_id: &str, result: &ExitResult) {
        if let Some(agents) = &self.agents {
            let Ok(_event_suppression) = agents.suppress_runtime_event(generation) else {
                return;
            };
            let class = if result.error.is_some() {
                "failed"
            } else {
                "exited"
            };
            let _ = agents.record_terminal_exit(generation, session_id, result.exit_code, class);
        }
    }
}

pub(super) fn revoke_prepared_tokens(
    event_token: &str,
    capability_token: &str,
    mut revoke_event: impl FnMut(&str),
    mut revoke_capability: impl FnMut(&str),
) {
    if !event_token.is_empty() {
        revoke_event(event_token);
    }
    if !capability_token.is_empty() {
        revoke_capability(capability_token);
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

fn join_terminal_cleanup(
    primary: AppError,
    cleanup: Result<(), ptrack_terminal::ManagerError>,
) -> AppError {
    match cleanup {
        Ok(()) => primary,
        Err(cleanup) => AppError::Message(format!("{primary}\n{cleanup}")),
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
    resource_revision: Arc<AtomicU64>,
}

#[allow(clippy::missing_errors_doc)]
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
            resource_revision: Arc::new(AtomicU64::new(0)),
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

    /// Returns the exact number of currently published terminal sessions for
    /// workspace lifecycle confirmation. The count carries no session tokens.
    pub fn active_session_count(&self, generation: u64) -> AppResult<usize> {
        let _operation = self.begin(generation)?;
        let (sessions, total) = self
            .manager
            .runtime_session_snapshot_bounded(MAX_RUNTIME_SESSION_CANDIDATES);
        if total > sessions.len() {
            return Err(AppError::Message(
                "terminal session snapshot exceeds exact limit".to_owned(),
            ));
        }
        Ok(sessions
            .iter()
            .filter(|session| {
                matches!(
                    session.state,
                    SessionState::Starting | SessionState::Running
                )
            })
            .count())
    }

    /// Counts live sessions associated with an exact task, failing closed when
    /// the bounded runtime inventory cannot be represented.
    pub fn task_session_count(
        &self,
        generation: u64,
        plan_id: u64,
        task_id: u64,
    ) -> AppResult<usize> {
        let _operation = self.begin(generation)?;
        let sessions = self
            .manager
            .session_snapshot_exact(MAX_RUNTIME_SESSION_CANDIDATES)
            .map_err(|error| AppError::Message(error.to_string()))?;
        Ok(sessions
            .iter()
            .filter(|session| {
                session.association.as_ref().is_some_and(|association| {
                    association.pointer.plan_id == plan_id && association.pointer.task_id == task_id
                })
            })
            .count())
    }

    /// Executes a callback while holding the exact terminal lifecycle epoch.
    pub fn with_exact_session_snapshot<T>(
        &self,
        generation: u64,
        use_snapshot: impl FnOnce(&[SessionInfo]) -> AppResult<T>,
    ) -> AppResult<T> {
        let _operation = self.begin(generation)?;
        self.manager
            .with_exact_session_snapshot(MAX_RUNTIME_SESSION_CANDIDATES, use_snapshot)
            .map_err(|error| AppError::Message(error.to_string()))?
    }

    /// Returns the bounded presentation candidate set and exact total.
    pub fn runtime_session_snapshot(
        &self,
        generation: u64,
    ) -> AppResult<(Vec<SessionInfo>, usize)> {
        let _operation = self.begin(generation)?;
        Ok(self
            .manager
            .runtime_session_snapshot_bounded(MAX_RUNTIME_SESSION_CANDIDATES))
    }

    /// Monotonic terminal lifecycle/association epoch used by confirmation flows.
    pub fn resource_revision(&self, generation: u64) -> AppResult<u64> {
        let _operation = self.begin(generation)?;
        Ok(self.resource_revision.load(Ordering::Acquire))
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
        self.create_inner(generation, profile_id, cwd, rows, columns, None)
    }

    /// Creates an agent session whose terminal and `AgentRun` associations are
    /// both published at revision one before the session is exposed.
    #[allow(clippy::too_many_arguments)] // Exact desktop launch contract fields remain explicit.
    pub fn create_linked(
        self: &Arc<Self>,
        generation: u64,
        profile_id: &str,
        cwd: Option<&Path>,
        rows: u16,
        columns: u16,
        pointer: TerminalAssociationPointer,
        launch_context: &str,
    ) -> AppResult<TerminalSessionV2> {
        self.create_inner(
            generation,
            profile_id,
            cwd,
            rows,
            columns,
            Some((pointer, launch_context)),
        )
    }

    #[allow(clippy::too_many_lines)]
    fn create_inner(
        self: &Arc<Self>,
        generation: u64,
        profile_id: &str,
        cwd: Option<&Path>,
        rows: u16,
        columns: u16,
        linked: Option<(TerminalAssociationPointer, &str)>,
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
        if linked.is_some() && profile.kind != ProfileKind::Agent {
            return Err(AppError::Message(format!(
                "terminal profile {profile_id:?} is not an agent"
            )));
        }
        let mut identity = self
            .identity
            .prepare(self.generation, &self.project_root, &profile)?;
        if let Some((_, launch_context)) = linked {
            identity.insert_environment("PTRACK_LAUNCH_CONTEXT_V1", launch_context.to_owned());
        }
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
        if linked.is_some()
            && (info.id.is_empty()
                || info.profile_id != profile.id
                || info.profile_kind != ProfileKind::Agent
                || info.provider != profile.provider
                || info.pid == 0)
        {
            self.identity.revoke_pending(self.generation, &identity);
            return Err(join_terminal_cleanup(
                AppError::Message(
                    "launched terminal identity does not match the selected agent profile"
                        .to_owned(),
                ),
                self.manager.close_session(session.id(), true),
            ));
        }
        if linked.is_some() {
            let expected_cwd = fs::canonicalize(cwd.unwrap_or(&self.project_root)).ok();
            let actual_cwd = fs::canonicalize(&info.cwd).ok();
            if expected_cwd.is_none() || actual_cwd != expected_cwd {
                self.identity.revoke_pending(self.generation, &identity);
                return Err(join_terminal_cleanup(
                    AppError::Message(
                        "launched terminal working directory does not match validated CWD"
                            .to_owned(),
                    ),
                    self.manager.close_session(session.id(), true),
                ));
            }
        }
        if identity.agent && info.profile_kind != ProfileKind::Agent {
            self.identity.revoke_pending(self.generation, &identity);
            return Err(join_terminal_cleanup(
                AppError::Message(
                    "terminal manager returned a shell for an agent profile".to_owned(),
                ),
                self.manager.close_session(session.id(), true),
            ));
        }
        let mut association_revision = None;
        let bind_result = if let Some((pointer, _)) = linked {
            let terminal_association = self
                .manager
                .associate_session(session.id(), pointer)
                .map_err(|error| AppError::Message(error.to_string()));
            match terminal_association {
                Ok(terminal_association) => {
                    let agent_pointer = AssociationPointer {
                        version: pointer.version,
                        plan_id: pointer.plan_id,
                        task_id: pointer.task_id,
                    };
                    self.identity
                        .bind_linked(self.generation, &identity, &info, agent_pointer)
                        .and_then(|agent_association| {
                            if agent_association.generation != self.generation
                                || agent_association.target.plan_id != pointer.plan_id
                                || agent_association.target.task_id != pointer.task_id
                                || agent_association.revision != terminal_association.revision
                            {
                                return Err(AppError::Message(
                                    "linked terminal and AgentRun associations differ".to_owned(),
                                ));
                            }
                            association_revision = Some(terminal_association.revision);
                            Ok(())
                        })
                }
                Err(error) => Err(error),
            }
        } else {
            self.identity.bind(self.generation, &identity, &info)
        };
        if let Err(error) = bind_result {
            if linked.is_some() {
                // Prepared tokens are the only stable identities until both binds
                // succeed. Revoke them event-first, then clear any session-bound
                // remnants before removing the paired runtime record.
                self.identity.revoke_pending(self.generation, &identity);
                self.identity
                    .revoke_failed_session(self.generation, session.id());
                self.identity
                    .remove_linked_session(self.generation, session.id());
            } else {
                self.identity.revoke_pending(self.generation, &identity);
                self.identity.revoke_session(self.generation, session.id());
            }
            return Err(join_terminal_cleanup(
                error,
                self.manager.close_session(session.id(), true),
            ));
        }
        let stream_url = match self.manager.session_url(session.id()) {
            Ok(url) => url,
            Err(error) => {
                if linked.is_some() {
                    self.identity.revoke_pending(self.generation, &identity);
                    self.identity
                        .revoke_failed_session(self.generation, session.id());
                    self.identity
                        .remove_linked_session(self.generation, session.id());
                } else {
                    self.identity.revoke_session(self.generation, session.id());
                }
                return Err(join_terminal_cleanup(
                    AppError::Message(error.to_string()),
                    self.manager.close_session(session.id(), true),
                ));
            }
        };
        let result = TerminalSessionV2 {
            generation: self.generation,
            session_id: session.id().to_owned(),
            profile_id: info.profile_id,
            cwd: info.cwd,
            state: info.state,
            stream_url,
            association_revision,
            linked_launch: linked.is_some(),
            shell_integration: session.shell_integration().clone(),
        };
        self.events.status(TerminalStatusV2 {
            generation: self.generation,
            session_id: result.session_id.clone(),
            state: result.state,
        });
        self.monitor_exit(&session);
        self.monitor_attachment(&session);
        increment_revision(&self.resource_revision);
        self.events.runtime_changed(self.generation);
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
        increment_revision(&self.resource_revision);
        self.events.runtime_changed(self.generation);
        Ok(())
    }

    /// Removes linked launch authority and force-closes its terminal. Callers
    /// must first prove the session is a linked launch through the `AgentRun` owner.
    pub fn rollback_linked(&self, generation: u64, session_id: &str) -> AppResult<()> {
        let _operation = self.begin(generation)?;
        self.identity.revoke_session(self.generation, session_id);
        if let Err(error) = self.manager.close_session(session_id, true)
            && error.kind() != ManagerErrorKind::SessionNotFound
        {
            return Err(AppError::Message(error.to_string()));
        }
        self.identity
            .remove_linked_session(self.generation, session_id);
        self.events.status(TerminalStatusV2 {
            generation: self.generation,
            session_id: session_id.to_owned(),
            state: SessionState::Closed,
        });
        increment_revision(&self.resource_revision);
        self.events.runtime_changed(self.generation);
        Ok(())
    }

    /// Cleans a linked launch that failed after publication. Unlike a user
    /// rollback, failure cleanup revokes event authority before capability
    /// authority and force-closes before reporting any teardown error.
    pub fn rollback_failed_linked(&self, generation: u64, session_id: &str) -> AppResult<()> {
        let _operation = self.begin(generation)?;
        self.identity
            .revoke_failed_session(self.generation, session_id);
        let close = self.manager.close_session(session_id, true);
        self.identity
            .remove_linked_session(self.generation, session_id);
        self.events.status(TerminalStatusV2 {
            generation: self.generation,
            session_id: session_id.to_owned(),
            state: SessionState::Closed,
        });
        increment_revision(&self.resource_revision);
        self.events.runtime_changed(self.generation);
        match close {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ManagerErrorKind::SessionNotFound => Ok(()),
            Err(error) => Err(AppError::Message(error.to_string())),
        }
    }

    /// Applies one application-validated association pointer under an exact
    /// live-session revision fence. A pointer with no plan is the established
    /// detached representation.
    pub fn mutate_association(
        &self,
        generation: u64,
        session_id: &str,
        expected_revision: u64,
        pointer: TerminalAssociationPointer,
    ) -> AppResult<TerminalAssociation> {
        let _operation = self.begin(generation)?;
        let change = self
            .manager
            .prepare_association_change(session_id, pointer, expected_revision)
            .map_err(|error| AppError::Message(error.to_string()))?;
        self.manager
            .commit_association_change(&change)
            .map_err(|error| AppError::Message(error.to_string()))?;
        increment_revision(&self.resource_revision);
        self.events.runtime_changed(self.generation);
        Ok(change.next)
    }

    pub fn prepare_association_change(
        &self,
        generation: u64,
        session_id: &str,
        expected_revision: u64,
        pointer: TerminalAssociationPointer,
    ) -> AppResult<TerminalAssociationChange> {
        let _operation = self.begin(generation)?;
        self.manager
            .prepare_association_change(session_id, pointer, expected_revision)
            .map_err(|error| AppError::Message(error.to_string()))
    }

    pub fn commit_association_change(
        &self,
        generation: u64,
        change: &TerminalAssociationChange,
    ) -> AppResult<()> {
        let _operation = self.begin(generation)?;
        self.manager
            .commit_association_change(change)
            .map_err(|error| AppError::Message(error.to_string()))
    }

    pub fn rollback_association_change(
        &self,
        generation: u64,
        change: &TerminalAssociationChange,
    ) -> AppResult<()> {
        let _operation = self.begin(generation)?;
        self.manager
            .rollback_association_change(change)
            .map_err(|error| AppError::Message(error.to_string()))
    }

    pub fn association_changed(&self, generation: u64) -> AppResult<()> {
        let _operation = self.begin(generation)?;
        increment_revision(&self.resource_revision);
        self.events.runtime_changed(self.generation);
        Ok(())
    }

    /// Associates a previously detached live session for the first time.
    pub fn associate(
        &self,
        generation: u64,
        session_id: &str,
        pointer: TerminalAssociationPointer,
    ) -> AppResult<TerminalAssociation> {
        let _operation = self.begin(generation)?;
        let association = self
            .manager
            .associate_session(session_id, pointer)
            .map_err(|error| AppError::Message(error.to_string()))?;
        increment_revision(&self.resource_revision);
        self.events.runtime_changed(self.generation);
        Ok(association)
    }

    /// Reads a live association only while holding its exact revision fence.
    pub fn live_association(
        &self,
        generation: u64,
        session_id: &str,
        expected_revision: u64,
    ) -> AppResult<TerminalAssociation> {
        let _operation = self.begin(generation)?;
        self.manager
            .get(session_id)
            .map_err(|error| AppError::Message(error.to_string()))?
            .with_live_association(expected_revision, Clone::clone)
            .map_err(|error| AppError::Message(error.to_string()))
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
                increment_revision(&runtime.resource_revision);
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
                increment_revision(&runtime.resource_revision);
                runtime.events.runtime_changed(runtime.generation);
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

fn increment_revision(revision: &AtomicU64) {
    let _ = revision.fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
        Some(value.saturating_add(1))
    });
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}
