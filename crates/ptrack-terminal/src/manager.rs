use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Serialize;
use tokio::sync::Notify;

use crate::profile::{
    CwdPolicy, Profile, ProfileKind, build_environment, resolve_cwd, safe_environment_entry,
    sort_profiles, validate_profile,
};
use crate::pty::{NativePtyFactory, PtyFactory, StartRequest};
use crate::session::{
    Session, SessionError, SessionInfo, SessionMetadata, TerminalAssociation,
    TerminalAssociationChange, TerminalAssociationPointer,
};
use crate::shell_integration::{ShellIntegrationOwner, prepare_shell_integration};
use crate::stream::{StreamServer, StreamSession, StreamSessionHost};

pub const MAX_SESSION_SNAPSHOT: usize = 64;
pub const MAX_RUNTIME_SESSION_CANDIDATES: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagerErrorKind {
    Shutdown,
    ProfileNotFound,
    SessionNotFound,
    SnapshotLimit,
    InvalidConfiguration,
    Launch,
    Stream,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagerError {
    kind: ManagerErrorKind,
    message: String,
}

impl ManagerError {
    fn new(kind: ManagerErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> ManagerErrorKind {
        self.kind
    }
}

impl fmt::Display for ManagerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ManagerError {}

/// One freshly minted single-use stream ticket and the replay position the
/// renderer will actually resume from.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamTicket {
    pub url: String,
    /// Sequence the replay starts at, clamped to the retained window.
    pub from_sequence: u64,
    /// True when output older than `from_sequence` was dropped and is lost.
    pub gap: bool,
}

impl From<SessionError> for ManagerError {
    fn from(error: SessionError) -> Self {
        Self::new(ManagerErrorKind::Launch, error.to_string())
    }
}

struct ManagerInner {
    sessions: HashMap<String, Arc<Session>>,
    closing: HashMap<String, Arc<Session>>,
    admission_closed: bool,
    shutdown_running: bool,
    shutdown_done: bool,
    shutdown_error: Option<ManagerError>,
    creates: usize,
}

pub struct Manager {
    project_root: PathBuf,
    profiles: BTreeMap<String, Profile>,
    factory: Arc<dyn PtyFactory>,
    inner: Mutex<ManagerInner>,
    stream_server: Mutex<Option<Arc<StreamServer>>>,
    shells: Mutex<Option<ShellIntegrationOwner>>,
    changed: Notify,
}

impl fmt::Debug for Manager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Manager")
            .field("project_root", &self.project_root)
            .field("profiles", &self.profiles.len())
            .finish_non_exhaustive()
    }
}

impl Manager {
    /// Construct a manager and bind its single IPv4 loopback stream listener.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid root/profile set or listener failure.
    pub async fn new(
        project_root: impl AsRef<Path>,
        profiles: Vec<Profile>,
        factory: Arc<dyn PtyFactory>,
    ) -> Result<Arc<Self>, ManagerError> {
        let canonical_root = resolve_cwd(project_root.as_ref(), None).map_err(|error| {
            ManagerError::new(
                ManagerErrorKind::InvalidConfiguration,
                format!("resolve project root: {error}"),
            )
        })?;
        let mut profile_map = BTreeMap::new();
        for source in profiles {
            let profile = validate_profile(&source).map_err(|error| {
                ManagerError::new(
                    ManagerErrorKind::InvalidConfiguration,
                    format!("validate terminal profile {:?}: {error}", source.id),
                )
            })?;
            if profile_map
                .insert(profile.id.clone(), profile.clone())
                .is_some()
            {
                return Err(ManagerError::new(
                    ManagerErrorKind::InvalidConfiguration,
                    format!("duplicate terminal profile ID {:?}", profile.id),
                ));
            }
        }
        if profile_map.is_empty() {
            return Err(ManagerError::new(
                ManagerErrorKind::InvalidConfiguration,
                "at least one terminal profile is required",
            ));
        }
        let shells = ShellIntegrationOwner::new(profile_map.values()).unwrap_or(None);
        let manager = Arc::new(Self {
            project_root: canonical_root,
            profiles: profile_map,
            factory,
            inner: Mutex::new(ManagerInner {
                sessions: HashMap::new(),
                closing: HashMap::new(),
                admission_closed: false,
                shutdown_running: false,
                shutdown_done: false,
                shutdown_error: None,
                creates: 0,
            }),
            stream_server: Mutex::new(None),
            shells: Mutex::new(shells),
            changed: Notify::new(),
        });
        let host: Arc<dyn StreamSessionHost> = manager.clone();
        let server = StreamServer::bind(Arc::downgrade(&host))
            .await
            .map_err(|error| {
                ManagerError::new(
                    ManagerErrorKind::Stream,
                    format!("bind terminal stream server: {error}"),
                )
            })?;
        *manager
            .stream_server
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(server);
        Ok(manager)
    }

    /// Construct a production manager with the native PTY factory.
    ///
    /// # Errors
    ///
    /// Returns the same validation/listener errors as [`Self::new`].
    pub async fn native(
        project_root: impl AsRef<Path>,
        profiles: Vec<Profile>,
    ) -> Result<Arc<Self>, ManagerError> {
        Self::new(project_root, profiles, Arc::new(NativePtyFactory)).await
    }

    #[must_use]
    pub fn profiles(&self) -> Vec<Profile> {
        let mut profiles: Vec<_> = self.profiles.values().cloned().collect();
        sort_profiles(&mut profiles);
        profiles
    }

    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Look up one live session.
    ///
    /// # Errors
    ///
    /// Returns the frozen session-not-found error for unknown/closing IDs.
    pub fn get(&self, session_id: &str) -> Result<Arc<Session>, ManagerError> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| {
                ManagerError::new(
                    ManagerErrorKind::SessionNotFound,
                    format!("terminal session not found: {session_id}"),
                )
            })
    }

    /// Create a terminal using profile and caller-owned working-directory data.
    ///
    /// # Errors
    ///
    /// Returns an error for shutdown, unknown profile, invalid CWD, or PTY start.
    pub fn create(
        self: &Arc<Self>,
        profile_id: &str,
        requested_cwd: Option<&Path>,
        rows: u16,
        columns: u16,
    ) -> Result<Arc<Session>, ManagerError> {
        self.create_with_env(profile_id, requested_cwd, rows, columns, &BTreeMap::new())
    }

    /// Create a terminal with validated host-owned environment overrides.
    ///
    /// # Errors
    ///
    /// Returns shutdown, profile, CWD, environment, randomness, or PTY errors.
    pub fn create_with_env(
        self: &Arc<Self>,
        profile_id: &str,
        requested_cwd: Option<&Path>,
        rows: u16,
        columns: u16,
        extra_environment: &BTreeMap<String, String>,
    ) -> Result<Arc<Session>, ManagerError> {
        let profile = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if inner.admission_closed {
                return Err(ManagerError::new(
                    ManagerErrorKind::Shutdown,
                    "terminal manager is shut down",
                ));
            }
            let profile = self.profiles.get(profile_id).cloned().ok_or_else(|| {
                ManagerError::new(
                    ManagerErrorKind::ProfileNotFound,
                    format!("terminal profile not found: {profile_id}"),
                )
            })?;
            inner.creates += 1;
            profile
        };
        let _create = CreateGuard { manager: self };

        let selected_cwd = match profile.cwd_policy {
            CwdPolicy::Requested => requested_cwd,
            CwdPolicy::Project => None,
            CwdPolicy::Fixed => Some(Path::new(&profile.fixed_cwd)),
        };
        let cwd = resolve_cwd(&self.project_root, selected_cwd)
            .map_err(|error| ManagerError::new(ManagerErrorKind::Launch, error.to_string()))?;
        let mut overrides = profile.env.clone();
        for (key, value) in extra_environment {
            if !safe_environment_entry(key, value) {
                return Err(ManagerError::new(
                    ManagerErrorKind::Launch,
                    format!("unsafe per-launch environment override {key:?}"),
                ));
            }
            overrides.insert(key.clone(), value.clone());
        }
        let inherited: Vec<String> = env::vars_os()
            .filter_map(|(key, value)| {
                Some(format!(
                    "{}={}",
                    key.into_string().ok()?,
                    value.into_string().ok()?
                ))
            })
            .collect();
        let environment = build_environment(&inherited, &overrides)
            .map_err(|error| ManagerError::new(ManagerErrorKind::Launch, error.to_string()))?;
        let id = random_opaque_value("create terminal session ID")?;
        let shell_nonce = random_opaque_value("create shell integration nonce")?;
        let (args, environment, shell_integration) = prepare_shell_integration(
            self.shells
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref(),
            &profile,
            &environment,
            &shell_nonce,
        );
        let session = Session::new(
            StartRequest {
                executable: profile.executable.clone(),
                args,
                env: environment,
                cwd: cwd.clone(),
                rows,
                columns,
            },
            SessionMetadata {
                id: id.clone(),
                profile_id: profile.id,
                profile_kind: profile.kind,
                provider: profile.provider,
                cwd: cwd.to_string_lossy().into_owned(),
                shell_integration,
            },
            Arc::clone(&self.factory),
        );
        session.start().map_err(|error| {
            let _ = session.close(true);
            ManagerError::new(ManagerErrorKind::Launch, error.to_string())
        })?;
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.admission_closed {
            drop(inner);
            let _ = session.close(true);
            return Err(ManagerError::new(
                ManagerErrorKind::Shutdown,
                "terminal manager is shut down",
            ));
        }
        inner.sessions.insert(id, Arc::clone(&session));
        Ok(session)
    }

    /// Resize one live session.
    ///
    /// # Errors
    ///
    /// Returns session-not-found or session resize errors.
    pub fn resize_session(
        &self,
        session_id: &str,
        lease: Option<u64>,
        rows: u16,
        columns: u16,
    ) -> Result<(), ManagerError> {
        let session = self.get(session_id)?;
        // ponytail: §3's resize fence is only as strong as what the renderer
        // presents, and the renderer has no lease to present until the pop-out
        // UI wires one through. Until then a host resize borrows whichever
        // lease is live, so a released renderer that presents nothing is still
        // indistinguishable from the host. Drop this fallback — and pass the
        // presented lease through — the moment the renderer carries one.
        let lease = lease.or_else(|| session.current_lease());
        session.resize(lease, rows, columns).map_err(Into::into)
    }

    /// Remove a session from lookup before closing it.
    ///
    /// # Errors
    ///
    /// Returns session-not-found or teardown errors.
    pub fn close_session(&self, session_id: &str, force: bool) -> Result<(), ManagerError> {
        let session = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let session = inner.sessions.remove(session_id).ok_or_else(|| {
                ManagerError::new(
                    ManagerErrorKind::SessionNotFound,
                    format!("terminal session not found: {session_id}"),
                )
            })?;
            inner
                .closing
                .insert(session_id.to_owned(), Arc::clone(&session));
            session
        };
        let result = session.close(force).map_err(ManagerError::from);
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closing
            .remove(session_id);
        result
    }

    /// Mint one single-use stream ticket for a live session and report where
    /// its replay will resume.
    ///
    /// Every mint rotates the ticket: an unused ticket and the ticket a
    /// released renderer already spent are both dead, so a leaked stream URL
    /// can never re-claim a session.
    ///
    /// # Errors
    ///
    /// Returns session-not-found, randomness, or manager-shutdown errors.
    pub fn mint_stream_ticket(
        &self,
        session_id: &str,
        from_sequence: u64,
    ) -> Result<StreamTicket, ManagerError> {
        let session = self.get(session_id)?;
        let server = self
            .stream_server
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                ManagerError::new(ManagerErrorKind::Shutdown, "terminal manager is shut down")
            })?;
        let ticket = random_opaque_value("create terminal stream ticket")?;
        let (oldest, newest) = session.replay_bounds();
        let requested = from_sequence.min(newest);
        let resume = requested.max(oldest);
        let url = server.session_url(session.id(), &ticket, resume);
        session.set_ticket(ticket);
        Ok(StreamTicket {
            url,
            from_sequence: resume,
            gap: requested < oldest,
        })
    }

    #[must_use]
    pub fn session_snapshot_bounded(&self, limit: usize) -> (Vec<SessionInfo>, usize) {
        self.snapshot(limit, MAX_SESSION_SNAPSHOT)
    }

    #[must_use]
    pub fn runtime_session_snapshot_bounded(&self, limit: usize) -> (Vec<SessionInfo>, usize) {
        self.snapshot(limit, MAX_RUNTIME_SESSION_CANDIDATES)
    }

    /// Return every authority-bearing session ID for lifecycle revocation.
    ///
    /// Unlike presentation snapshots, teardown enumeration is intentionally
    /// untruncated and includes sessions already removed from live lookup.
    #[must_use]
    pub fn lifecycle_session_ids(&self) -> Vec<String> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .sessions
            .keys()
            .chain(inner.closing.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn snapshot(&self, limit: usize, cap: usize) -> (Vec<SessionInfo>, usize) {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut sessions = BTreeMap::new();
        for (id, session) in inner.sessions.iter().chain(&inner.closing) {
            sessions.entry(id.clone()).or_insert_with(|| session.info());
        }
        let mut values: Vec<_> = sessions.into_values().collect();
        values.sort_by(|left, right| {
            right
                .started_at
                .cmp(&left.started_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        let total = values.len();
        let bounded = if limit == 0 || limit > cap {
            cap
        } else {
            limit
        };
        values.truncate(bounded);
        (values, total)
    }

    /// Return a snapshot only when every candidate fits the requested bound.
    ///
    /// # Errors
    ///
    /// Returns the frozen snapshot-limit error when truncation would occur.
    pub fn session_snapshot_exact(&self, limit: usize) -> Result<Vec<SessionInfo>, ManagerError> {
        let (values, total) = self.session_snapshot_bounded(limit);
        if total > values.len() {
            return Err(ManagerError::new(
                ManagerErrorKind::SnapshotLimit,
                "terminal session snapshot exceeds exact limit",
            ));
        }
        Ok(values)
    }

    /// Execute a callback while holding the exact session lifecycle epoch.
    ///
    /// # Errors
    ///
    /// Returns the frozen snapshot-limit error when every session cannot be
    /// represented within `maximum`.
    pub fn with_exact_session_snapshot<T>(
        &self,
        maximum: usize,
        use_snapshot: impl FnOnce(&[SessionInfo]) -> T,
    ) -> Result<T, ManagerError> {
        if maximum == 0 {
            return Err(ManagerError::new(
                ManagerErrorKind::SnapshotLimit,
                "exact terminal session snapshot requires a limit",
            ));
        }
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut sessions = BTreeMap::new();
        for (id, session) in inner.sessions.iter().chain(&inner.closing) {
            sessions.entry(id.clone()).or_insert_with(|| session.info());
        }
        if sessions.len() > maximum {
            return Err(ManagerError::new(
                ManagerErrorKind::SnapshotLimit,
                "terminal session snapshot exceeds exact limit",
            ));
        }
        let values = sessions.into_values().collect::<Vec<_>>();
        Ok(use_snapshot(&values))
    }

    /// Synchronously stop admission and request listener/stream cancellation.
    ///
    /// This is the bounded first phase used by emergency drop paths. Call
    /// [`Self::shutdown`] afterwards to join all owned work and collect errors.
    pub fn request_shutdown(&self) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .admission_closed = true;
        if let Some(server) = self
            .stream_server
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            server.request_shutdown();
        }
        self.changed.notify_waiters();
    }

    /// Associate a live terminal with an app-validated authority-free pointer.
    ///
    /// # Errors
    ///
    /// Returns session-not-found, not-live, or stale errors.
    pub fn associate_session(
        &self,
        session_id: &str,
        pointer: TerminalAssociationPointer,
    ) -> Result<TerminalAssociation, ManagerError> {
        self.get(session_id)?.associate(pointer).map_err(Into::into)
    }

    /// Prepare an association revision CAS.
    ///
    /// # Errors
    ///
    /// Returns session-not-found or stale errors.
    pub fn prepare_association_change(
        &self,
        session_id: &str,
        pointer: TerminalAssociationPointer,
        expected_revision: u64,
    ) -> Result<TerminalAssociationChange, ManagerError> {
        self.get(session_id)?
            .prepare_association_change(pointer, expected_revision)
            .map_err(Into::into)
    }

    /// Commit a prepared association revision CAS.
    ///
    /// # Errors
    ///
    /// Returns session-not-found or stale errors.
    pub fn commit_association_change(
        &self,
        change: &TerminalAssociationChange,
    ) -> Result<(), ManagerError> {
        self.get(&change.session_id)?
            .commit_association_change(change)
            .map_err(Into::into)
    }

    /// Roll back a committed association revision CAS.
    ///
    /// # Errors
    ///
    /// Returns session-not-found or stale errors.
    pub fn rollback_association_change(
        &self,
        change: &TerminalAssociationChange,
    ) -> Result<(), ManagerError> {
        self.get(&change.session_id)?
            .rollback_association_change(change)
            .map_err(Into::into)
    }

    /// Stop the listener first, wait for creates, then close sessions in parallel.
    ///
    /// # Errors
    ///
    /// Returns aggregated stream, session, and shell hook cleanup failures.
    pub async fn shutdown(self: &Arc<Self>) -> Result<(), ManagerError> {
        let start = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if inner.shutdown_done {
                return inner.shutdown_error.clone().map_or(Ok(()), Err);
            }
            inner.admission_closed = true;
            if inner.shutdown_running {
                false
            } else {
                inner.shutdown_running = true;
                true
            }
        };
        if start {
            let manager = Arc::clone(self);
            tokio::spawn(async move {
                let mut completion = ShutdownCompletion::new(Arc::clone(&manager));
                let error = manager.run_shutdown().await.err();
                completion.finish(error);
            });
        }
        loop {
            let notified = self.changed.notified();
            {
                let inner = self
                    .inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if inner.shutdown_done {
                    return inner.shutdown_error.clone().map_or(Ok(()), Err);
                }
            }
            notified.await;
        }
    }

    async fn run_shutdown(&self) -> Result<(), ManagerError> {
        let mut errors = Vec::new();
        let server = self
            .stream_server
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(server) = server
            && let Err(error) = server.shutdown().await
        {
            errors.push(format!("shutdown terminal stream server: {error}"));
        }
        loop {
            let notified = self.changed.notified();
            if self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .creates
                == 0
            {
                break;
            }
            notified.await;
        }
        let sessions = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut sessions: HashMap<_, _> = inner.sessions.drain().collect();
            sessions.extend(inner.closing.drain());
            sessions.into_values().collect::<Vec<_>>()
        };
        let workers: Vec<_> = sessions
            .into_iter()
            .map(|session| tokio::task::spawn_blocking(move || session.close(false)))
            .collect();
        for worker in workers {
            match worker.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => errors.push(error.to_string()),
                Err(_) => errors.push("terminal shutdown worker panicked".to_owned()),
            }
        }
        if let Some(shells) = self
            .shells
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            && let Err(error) = shells.close()
        {
            errors.push(error.to_string());
        }
        let error = (!errors.is_empty())
            .then(|| ManagerError::new(ManagerErrorKind::Shutdown, errors.join("; ")));
        error.map_or(Ok(()), Err)
    }

    fn finish_shutdown(&self, error: Option<ManagerError>) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.shutdown_error = error;
        inner.shutdown_running = false;
        inner.shutdown_done = true;
        self.changed.notify_waiters();
    }
}

struct ShutdownCompletion {
    manager: Arc<Manager>,
    finished: bool,
}

impl ShutdownCompletion {
    fn new(manager: Arc<Manager>) -> Self {
        Self {
            manager,
            finished: false,
        }
    }

    fn finish(&mut self, error: Option<ManagerError>) {
        self.manager.finish_shutdown(error);
        self.finished = true;
    }
}

impl Drop for ShutdownCompletion {
    fn drop(&mut self) {
        if !self.finished {
            self.manager.finish_shutdown(Some(ManagerError::new(
                ManagerErrorKind::Shutdown,
                "terminal shutdown task was cancelled",
            )));
        }
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        self.request_shutdown();
        let sessions = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut sessions: HashMap<_, _> = inner.sessions.drain().collect();
            sessions.extend(inner.closing.drain());
            sessions.into_values().collect::<Vec<_>>()
        };
        for session in sessions {
            let _ = session.close(true);
        }
        if let Some(shells) = self
            .shells
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = shells.close();
        }
    }
}

impl StreamSessionHost for Manager {
    fn stream_session(&self, session_id: &str) -> Option<Arc<dyn StreamSession>> {
        self.get(session_id)
            .ok()
            .map(|session| -> Arc<dyn StreamSession> { session })
    }
}

struct CreateGuard<'a> {
    manager: &'a Manager,
}

impl Drop for CreateGuard<'_> {
    fn drop(&mut self) {
        let mut inner = self
            .manager
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.creates = inner.creates.saturating_sub(1);
        self.manager.changed.notify_waiters();
    }
}

fn random_opaque_value(context: &'static str) -> Result<String, ManagerError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| {
        ManagerError::new(ManagerErrorKind::Launch, format!("{context}: {error}"))
    })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[must_use]
pub const fn profile_kind_name(kind: ProfileKind) -> &'static str {
    match kind {
        ProfileKind::Shell => "shell",
        ProfileKind::Agent => "agent",
    }
}
