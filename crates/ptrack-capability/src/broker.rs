use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ptrack_capability_policy::{AuditEvent, Denied};
use ptrack_core::Capability;
use ptrack_store::{ActiveBinding, ProjectStore};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::audit::{AuditError, AuditRecorder, AuditSink};
use crate::{GitExecutor, GitRequest, HttpExecutor, HttpRequest, SshExecutor, SshRequest};

pub const TOOL_HTTP_REQUEST: &str = "ptrack_http_request";
pub const TOOL_GIT: &str = "ptrack_git";
pub const TOOL_SSH: &str = "ptrack_ssh";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub annotations: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// Returns the exact provider-facing capability tool surface.
#[must_use]
pub fn tool_definitions() -> Vec<ToolDefinition> {
    let id = json!({"type": "integer", "minimum": 1});
    let text = json!({"type": "string"});
    let annotations = json!({"destructiveHint": true, "openWorldHint": true});
    vec![
        ToolDefinition {
            name: TOOL_HTTP_REQUEST.to_owned(),
            title: "p-track HTTP capability".to_owned(),
            description: "Make an explicitly approved bounded HTTP request".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "capability_id": id.clone(),
                    "request": {
                        "type": "object",
                        "properties": {
                            "method": text.clone(),
                            "url": text.clone(),
                            "headers": {"type": "object", "additionalProperties": {"type": "array", "items": text.clone()}},
                            "body": {"type": "string", "contentEncoding": "base64"}
                        },
                        "additionalProperties": false,
                        "required": ["method", "url"]
                    }
                },
                "additionalProperties": false,
                "required": ["capability_id", "request"]
            }),
            annotations: annotations.clone(),
        },
        ToolDefinition {
            name: TOOL_GIT.to_owned(),
            title: "p-track Git capability".to_owned(),
            description: "Run an explicitly approved fixed Git operation".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "capability_id": id.clone(),
                    "ssh_capability_id": id.clone(),
                    "request": {
                        "type": "object",
                        "properties": {
                            "operation": text.clone(), "branch": text.clone(), "refspec": text.clone(),
                            "force": {"type": "boolean"}
                        },
                        "additionalProperties": false,
                        "required": ["operation"]
                    }
                },
                "additionalProperties": false,
                "required": ["capability_id", "request"]
            }),
            annotations: annotations.clone(),
        },
        ToolDefinition {
            name: TOOL_SSH.to_owned(),
            title: "p-track SSH capability".to_owned(),
            description: "Run an explicitly approved fixed SSH operation".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "capability_id": id,
                    "request": {
                        "type": "object",
                        "properties": {
                            "operation": text.clone(), "command": text.clone(), "local_path": text.clone(),
                            "remote_path": text.clone(), "forward_target": text,
                            "listen_port": {"type": "integer", "minimum": 1, "maximum": 65535}
                        },
                        "additionalProperties": false,
                        "required": ["operation"]
                    }
                },
                "additionalProperties": false,
                "required": ["capability_id", "request"]
            }),
            annotations,
        },
    ]
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionIdentity {
    pub profile: String,
    pub project_root: PathBuf,
    pub generation: u64,
    pub session_id: String,
}

#[derive(Clone)]
pub struct BrokerConfig {
    pub project_root: PathBuf,
    pub database: PathBuf,
    pub binding: ActiveBinding,
    pub writer_version: String,
    pub generation: u64,
}

struct SessionGrant {
    hash: [u8; 32],
    identity: SessionIdentity,
    cancellation: CancellationToken,
}

#[derive(Default)]
struct BrokerState {
    sessions: Vec<SessionGrant>,
    active: HashMap<u64, HashMap<u64, CancellationToken>>,
    next_active: u64,
    closed: bool,
}

/// Generation-scoped, host-minted capability authority.
pub struct Broker {
    config: BrokerConfig,
    cancellation: CancellationToken,
    state: Mutex<BrokerState>,
}

impl Broker {
    /// Creates one broker bound to an exact canonical project and store binding.
    ///
    /// # Errors
    /// Fails when the project cannot be canonicalized or the binding differs.
    pub fn new(mut config: BrokerConfig) -> Result<Self, BrokerError> {
        config.project_root = config
            .project_root
            .canonicalize()
            .map_err(|_| BrokerError::internal("capability project root is unavailable"))?;
        if config.generation == 0 {
            return Err(BrokerError::internal(
                "capability broker generation is required",
            ));
        }
        let store =
            ProjectStore::open_existing(&config.database, &config.binding, &config.writer_version)
                .map_err(|_| BrokerError::internal("capability store is unavailable"))?;
        drop(store);
        Ok(Self {
            config,
            cancellation: CancellationToken::new(),
            state: Mutex::new(BrokerState::default()),
        })
    }

    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.config.project_root
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.config.generation
    }

    /// Mints a 32-byte bearer token whose hash is the only retained token material.
    ///
    /// # Errors
    /// Fails for an invalid host profile, unavailable entropy, or closed broker.
    pub fn issue_session_token(&self, profile: &str) -> Result<String, BrokerError> {
        let profile = normalize_profile(profile)?;
        let mut raw = [0_u8; 32];
        getrandom::fill(&mut raw)
            .map_err(|_| BrokerError::internal("capability token could not be created"))?;
        let token = URL_SAFE_NO_PAD.encode(raw);
        let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let mut state = lock(&self.state);
        if state.closed {
            return Err(BrokerError::internal("capability broker is closed"));
        }
        state.sessions.push(SessionGrant {
            hash,
            identity: SessionIdentity {
                profile,
                project_root: self.config.project_root.clone(),
                generation: self.config.generation,
                session_id: String::new(),
            },
            cancellation: self.cancellation.child_token(),
        });
        Ok(token)
    }

    /// Binds a token to exactly one non-empty terminal session identity.
    ///
    /// # Errors
    /// Fails closed for malformed, unknown, stale, or already-bound tokens.
    pub fn bind_session(&self, token: &str, session_id: &str) -> Result<(), BrokerError> {
        let hash = token_hash(token).ok_or_else(bind_error)?;
        let mut state = lock(&self.state);
        let Some(index) = constant_time_session_index(&state.sessions, &hash) else {
            return Err(bind_error());
        };
        let grant = &mut state.sessions[index];
        if session_id.is_empty() || !grant.identity.session_id.is_empty() {
            return Err(bind_error());
        }
        session_id.clone_into(&mut grant.identity.session_id);
        Ok(())
    }

    pub fn revoke_token(&self, token: &str) {
        let Some(hash) = token_hash(token) else {
            return;
        };
        let mut state = lock(&self.state);
        if let Some(index) = constant_time_session_index(&state.sessions, &hash) {
            state.sessions[index].cancellation.cancel();
            state.sessions.swap_remove(index);
        }
    }

    pub fn revoke_session(&self, session_id: &str) {
        let mut state = lock(&self.state);
        state.sessions.retain(|grant| {
            if grant.identity.session_id == session_id {
                grant.cancellation.cancel();
                false
            } else {
                true
            }
        });
    }

    pub fn revoke_capability(&self, capability_id: u64) {
        let mut state = lock(&self.state);
        if let Some(active) = state.active.remove(&capability_id) {
            for cancellation in active.into_values() {
                cancellation.cancel();
            }
        }
    }

    /// Invalidates every session and in-flight call. Idempotent.
    pub fn shutdown(&self) {
        self.cancellation.cancel();
        let mut state = lock(&self.state);
        state.closed = true;
        for grant in state.sessions.drain(..) {
            grant.cancellation.cancel();
        }
        for operations in state.active.drain().map(|(_, operations)| operations) {
            for cancellation in operations.into_values() {
                cancellation.cancel();
            }
        }
    }

    /// Authenticates and dispatches one fixed provider tool.
    ///
    /// # Errors
    /// Returns stable, secret-free policy, transport, or storage errors.
    pub async fn call(
        &self,
        caller_cancellation: &CancellationToken,
        token: &str,
        call: ToolCall,
    ) -> Result<Value, BrokerError> {
        self.call_with_reload_barrier(caller_cancellation, token, call, &|| {})
            .await
    }

    pub(crate) async fn call_with_reload_barrier(
        &self,
        caller_cancellation: &CancellationToken,
        token: &str,
        call: ToolCall,
        reload_barrier: &(dyn Fn() + Send + Sync),
    ) -> Result<Value, BrokerError> {
        let (identity, session_cancellation) = self.authenticate(token)?;
        if call.name == TOOL_SSH
            && call
                .arguments
                .get("request")
                .and_then(|request| request.get("operation"))
                .and_then(Value::as_str)
                == Some("interactive-shell")
        {
            return Err(BrokerError::external(
                "capability denied: interactive SSH is unavailable over the MCP transport",
            ));
        }
        let call_cancellation = session_cancellation.child_token();
        let _watcher = CallerWatch(watch_caller(
            caller_cancellation.clone(),
            call_cancellation.clone(),
        ));
        match call.name.as_str() {
            TOOL_HTTP_REQUEST => {
                self.call_http(
                    call.arguments,
                    &identity,
                    &call_cancellation,
                    reload_barrier,
                )
                .await
            }
            TOOL_GIT => {
                self.call_git(
                    call.arguments,
                    &identity,
                    &call_cancellation,
                    reload_barrier,
                )
                .await
            }
            TOOL_SSH => {
                self.call_ssh(
                    call.arguments,
                    &identity,
                    &call_cancellation,
                    reload_barrier,
                )
                .await
            }
            name => Err(BrokerError::external(format!(
                "capability denied: unknown capability tool {name:?}"
            ))),
        }
    }

    async fn call_http(
        &self,
        arguments: Value,
        identity: &SessionIdentity,
        cancellation: &CancellationToken,
        reload_barrier: &(dyn Fn() + Send + Sync),
    ) -> Result<Value, BrokerError> {
        let arguments: HttpArguments = decode_arguments(arguments)?;
        let first = self.load_capabilities(&[arguments.capability_id])?;
        let guard = self.track(
            &[(arguments.capability_id, first[0].limits.max_concurrent)],
            cancellation,
        )?;
        reload_barrier();
        let second = self.load_capabilities(&[arguments.capability_id])?;
        let sink = BrokerAuditSink {
            config: &self.config,
        };
        let value = HttpExecutor::from_recorder(AuditRecorder::from_sink(&sink))
            .execute(
                cancellation,
                &second[0],
                &identity.profile,
                &arguments.request,
            )
            .await
            .map_err(BrokerError::external)?;
        drop(guard);
        encode_response(value)
    }

    async fn call_git(
        &self,
        arguments: Value,
        identity: &SessionIdentity,
        cancellation: &CancellationToken,
        reload_barrier: &(dyn Fn() + Send + Sync),
    ) -> Result<Value, BrokerError> {
        let arguments: GitArguments = decode_arguments(arguments)?;
        let mut ids = vec![arguments.capability_id];
        if arguments.ssh_capability_id != 0 {
            ids.push(arguments.ssh_capability_id);
        }
        let first = self.load_capabilities(&ids)?;
        let slots: Vec<_> = ids
            .iter()
            .zip(first.iter())
            .map(|(id, capability)| (*id, capability.limits.max_concurrent))
            .collect();
        let guard = self.track(&slots, cancellation)?;
        reload_barrier();
        let second = self.load_capabilities(&ids)?;
        let sink = BrokerAuditSink {
            config: &self.config,
        };
        let value =
            GitExecutor::from_parts(AuditRecorder::from_sink(&sink), super::git::system_runner())
                .execute(
                    cancellation,
                    &second[0],
                    second.get(1),
                    &identity.profile,
                    &identity.project_root,
                    &arguments.request,
                )
                .await
                .map_err(BrokerError::external)?;
        drop(guard);
        encode_response(value)
    }

    async fn call_ssh(
        &self,
        arguments: Value,
        identity: &SessionIdentity,
        cancellation: &CancellationToken,
        reload_barrier: &(dyn Fn() + Send + Sync),
    ) -> Result<Value, BrokerError> {
        let arguments: SshArguments = decode_arguments(arguments)?;
        let first = self.load_capabilities(&[arguments.capability_id])?;
        let guard = self.track(
            &[(arguments.capability_id, first[0].limits.max_concurrent)],
            cancellation,
        )?;
        reload_barrier();
        let second = self.load_capabilities(&[arguments.capability_id])?;
        let sink = BrokerAuditSink {
            config: &self.config,
        };
        let value =
            SshExecutor::from_parts(AuditRecorder::from_sink(&sink), super::git::system_runner())
                .execute(
                    cancellation,
                    &second[0],
                    &identity.profile,
                    &identity.project_root,
                    &arguments.request,
                )
                .await
                .map_err(BrokerError::external)?;
        drop(guard);
        encode_response(value)
    }

    fn authenticate(
        &self,
        token: &str,
    ) -> Result<(SessionIdentity, CancellationToken), BrokerError> {
        if token.is_empty() {
            return Err(BrokerError::external(
                "capability denied: capability session token is required",
            ));
        }
        let hash = token_hash(token).ok_or_else(stale_token_error)?;
        let state = lock(&self.state);
        let Some(index) = constant_time_session_index(&state.sessions, &hash) else {
            return Err(stale_token_error());
        };
        let grant = &state.sessions[index];
        if grant.identity.session_id.is_empty() || grant.cancellation.is_cancelled() {
            return Err(stale_token_error());
        }
        Ok((grant.identity.clone(), grant.cancellation.clone()))
    }

    pub(crate) fn authenticate_token(&self, token: &str) -> Result<SessionIdentity, BrokerError> {
        self.authenticate(token).map(|(identity, _)| identity)
    }

    pub(crate) fn load_capabilities(&self, ids: &[u64]) -> Result<Vec<Capability>, BrokerError> {
        let store = ProjectStore::open_existing(
            &self.config.database,
            &self.config.binding,
            &self.config.writer_version,
        )
        .map_err(|_| BrokerError::internal("capability store is unavailable"))?;
        let result = ids
            .iter()
            .map(|id| {
                store
                    .capability(*id)
                    .map_err(|_| BrokerError::internal("capability is unavailable"))
            })
            .collect();
        drop(store);
        result
    }

    pub(crate) fn track<'a>(
        &'a self,
        slots: &[(u64, i64)],
        cancellation: &CancellationToken,
    ) -> Result<ActiveGuard<'a>, BrokerError> {
        let mut state = lock(&self.state);
        let mut requested = HashMap::<u64, (usize, usize)>::new();
        for (id, maximum) in slots {
            let maximum = usize::try_from(*maximum).unwrap_or(1).max(1);
            let entry = requested.entry(*id).or_insert((0, maximum));
            entry.0 = entry.0.saturating_add(1);
            entry.1 = entry.1.min(maximum);
        }
        for (id, (additional, maximum)) in &requested {
            if state.active.get(id).map_or(*additional, |active| {
                active.len().saturating_add(*additional)
            }) > *maximum
            {
                return Err(BrokerError::external(
                    "capability denied: capability concurrency limit reached",
                ));
            }
        }
        let mut entries = Vec::with_capacity(slots.len());
        for (id, _) in slots {
            state.next_active = state.next_active.wrapping_add(1).max(1);
            let operation = state.next_active;
            state
                .active
                .entry(*id)
                .or_default()
                .insert(operation, cancellation.clone());
            entries.push((*id, operation));
        }
        Ok(ActiveGuard {
            broker: self,
            entries,
        })
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) struct ActiveGuard<'a> {
    broker: &'a Broker,
    entries: Vec<(u64, u64)>,
}

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.broker.state);
        for (capability, operation) in &self.entries {
            if let Some(active) = state.active.get_mut(capability) {
                active.remove(operation);
                if active.is_empty() {
                    state.active.remove(capability);
                }
            }
        }
    }
}

struct BrokerAuditSink<'a> {
    config: &'a BrokerConfig,
}

impl AuditSink for BrokerAuditSink<'_> {
    fn record(&self, capability: &Capability, event: &AuditEvent) -> Result<(), AuditError> {
        let store = ProjectStore::open_existing(
            &self.config.database,
            &self.config.binding,
            &self.config.writer_version,
        )
        .map_err(|_| AuditError)?;
        AuditRecorder::new(Some(&store)).record(capability, event)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpArguments {
    capability_id: u64,
    request: HttpRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitArguments {
    capability_id: u64,
    #[serde(default)]
    ssh_capability_id: u64,
    request: GitRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SshArguments {
    capability_id: u64,
    request: SshRequest,
}

fn decode_arguments<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, BrokerError> {
    if value.is_null() {
        return Err(BrokerError::external("tool arguments are required"));
    }
    serde_json::from_value(value)
        .map_err(|error| BrokerError::external(format!("invalid tool arguments: {error}")))
}

fn encode_response(value: impl Serialize) -> Result<Value, BrokerError> {
    serde_json::to_value(value)
        .map_err(|_| BrokerError::internal("capability response could not be encoded"))
}

fn token_hash(token: &str) -> Option<[u8; 32]> {
    let raw = URL_SAFE_NO_PAD.decode(token).ok()?;
    let raw: [u8; 32] = raw.try_into().ok()?;
    if URL_SAFE_NO_PAD.encode(raw) != token {
        return None;
    }
    Some(Sha256::digest(token.as_bytes()).into())
}

fn constant_time_session_index(sessions: &[SessionGrant], hash: &[u8; 32]) -> Option<usize> {
    let mut selected = None;
    for (index, grant) in sessions.iter().enumerate() {
        if bool::from(grant.hash.ct_eq(hash)) {
            selected = Some(index);
        }
    }
    selected
}

fn normalize_profile(profile: &str) -> Result<String, BrokerError> {
    static PROFILE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let value = profile.trim();
    if value.is_empty()
        || value.len() > 64
        || value.starts_with('-')
        || !PROFILE
            .get_or_init(|| Regex::new(r"^[\p{L}\p{Nd}._-]+$").expect("static profile regex"))
            .is_match(value)
    {
        return Err(BrokerError::external(format!(
            "invalid agent profile {profile:?}"
        )));
    }
    Ok(value.to_owned())
}

fn watch_caller(caller: CancellationToken, call: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        caller.cancelled().await;
        call.cancel();
    })
}

struct CallerWatch(JoinHandle<()>);

impl Drop for CallerWatch {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn bind_error() -> BrokerError {
    BrokerError::external("capability session token cannot be bound")
}

fn stale_token_error() -> BrokerError {
    BrokerError::external("capability denied: capability session token is invalid or stale")
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrokerError(String);

impl BrokerError {
    fn external(message: impl fmt::Display) -> Self {
        Self(message.to_string())
    }

    fn internal(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for BrokerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for BrokerError {}

impl From<Denied> for BrokerError {
    fn from(error: Denied) -> Self {
        Self(error.to_string())
    }
}
