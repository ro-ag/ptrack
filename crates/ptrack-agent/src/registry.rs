use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};
#[cfg(test)]
use std::sync::Barrier;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::{
    Association, AssociationHost, AssociationPointer, Event, EventObservation, EventPrivacyPolicy,
    Exit, LeaseState, ProcessState, ProviderEvent, RegistrationKind, Run, RunIntelligence,
    RunState, Timestamp, bind_association, default_event_privacy_policy, derive_run_intelligence,
    discover_repository_root, event_correlation_for_run, normalize_event_observation,
    normalize_provider_event, retain_events,
};
use crate::{
    EVENT_MODEL_VERSION,
    event::observation_from_persisted_event,
    persistence::{
        PERSISTED_STATE_VERSION, PersistedRecord, PersistedRegistryState, PersistenceError,
        WriteHistoryOutcome, read_history, write_history,
    },
    run::run_is_active,
};
use subtle::ConstantTimeEq;

pub const DEFAULT_LEASE_DURATION: Duration = Duration::from_secs(30);
pub const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_secs(5);
pub const DEFAULT_SNAPSHOT_LIMIT: usize = 64;
pub const DEFAULT_MAX_RECORDS: usize = 1_024;

type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;
type Random = Arc<dyn Fn(&mut [u8]) -> Result<(), String> + Send + Sync>;
type TickerFactory = Arc<dyn Fn(Duration) -> Arc<dyn RegistryTicker> + Send + Sync>;
type CwdValidator = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Registration {
    pub profile: String,
    pub provider: String,
    pub pid: i32,
    pub terminal_id: String,
    pub cwd: String,
}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct Lease {
    pub run: Run,
    pub lease_token: String,
}

impl fmt::Debug for Lease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Lease")
            .field("run", &self.run)
            .field("lease_token", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkedAssociationChange {
    pub run_id: String,
    pub terminal_id: String,
    pub previous: Association,
    pub next: Association,
}

/// Exact outcome for a terminal observation whose legacy boolean reports only
/// whether a matching run existed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RegistryMutationOutcome {
    pub matched: bool,
    pub changed: bool,
}

#[derive(Clone)]
pub struct RegistryConfig {
    pub project_root: PathBuf,
    pub lease_duration: Duration,
    pub sweep_interval: Duration,
    pub now: Option<Clock>,
    pub new_ticker: Option<TickerFactory>,
    pub random: Option<Random>,
    pub max_records: usize,
    pub additional_cwd_validator: Option<CwdValidator>,
    pub event_policy: Option<EventPrivacyPolicy>,
    pub state_path: PathBuf,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            project_root: PathBuf::new(),
            lease_duration: Duration::ZERO,
            sweep_interval: Duration::ZERO,
            now: None,
            new_ticker: None,
            random: None,
            max_records: 0,
            additional_cwd_validator: None,
            event_policy: None,
            state_path: PathBuf::new(),
        }
    }
}

pub trait RegistryTicker: Send + Sync {
    /// Waits for the next tick, returning false after [`Self::stop`].
    fn wait(&self) -> bool;
    fn stop(&self);
}

pub struct RealRegistryTicker {
    interval: Duration,
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl RealRegistryTicker {
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            stopped: Mutex::new(false),
            wake: Condvar::new(),
        }
    }
}

impl RegistryTicker for RealRegistryTicker {
    fn wait(&self) -> bool {
        let stopped = lock(&self.stopped);
        if *stopped {
            return false;
        }
        let (stopped, _) = self
            .wake
            .wait_timeout_while(stopped, self.interval, |value| !*value)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !*stopped
    }

    fn stop(&self) {
        *lock(&self.stopped) = true;
        self.wake.notify_all();
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryError {
    InvalidLease,
    RunNotFound,
    Closed,
    Full,
    AdmissionFenced,
    AssociationMismatch,
    LinkedAssociation,
    SnapshotLimit,
    EventOrder,
    InvalidEventToken,
    ShutdownTimedOut,
    Message(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidLease => "invalid AgentRun lease",
            Self::RunNotFound => "AgentRun not found",
            Self::Closed => "AgentRun registry is closed",
            Self::Full => "AgentRun registry is full",
            Self::AdmissionFenced => "AgentRun admission is fenced",
            Self::AssociationMismatch => "AgentRun association does not correspond to terminal",
            Self::LinkedAssociation => {
                "linked AgentRun association requires terminal-paired mutation"
            }
            Self::SnapshotLimit => "AgentRun snapshot exceeds exact limit",
            Self::EventOrder => "AgentRun event source order is stale or duplicated",
            Self::InvalidEventToken => "invalid AgentRun launched-event token",
            Self::ShutdownTimedOut => "AgentRun registry shutdown timed out",
            Self::Message(message) => message,
        })
    }
}

impl std::error::Error for RegistryError {}

struct Record {
    run: Run,
    lease_token: String,
    lifecycle_revision: u64,
    linked_launch: bool,
    event_token: String,
    events: Vec<Event>,
    last_source_sequence: u64,
    next_host_sequence: u64,
}

type PendingSignal = Arc<(Mutex<bool>, Condvar)>;

struct State {
    records: BTreeMap<String, Record>,
    pending_event_tokens: BTreeMap<String, PendingSignal>,
    event_token_runs: BTreeMap<String, String>,
    closed: bool,
    admission_fences: usize,
    persistence_dirty: bool,
    persistence_writable: bool,
    persistence_error: Option<String>,
}

struct RegistryInner {
    project_root: PathBuf,
    repository_root: Option<PathBuf>,
    additional_cwd_validator: Option<CwdValidator>,
    lease_duration: Duration,
    now: Clock,
    random: Random,
    ticker: Arc<dyn RegistryTicker>,
    max_records: usize,
    event_policy: EventPrivacyPolicy,
    state_path: PathBuf,
    state: Mutex<State>,
    #[cfg(test)]
    wait_barrier: Mutex<Option<Arc<Barrier>>>,
    #[cfg(test)]
    heartbeat_barrier: Mutex<Option<Arc<Barrier>>>,
}

pub struct Registry {
    inner: Arc<RegistryInner>,
    sweep_thread: Mutex<Option<JoinHandle<()>>>,
    shutdown_done: Arc<(Mutex<bool>, Condvar)>,
}

impl Registry {
    #[must_use]
    pub fn new(config: RegistryConfig) -> Self {
        let project_root = canonical_registry_path(&config.project_root);
        let lease_duration = positive_duration(config.lease_duration, DEFAULT_LEASE_DURATION);
        let sweep_interval = positive_duration(config.sweep_interval, DEFAULT_SWEEP_INTERVAL);
        let now = config.now.unwrap_or_else(|| Arc::new(Timestamp::now_utc));
        let random = config.random.unwrap_or_else(|| {
            Arc::new(|buffer: &mut [u8]| {
                getrandom::fill(buffer)
                    .map_err(|error| format!("create AgentRun opaque value: {error}"))
            })
        });
        let ticker = config.new_ticker.map_or_else(
            || Arc::new(RealRegistryTicker::new(sweep_interval)) as Arc<dyn RegistryTicker>,
            |factory| factory(sweep_interval),
        );
        let max_records = if config.max_records == 0 {
            DEFAULT_MAX_RECORDS
        } else {
            config.max_records
        };
        let event_policy = config
            .event_policy
            .map_or_else(default_event_privacy_policy, |value| {
                if retain_events(&[], Timestamp::from_unix_seconds(1), value).is_ok() {
                    value
                } else {
                    EventPrivacyPolicy::default()
                }
            });
        let repository_root = discover_repository_root(&project_root);
        let inner = Arc::new(RegistryInner {
            project_root,
            repository_root,
            additional_cwd_validator: config.additional_cwd_validator,
            lease_duration,
            now,
            random,
            ticker: Arc::clone(&ticker),
            max_records,
            event_policy,
            state_path: config.state_path,
            state: Mutex::new(State {
                records: BTreeMap::new(),
                pending_event_tokens: BTreeMap::new(),
                event_token_runs: BTreeMap::new(),
                closed: false,
                admission_fences: 0,
                persistence_dirty: false,
                persistence_writable: true,
                persistence_error: None,
            }),
            #[cfg(test)]
            wait_barrier: Mutex::new(None),
            #[cfg(test)]
            heartbeat_barrier: Mutex::new(None),
        });
        restore_history(&inner);
        let weak = Arc::downgrade(&inner);
        let shutdown_done = Arc::new((Mutex::new(false), Condvar::new()));
        let thread_done = Arc::clone(&shutdown_done);
        let sweep_thread = std::thread::spawn(move || run_sweeper(weak, ticker, &thread_done));
        Self {
            inner,
            sweep_thread: Mutex::new(Some(sweep_thread)),
            shutdown_done,
        }
    }

    /// Registers a host-launched process.
    ///
    /// # Errors
    /// Returns a closed, fenced, full, collision, or bounded-input error.
    pub fn register_launched(&self, registration: Registration) -> Result<Run, RegistryError> {
        if registration.pid <= 0 || registration.terminal_id.is_empty() {
            return message("launched AgentRun requires PID and terminal");
        }
        self.register(registration, RegistrationKind::Launched, None, None)
    }

    /// Atomically registers a launched process with a host-bound association.
    ///
    /// # Errors
    /// Returns a registration or association validation error.
    pub fn register_linked_launched(
        &self,
        registration: Registration,
        host: Option<&AssociationHost<'_>>,
        pointer: AssociationPointer,
    ) -> Result<Run, RegistryError> {
        if registration.pid <= 0 || registration.terminal_id.is_empty() {
            return message("launched AgentRun requires PID and terminal");
        }
        self.register(
            registration,
            RegistrationKind::Launched,
            host,
            Some(pointer),
        )
    }

    /// Registers an externally owned process and returns its opaque lease.
    ///
    /// # Errors
    /// Returns a closed, fenced, full, collision, or bounded-input error.
    pub fn register_external(&self, registration: Registration) -> Result<Lease, RegistryError> {
        let run = self.register(registration, RegistrationKind::External, None, None)?;
        let state = lock(&self.inner.state);
        let lease_token = state
            .records
            .get(&run.id)
            .map(|entry| entry.lease_token.clone())
            .ok_or(RegistryError::RunNotFound)?;
        Ok(Lease { run, lease_token })
    }

    fn register(
        &self,
        mut registration: Registration,
        kind: RegistrationKind,
        host: Option<&AssociationHost<'_>>,
        pointer: Option<AssociationPointer>,
    ) -> Result<Run, RegistryError> {
        trim_string(&mut registration.profile);
        trim_string(&mut registration.provider);
        if registration.profile.is_empty() || registration.provider.is_empty() {
            return message("AgentRun profile and provider are required");
        }
        let requested = if registration.cwd.is_empty() {
            self.inner.project_root.clone()
        } else {
            PathBuf::from(&registration.cwd)
        };
        let cwd = canonical_registry_path(&requested);
        if !path_within(&self.inner.project_root, &cwd)
            && (kind != RegistrationKind::Launched
                || !self
                    .inner
                    .additional_cwd_validator
                    .as_ref()
                    .is_some_and(|validator| validator(&cwd)))
        {
            return message("AgentRun CWD is outside the project");
        }
        let id = self.random_opaque_value()?;
        let lease_token = if kind == RegistrationKind::External {
            self.random_opaque_value()?
        } else {
            String::new()
        };
        let now = (self.inner.now)();
        let mut run = Run {
            id,
            profile: registration.profile,
            provider: registration.provider,
            pid: registration.pid,
            process_state: ProcessState::Unknown,
            lease_state: LeaseState::Active,
            project_root: self.inner.project_root.to_string_lossy().into_owned(),
            association: None,
            terminal_id: registration.terminal_id,
            cwd: cwd.to_string_lossy().into_owned(),
            started_at: now,
            last_activity_at: now,
            last_heartbeat_at: now,
            state: RunState::Running,
            exit: None,
            registration_kind: kind,
            lifecycle_revision: 0,
        };
        if kind == RegistrationKind::Launched {
            run.process_state = ProcessState::Running;
            run.lease_state = LeaseState::None;
            run.last_heartbeat_at = Timestamp::ZERO;
        }
        if let Some(pointer) = pointer {
            if kind != RegistrationKind::Launched || host.is_none() {
                return message("linked AgentRun requires a launched host binding");
            }
            run.association = Some(
                bind_association(host, &run.id, pointer, None)
                    .map_err(|error| RegistryError::Message(error.to_string()))?,
            );
        }
        let linked_launch = pointer.is_some();
        let mut state = lock(&self.inner.state);
        if state.closed {
            return Err(RegistryError::Closed);
        }
        if state.admission_fences > 0 {
            return Err(RegistryError::AdmissionFenced);
        }
        if state.records.contains_key(&run.id) {
            return message("AgentRun ID collision");
        }
        if state.records.len() >= self.inner.max_records && !evict_inactive(&mut state) {
            return Err(RegistryError::Full);
        }
        state.records.insert(
            run.id.clone(),
            Record {
                run: run.clone(),
                lease_token,
                lifecycle_revision: 1,
                linked_launch,
                event_token: String::new(),
                events: Vec::new(),
                last_source_sequence: 0,
                next_host_sequence: 0,
            },
        );
        persist_locked(&self.inner, &mut state);
        Ok(run)
    }

    #[must_use]
    pub fn rollback_linked_launched(&self, id: &str, terminal_id: &str) -> bool {
        self.rollback_launched_inner(id, terminal_id, true)
    }

    #[must_use]
    pub fn rollback_launched(&self, id: &str, terminal_id: &str) -> bool {
        self.rollback_launched_inner(id, terminal_id, false)
    }

    fn rollback_launched_inner(&self, id: &str, terminal_id: &str, linked: bool) -> bool {
        let mut state = lock(&self.inner.state);
        let matches = state.records.get(id).is_some_and(|entry| {
            entry.linked_launch == linked
                && entry.run.registration_kind == RegistrationKind::Launched
                && !terminal_id.is_empty()
                && entry.run.terminal_id == terminal_id
        });
        if !matches {
            return false;
        }
        revoke_record_token(&mut state, id);
        state.records.remove(id);
        persist_locked(&self.inner, &mut state);
        true
    }

    #[must_use]
    pub fn revoke_launched_event_token_for_terminal(&self, terminal_id: &str) -> bool {
        if terminal_id.is_empty() {
            return false;
        }
        let mut state = lock(&self.inner.state);
        let ids: Vec<String> = state
            .records
            .iter()
            .filter(|(_, entry)| {
                entry.run.registration_kind == RegistrationKind::Launched
                    && entry.run.terminal_id == terminal_id
                    && !entry.event_token.is_empty()
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            revoke_record_token(&mut state, id);
        }
        !ids.is_empty()
    }

    pub fn rollback_linked_terminal(&self, terminal_id: &str) -> usize {
        if terminal_id.is_empty() {
            return 0;
        }
        let mut state = lock(&self.inner.state);
        let ids: Vec<String> = state
            .records
            .iter()
            .filter(|(_, entry)| {
                entry.linked_launch
                    && entry.run.registration_kind == RegistrationKind::Launched
                    && entry.run.association.is_some()
                    && entry.run.terminal_id == terminal_id
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            revoke_record_token(&mut state, id);
            state.records.remove(id);
        }
        if !ids.is_empty() {
            persist_locked(&self.inner, &mut state);
        }
        ids.len()
    }

    #[must_use]
    pub fn has_linked_terminal(&self, terminal_id: &str) -> bool {
        !terminal_id.is_empty()
            && lock(&self.inner.state).records.values().any(|entry| {
                entry.linked_launch
                    && entry.run.registration_kind == RegistrationKind::Launched
                    && entry.run.association.is_some()
                    && entry.run.terminal_id == terminal_id
            })
    }

    #[must_use]
    pub fn is_linked_launch_run(&self, run_id: &str) -> bool {
        lock(&self.inner.state)
            .records
            .get(run_id)
            .is_some_and(|entry| entry.linked_launch)
    }

    /// Host-binds an ordinary run. Linked runs require paired terminal mutation.
    ///
    /// # Errors
    /// Returns a lookup, linked-provenance, or association validation error.
    pub fn associate(
        &self,
        id: &str,
        host: Option<&AssociationHost<'_>>,
        pointer: AssociationPointer,
    ) -> Result<Association, RegistryError> {
        let mut state = lock(&self.inner.state);
        let entry = state
            .records
            .get_mut(id)
            .ok_or(RegistryError::RunNotFound)?;
        if entry.linked_launch {
            return Err(RegistryError::LinkedAssociation);
        }
        let next = bind_association(host, &entry.run.id, pointer, entry.run.association.as_ref())
            .map_err(|error| RegistryError::Message(error.to_string()))?;
        entry.run.association = Some(next.clone());
        persist_locked(&self.inner, &mut state);
        Ok(next)
    }

    /// Prepares the registry half of an exact paired terminal association CAS.
    ///
    /// # Errors
    /// Fails closed on ambiguity, mismatch, or invalid association metadata.
    pub fn prepare_linked_terminal_association_change(
        &self,
        terminal_id: &str,
        terminal_previous: Option<&Association>,
        terminal_next: &Association,
        host: Option<&AssociationHost<'_>>,
        pointer: AssociationPointer,
    ) -> Result<Option<LinkedAssociationChange>, RegistryError> {
        let state = lock(&self.inner.state);
        let mut matches = state.records.values().filter(|entry| {
            entry.linked_launch
                && entry.run.registration_kind == RegistrationKind::Launched
                && entry.run.terminal_id == terminal_id
        });
        let Some(entry) = matches.next() else {
            return Ok(None);
        };
        if matches.next().is_some() {
            return Err(RegistryError::AssociationMismatch);
        }
        let Some(previous) = entry.run.association.as_ref() else {
            return Err(RegistryError::AssociationMismatch);
        };
        if !associations_correspond(Some(previous), terminal_previous) {
            return Err(RegistryError::AssociationMismatch);
        }
        let next = bind_association(host, &entry.run.id, pointer, Some(previous))
            .map_err(|error| RegistryError::Message(error.to_string()))?;
        if !associations_correspond(Some(&next), Some(terminal_next)) {
            return Err(RegistryError::AssociationMismatch);
        }
        Ok(Some(LinkedAssociationChange {
            run_id: entry.run.id.clone(),
            terminal_id: terminal_id.to_owned(),
            previous: previous.clone(),
            next,
        }))
    }

    /// Commits a prepared linked association CAS.
    ///
    /// # Errors
    /// Returns a correspondence error if registry state has changed.
    pub fn commit_linked_association_change(
        &self,
        change: &LinkedAssociationChange,
    ) -> Result<(), RegistryError> {
        self.apply_linked_association_change(change, &change.previous, &change.next)
    }

    /// Rolls back a previously committed linked association CAS.
    ///
    /// # Errors
    /// Returns a correspondence error if registry state has changed.
    pub fn rollback_linked_association_change(
        &self,
        change: &LinkedAssociationChange,
    ) -> Result<(), RegistryError> {
        self.apply_linked_association_change(change, &change.next, &change.previous)
    }

    fn apply_linked_association_change(
        &self,
        change: &LinkedAssociationChange,
        expected: &Association,
        replacement: &Association,
    ) -> Result<(), RegistryError> {
        let mut state = lock(&self.inner.state);
        let entry = state
            .records
            .get_mut(&change.run_id)
            .ok_or(RegistryError::AssociationMismatch)?;
        if !entry.linked_launch
            || entry.run.registration_kind != RegistrationKind::Launched
            || entry.run.terminal_id != change.terminal_id
            || entry.run.association.as_ref() != Some(expected)
        {
            return Err(RegistryError::AssociationMismatch);
        }
        entry.run.association = Some(replacement.clone());
        persist_locked(&self.inner, &mut state);
        Ok(())
    }

    #[must_use]
    pub fn fence_admission(&self) -> AdmissionFence {
        lock(&self.inner.state).admission_fences += 1;
        AdmissionFence {
            inner: Arc::downgrade(&self.inner),
            released: false,
        }
    }

    /// Refreshes an external lease and revives a stale run into a new epoch.
    ///
    /// # Errors
    /// Returns a not-found or uniform invalid-lease error.
    pub fn heartbeat(&self, id: &str, token: &str) -> Result<(), RegistryError> {
        #[cfg(test)]
        if let Some(barrier) = lock(&self.inner.heartbeat_barrier).take() {
            barrier.wait();
        }
        let mut state = lock(&self.inner.state);
        let entry = external_record_mut(&mut state, id, token)?;
        if entry.run.state == RunState::Exited {
            return Err(RegistryError::InvalidLease);
        }
        let was_active = run_is_active(&entry.run);
        let now = (self.inner.now)();
        entry.run.last_heartbeat_at = now;
        entry.run.last_activity_at = now;
        entry.run.lease_state = LeaseState::Active;
        entry.run.state = RunState::Running;
        if !was_active {
            entry.lifecycle_revision = entry.lifecycle_revision.saturating_add(1);
        }
        Ok(())
    }

    /// Records an external process exit without retaining raw result text.
    ///
    /// # Errors
    /// Returns a not-found or uniform invalid-lease error.
    pub fn exit_external(
        &self,
        id: &str,
        token: &str,
        code: i32,
        result: &str,
    ) -> Result<(), RegistryError> {
        let mut state = lock(&self.inner.state);
        external_record_mut(&mut state, id, token)?;
        record_exit(&self.inner, &mut state, id, code, result);
        persist_locked(&self.inner, &mut state);
        Ok(())
    }

    /// Verifies an external event lease before any payload parsing.
    ///
    /// # Errors
    /// Returns a not-found or uniform invalid-lease error.
    pub fn authenticate_event_lease(&self, id: &str, token: &str) -> Result<(), RegistryError> {
        let state = lock(&self.inner.state);
        let entry = external_record(&state, id, token)?;
        if !run_is_active(&entry.run) {
            return Err(RegistryError::InvalidLease);
        }
        Ok(())
    }

    /// Mints a pending token that carries no event authority until bound.
    ///
    /// # Errors
    /// Returns a closed, fenced, full, collision, or randomness error.
    pub fn issue_launched_event_token(&self) -> Result<String, RegistryError> {
        let token = self.random_opaque_value()?;
        let mut state = lock(&self.inner.state);
        if state.closed {
            return Err(RegistryError::Closed);
        }
        if state.admission_fences > 0 {
            return Err(RegistryError::AdmissionFenced);
        }
        if state.pending_event_tokens.len() >= self.inner.max_records {
            return Err(RegistryError::Full);
        }
        if state.pending_event_tokens.contains_key(&token)
            || state.event_token_runs.contains_key(&token)
        {
            return message("AgentRun opaque value collision");
        }
        state
            .pending_event_tokens
            .insert(token.clone(), Arc::new((Mutex::new(false), Condvar::new())));
        Ok(token)
    }

    /// Consumes a pending token and binds it to one active launched run.
    ///
    /// # Errors
    /// Returns uniform token failure, except that an unknown run is not-found.
    pub fn bind_launched_event_token(
        &self,
        token: &str,
        run_id: &str,
    ) -> Result<(), RegistryError> {
        let mut state = lock(&self.inner.state);
        let pending = matching_key(state.pending_event_tokens.keys(), token)
            .ok_or(RegistryError::InvalidEventToken)?;
        let entry = state
            .records
            .get(run_id)
            .ok_or(RegistryError::RunNotFound)?;
        if entry.run.registration_kind != RegistrationKind::Launched
            || !run_is_active(&entry.run)
            || !entry.event_token.is_empty()
        {
            return Err(RegistryError::InvalidEventToken);
        }
        let signal = state
            .pending_event_tokens
            .remove(&pending)
            .ok_or(RegistryError::InvalidEventToken)?;
        let entry = state
            .records
            .get_mut(run_id)
            .ok_or(RegistryError::RunNotFound)?;
        entry.event_token.clone_from(&pending);
        state.event_token_runs.insert(pending, run_id.to_owned());
        notify_signal(&signal);
        Ok(())
    }

    pub fn revoke_launched_event_token(&self, token: &str) -> bool {
        let mut state = lock(&self.inner.state);
        let mut changed = false;
        if let Some(pending) = matching_key(state.pending_event_tokens.keys(), token)
            && let Some(signal) = state.pending_event_tokens.remove(&pending)
        {
            notify_signal(&signal);
            changed = true;
        }
        if let Some((run_id, _)) = launched_event_record(&state, token) {
            revoke_record_token(&mut state, &run_id);
            changed = true;
        }
        changed
    }

    /// Verifies that a token is bound to a live launched run.
    ///
    /// # Errors
    /// Pending, exited, revoked, and unknown tokens fail identically.
    pub fn authenticate_launched_event_token(&self, token: &str) -> Result<(), RegistryError> {
        let state = lock(&self.inner.state);
        let (_, entry) = launched_event_record(&state, token)
            .filter(|(_, entry)| run_is_active(&entry.run))
            .ok_or(RegistryError::InvalidEventToken)?;
        let _ = entry;
        Ok(())
    }

    /// Waits only for a pending token to become bound within `timeout`.
    ///
    /// # Errors
    /// Timeout, revocation, shutdown, and invalid tokens fail identically.
    pub fn await_launched_event_token(
        &self,
        token: &str,
        timeout: Duration,
    ) -> Result<(), RegistryError> {
        let signal = {
            let state = lock(&self.inner.state);
            if launched_event_record(&state, token)
                .is_some_and(|(_, entry)| run_is_active(&entry.run))
            {
                return Ok(());
            }
            let pending = matching_key(state.pending_event_tokens.keys(), token)
                .ok_or(RegistryError::InvalidEventToken)?;
            Arc::clone(
                state
                    .pending_event_tokens
                    .get(&pending)
                    .ok_or(RegistryError::InvalidEventToken)?,
            )
        };
        let (ready, wake) = &*signal;
        let ready = lock(ready);
        #[cfg(test)]
        if let Some(barrier) = lock(&self.inner.wait_barrier).take() {
            barrier.wait();
        }
        let (_ready, timeout_result) = wake
            .wait_timeout_while(ready, timeout, |value| !*value)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if timeout_result.timed_out() {
            return Err(RegistryError::InvalidEventToken);
        }
        self.authenticate_launched_event_token(token)
    }

    /// Maps and records one external provider event after authentication.
    ///
    /// # Errors
    /// Returns authority, adapter, privacy, randomness, or ordering failure.
    pub fn record_provider_event(
        &self,
        id: &str,
        token: &str,
        provider_event: ProviderEvent,
    ) -> Result<Event, RegistryError> {
        let provider = {
            let state = lock(&self.inner.state);
            let entry = external_record(&state, id, token)?;
            if !run_is_active(&entry.run) {
                return Err(RegistryError::InvalidLease);
            }
            entry.run.provider.clone()
        };
        let observation = normalize_provider_event(&provider, provider_event)
            .map_err(|error| RegistryError::Message(error.to_string()))?;
        self.record_event(id, token, observation)
    }

    /// Records one normalized external event with authority rechecked at commit.
    ///
    /// # Errors
    /// Returns authority, privacy, randomness, or ordering failure.
    pub fn record_event(
        &self,
        id: &str,
        token: &str,
        observation: EventObservation,
    ) -> Result<Event, RegistryError> {
        self.authenticate_event_lease(id, token)?;
        let (now, normalized, event_id) = self.prepare_event(observation)?;
        let mut state = lock(&self.inner.state);
        let entry = external_record(&state, id, token)?;
        if !run_is_active(&entry.run) {
            return Err(RegistryError::InvalidLease);
        }
        record_normalized_event(&self.inner, &mut state, id, normalized, event_id, now)
    }

    /// Maps and records one provider event through a host-bound launched token.
    ///
    /// # Errors
    /// Returns uniform token, adapter, privacy, randomness, or ordering failure.
    pub fn record_launched_provider_event(
        &self,
        token: &str,
        provider_event: ProviderEvent,
    ) -> Result<Event, RegistryError> {
        let provider = {
            let state = lock(&self.inner.state);
            let (_, entry) = launched_event_record(&state, token)
                .filter(|(_, entry)| run_is_active(&entry.run))
                .ok_or(RegistryError::InvalidEventToken)?;
            entry.run.provider.clone()
        };
        let observation = normalize_provider_event(&provider, provider_event)
            .map_err(|error| RegistryError::Message(error.to_string()))?;
        self.record_launched_event(token, observation)
    }

    fn record_launched_event(
        &self,
        token: &str,
        observation: EventObservation,
    ) -> Result<Event, RegistryError> {
        self.authenticate_launched_event_token(token)?;
        let (now, normalized, event_id) = self.prepare_event(observation)?;
        let mut state = lock(&self.inner.state);
        let (run_id, entry) = launched_event_record(&state, token)
            .filter(|(_, entry)| run_is_active(&entry.run))
            .ok_or(RegistryError::InvalidEventToken)?;
        let _ = entry;
        record_normalized_event(&self.inner, &mut state, &run_id, normalized, event_id, now)
    }

    fn prepare_event(
        &self,
        observation: EventObservation,
    ) -> Result<(Timestamp, EventObservation, String), RegistryError> {
        let now = (self.inner.now)();
        let normalized = normalize_event_observation(
            &self.inner.project_root,
            now,
            self.inner.event_policy,
            observation,
        )
        .map_err(|error| RegistryError::Message(error.to_string()))?;
        let event_id = self.random_opaque_value()?;
        Ok((now, normalized, event_id))
    }

    fn random_opaque_value(&self) -> Result<String, RegistryError> {
        let mut bytes = [0_u8; 32];
        (self.inner.random)(&mut bytes).map_err(RegistryError::Message)?;
        Ok(raw_url_base64(&bytes))
    }

    /// Returns a retained, independently owned event timeline.
    ///
    /// # Errors
    /// Returns not-found or policy failure.
    pub fn event_snapshot(
        &self,
        id: &str,
        limit: usize,
    ) -> Result<(Vec<Event>, usize), RegistryError> {
        let mut state = lock(&self.inner.state);
        let now = (self.inner.now)();
        let entry = state
            .records
            .get_mut(id)
            .ok_or(RegistryError::RunNotFound)?;
        let total_before = entry.events.len();
        entry.events = retain_events(&entry.events, now, self.inner.event_policy)
            .map_err(|error| RegistryError::Message(error.to_string()))?;
        let pruned = total_before != entry.events.len();
        let total = entry.events.len();
        let limit = if limit == 0 || limit > self.inner.event_policy.retain_last {
            self.inner.event_policy.retain_last
        } else {
            limit
        };
        let start = entry.events.len().saturating_sub(limit);
        let events = entry.events[start..].to_vec();
        if pruned {
            persist_locked(&self.inner, &mut state);
        }
        Ok((events, total))
    }

    /// Returns lifecycle, retained evidence, and intelligence from one lock epoch.
    ///
    /// # Errors
    /// Returns not-found or policy failure.
    pub fn intelligence_snapshot(
        &self,
        id: &str,
        limit: usize,
    ) -> Result<(Run, Vec<Event>, usize, RunIntelligence), RegistryError> {
        let mut state = lock(&self.inner.state);
        let now = (self.inner.now)();
        let entry = state
            .records
            .get_mut(id)
            .ok_or(RegistryError::RunNotFound)?;
        let total_before = entry.events.len();
        entry.events = retain_events(&entry.events, now, self.inner.event_policy)
            .map_err(|error| RegistryError::Message(error.to_string()))?;
        let pruned = total_before != entry.events.len();
        let mut run = entry.run.clone();
        run.lifecycle_revision = entry.lifecycle_revision;
        let intelligence = derive_run_intelligence(&run, &entry.events);
        let total = entry.events.len();
        let limit = if limit == 0 || limit > self.inner.event_policy.retain_last {
            self.inner.event_policy.retain_last
        } else {
            limit
        };
        let start = entry.events.len().saturating_sub(limit);
        let events = entry.events[start..].to_vec();
        if pruned {
            persist_locked(&self.inner, &mut state);
        }
        Ok((run, events, total, intelligence))
    }

    /// Returns current intelligence for one run.
    ///
    /// # Errors
    /// Returns not-found or policy failure.
    pub fn intelligence(&self, id: &str) -> Result<RunIntelligence, RegistryError> {
        self.intelligence_snapshot(id, 0)
            .map(|(_, _, _, intelligence)| intelligence)
    }

    /// Returns an independent run copy.
    ///
    /// # Errors
    /// Returns not-found for an unknown identifier.
    pub fn run(&self, id: &str) -> Result<Run, RegistryError> {
        lock(&self.inner.state)
            .records
            .get(id)
            .map(|entry| entry.run.clone())
            .ok_or(RegistryError::RunNotFound)
    }

    #[must_use]
    pub fn record_terminal_activity(&self, terminal_id: &str) -> bool {
        self.record_terminal_activity_at(terminal_id, (self.inner.now)())
    }

    #[must_use]
    pub fn record_terminal_activity_at(&self, terminal_id: &str, activity_at: Timestamp) -> bool {
        self.record_terminal_activity_at_outcome(terminal_id, activity_at)
            .matched
    }

    /// Returns both match and exact state-change outcomes for a terminal clock.
    #[must_use]
    pub fn record_terminal_activity_at_outcome(
        &self,
        terminal_id: &str,
        activity_at: Timestamp,
    ) -> RegistryMutationOutcome {
        let mut state = lock(&self.inner.state);
        let Some(entry) = state.records.values_mut().find(|entry| {
            entry.run.registration_kind == RegistrationKind::Launched
                && entry.run.terminal_id == terminal_id
                && entry.run.state == RunState::Running
        }) else {
            return RegistryMutationOutcome::default();
        };
        let changed = activity_at > entry.run.last_activity_at;
        if activity_at > entry.run.last_activity_at {
            entry.run.last_activity_at = activity_at;
        }
        RegistryMutationOutcome {
            matched: true,
            changed,
        }
    }

    #[must_use]
    pub fn record_terminal_exit(&self, terminal_id: &str, code: i32, result: &str) -> bool {
        self.record_terminal_exit_outcome(terminal_id, code, result)
            .matched
    }

    /// Returns both match and exact lifecycle-change outcomes for terminal exit.
    #[must_use]
    pub fn record_terminal_exit_outcome(
        &self,
        terminal_id: &str,
        code: i32,
        result: &str,
    ) -> RegistryMutationOutcome {
        let mut state = lock(&self.inner.state);
        let ids: Vec<String> = state
            .records
            .iter()
            .filter(|(_, entry)| {
                entry.run.registration_kind == RegistrationKind::Launched
                    && entry.run.terminal_id == terminal_id
            })
            .map(|(id, _)| id.clone())
            .collect();
        let mut changed = false;
        for id in &ids {
            if state.records[id].run.state != RunState::Exited {
                record_exit(&self.inner, &mut state, id, code, result);
                changed = true;
            }
        }
        if !ids.is_empty() {
            persist_locked(&self.inner, &mut state);
        }
        RegistryMutationOutcome {
            matched: !ids.is_empty(),
            changed,
        }
    }

    pub fn sweep_expired(&self) {
        sweep_inner(&self.inner);
    }

    #[must_use]
    pub fn snapshot(&self, limit: usize) -> Vec<Run> {
        self.snapshot_bounded(limit).0
    }

    #[must_use]
    pub fn snapshot_bounded(&self, limit: usize) -> (Vec<Run>, usize) {
        self.snapshot_inner(limit, DEFAULT_SNAPSHOT_LIMIT)
    }

    #[must_use]
    pub fn runtime_snapshot_bounded(&self, limit: usize) -> (Vec<Run>, usize) {
        self.snapshot_inner(limit, DEFAULT_MAX_RECORDS)
    }

    fn snapshot_inner(&self, limit: usize, maximum: usize) -> (Vec<Run>, usize) {
        let limit = if limit == 0 || limit > maximum {
            maximum
        } else {
            limit
        };
        let state = lock(&self.inner.state);
        let mut runs: Vec<Run> = state
            .records
            .values()
            .map(|entry| entry.run.clone())
            .collect();
        drop(state);
        runs.sort_by(|left, right| {
            right
                .last_activity_at
                .cmp(&left.last_activity_at)
                .then(left.id.cmp(&right.id))
        });
        let total = runs.len();
        runs.truncate(limit);
        (runs, total)
    }

    /// Executes a callback while holding the exact lifecycle epoch.
    ///
    /// # Errors
    /// Returns a callback/limit error, snapshot overflow, or callback error.
    pub fn with_exact_runtime_snapshot<T>(
        &self,
        maximum: usize,
        use_snapshot: impl FnOnce(&[Run]) -> Result<T, RegistryError>,
    ) -> Result<T, RegistryError> {
        if maximum == 0 {
            return message("exact AgentRun snapshot callback and limit are required");
        }
        let mut state = lock(&self.inner.state);
        if sweep_expired_locked(&self.inner, &mut state, (self.inner.now)()) {
            persist_locked(&self.inner, &mut state);
        }
        if state.records.len() > maximum {
            return Err(RegistryError::SnapshotLimit);
        }
        let mut runs: Vec<Run> = state
            .records
            .values()
            .map(|entry| {
                let mut run = entry.run.clone();
                run.lifecycle_revision = entry.lifecycle_revision;
                run
            })
            .collect();
        runs.sort_by(|left, right| left.id.cmp(&right.id));
        use_snapshot(&runs)
    }

    #[must_use]
    pub fn active_count(&self) -> usize {
        lock(&self.inner.state)
            .records
            .values()
            .filter(|entry| run_is_active(&entry.run))
            .count()
    }

    #[cfg(test)]
    pub(crate) fn install_wait_barrier(&self, barrier: Arc<Barrier>) {
        *lock(&self.inner.wait_barrier) = Some(barrier);
    }

    #[cfg(test)]
    pub(crate) fn install_heartbeat_barrier(&self, barrier: Arc<Barrier>) {
        *lock(&self.inner.heartbeat_barrier) = Some(barrier);
    }

    /// Closes admissions, invalidates pending tokens, and joins the sweeper.
    ///
    /// # Errors
    /// Returns the final history persistence failure after shutdown completes.
    pub fn shutdown(&self) -> Result<(), RegistryError> {
        self.begin_shutdown();
        self.join_sweeper();
        self.final_persistence_result()
    }

    /// Closes the registry and waits no longer than `timeout` for its sweeper.
    ///
    /// # Errors
    /// Returns a fixed timeout error if an injected ticker does not stop in time.
    pub fn shutdown_timeout(&self, timeout: Duration) -> Result<(), RegistryError> {
        self.begin_shutdown();
        let (done, wake) = &*self.shutdown_done;
        let done = lock(done);
        let (done, timeout_result) = wake
            .wait_timeout_while(done, timeout, |value| !*value)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if timeout_result.timed_out() && !*done {
            return Err(RegistryError::ShutdownTimedOut);
        }
        drop(done);
        self.join_sweeper();
        self.final_persistence_result()
    }

    fn final_persistence_result(&self) -> Result<(), RegistryError> {
        lock(&self.inner.state)
            .persistence_error
            .clone()
            .map_or(Ok(()), |error| Err(RegistryError::Message(error)))
    }

    fn begin_shutdown(&self) {
        {
            let mut state = lock(&self.inner.state);
            if !state.closed {
                state.closed = true;
                let signals: Vec<PendingSignal> =
                    state.pending_event_tokens.values().cloned().collect();
                state.pending_event_tokens.clear();
                for signal in &signals {
                    notify_signal(signal);
                }
                persist_locked(&self.inner, &mut state);
            }
        }
        self.inner.ticker.stop();
    }

    fn join_sweeper(&self) {
        if let Some(thread) = lock(&self.sweep_thread).take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

pub struct AdmissionFence {
    inner: Weak<RegistryInner>,
    released: bool,
}

impl AdmissionFence {
    pub fn release(mut self) {
        self.release_inner();
    }

    fn release_inner(&mut self) {
        if self.released {
            return;
        }
        if let Some(inner) = self.inner.upgrade() {
            let mut state = lock(&inner.state);
            state.admission_fences = state.admission_fences.saturating_sub(1);
        }
        self.released = true;
    }
}

impl Drop for AdmissionFence {
    fn drop(&mut self) {
        self.release_inner();
    }
}

#[allow(clippy::needless_pass_by_value)]
fn run_sweeper(
    inner: Weak<RegistryInner>,
    ticker: Arc<dyn RegistryTicker>,
    shutdown_done: &PendingSignal,
) {
    while ticker.wait() {
        let Some(inner) = inner.upgrade() else {
            break;
        };
        sweep_inner(&inner);
    }
    let (done, wake) = &**shutdown_done;
    *lock(done) = true;
    wake.notify_all();
}

fn sweep_inner(inner: &RegistryInner) {
    let now = (inner.now)();
    let mut state = lock(&inner.state);
    let mut changed = false;
    for entry in state.records.values_mut() {
        if let Ok(retained) = retain_events(&entry.events, now, inner.event_policy) {
            changed |= retained.len() != entry.events.len();
            entry.events = retained;
        }
    }
    changed |= sweep_expired_locked(inner, &mut state, now);
    if changed || state.persistence_dirty {
        persist_locked(inner, &mut state);
    }
}

fn sweep_expired_locked(inner: &RegistryInner, state: &mut State, now: Timestamp) -> bool {
    let mut changed = false;
    for entry in state.records.values_mut() {
        if entry.run.registration_kind != RegistrationKind::External
            || entry.run.state == RunState::Exited
            || entry.run.lease_state != LeaseState::Active
        {
            continue;
        }
        if elapsed_greater_than(now, entry.run.last_heartbeat_at, inner.lease_duration) {
            entry.run.state = RunState::Stale;
            entry.run.process_state = ProcessState::Unknown;
            entry.run.lease_state = LeaseState::Expired;
            entry.lifecycle_revision = entry.lifecycle_revision.saturating_add(1);
            changed = true;
        }
    }
    changed
}

fn record_normalized_event(
    inner: &RegistryInner,
    state: &mut State,
    id: &str,
    normalized: EventObservation,
    event_id: String,
    now: Timestamp,
) -> Result<Event, RegistryError> {
    let entry = state
        .records
        .get_mut(id)
        .ok_or(RegistryError::RunNotFound)?;
    if normalized.source_sequence <= entry.last_source_sequence
        || entry
            .events
            .iter()
            .any(|event| event.source_id == normalized.source_id)
    {
        return Err(RegistryError::EventOrder);
    }
    let host_sequence = entry.next_host_sequence.saturating_add(1);
    let event = Event {
        model_version: EVENT_MODEL_VERSION,
        id: event_id,
        run_id: entry.run.id.clone(),
        provider: entry.run.provider.clone(),
        source_id: normalized.source_id,
        source_sequence: normalized.source_sequence,
        host_sequence,
        lifecycle_revision: entry.lifecycle_revision,
        kind: normalized.kind,
        phase: normalized.phase,
        outcome: normalized.outcome,
        subject: normalized.subject,
        paths: normalized.paths,
        commit_sha: normalized.commit_sha,
        exit_code: normalized.exit_code,
        error_class: normalized.error_class,
        summary: normalized.summary,
        occurred_at: normalized.occurred_at,
        observed_at: now,
        correlation: event_correlation_for_run(&entry.run, inner.repository_root.as_deref()),
        notification: normalized.notification,
    };
    let mut candidate = entry.events.clone();
    candidate.push(event.clone());
    entry.events = retain_events(&candidate, now, inner.event_policy)
        .map_err(|error| RegistryError::Message(error.to_string()))?;
    entry.next_host_sequence = host_sequence;
    entry.last_source_sequence = normalized.source_sequence;
    if now > entry.run.last_activity_at {
        entry.run.last_activity_at = now;
    }
    persist_locked(inner, state);
    Ok(event)
}

fn record_exit(inner: &RegistryInner, state: &mut State, id: &str, code: i32, result: &str) {
    revoke_record_token(state, id);
    let Some(entry) = state.records.get_mut(id) else {
        return;
    };
    let now = (inner.now)();
    entry.run.state = RunState::Exited;
    entry.run.process_state = ProcessState::Exited;
    if entry.run.registration_kind == RegistrationKind::External {
        entry.run.lease_state = LeaseState::Expired;
    }
    entry.run.last_activity_at = now;
    entry.run.exit = Some(Exit {
        code,
        result: classify_exit_result(result, code),
        occurred_at: now,
    });
    entry.lifecycle_revision = entry.lifecycle_revision.saturating_add(1);
}

fn classify_exit_result(result: &str, code: i32) -> String {
    let normalized = result.trim().to_lowercase();
    if matches!(
        normalized.as_str(),
        "completed"
            | "done"
            | "success"
            | "succeeded"
            | "failed"
            | "error"
            | "cancelled"
            | "canceled"
            | "timeout"
            | "timed_out"
            | "interrupted"
            | "killed"
            | "session restarted"
            | "unknown"
    ) {
        normalized
    } else if code == 0 {
        "completed".to_owned()
    } else {
        "failed".to_owned()
    }
}

fn external_record<'a>(
    state: &'a State,
    id: &str,
    token: &str,
) -> Result<&'a Record, RegistryError> {
    let entry = state.records.get(id).ok_or(RegistryError::RunNotFound)?;
    if entry.run.registration_kind != RegistrationKind::External
        || token.is_empty()
        || !constant_time_eq(entry.lease_token.as_bytes(), token.as_bytes())
    {
        return Err(RegistryError::InvalidLease);
    }
    Ok(entry)
}

fn external_record_mut<'a>(
    state: &'a mut State,
    id: &str,
    token: &str,
) -> Result<&'a mut Record, RegistryError> {
    let entry = state
        .records
        .get_mut(id)
        .ok_or(RegistryError::RunNotFound)?;
    if entry.run.registration_kind != RegistrationKind::External
        || token.is_empty()
        || !constant_time_eq(entry.lease_token.as_bytes(), token.as_bytes())
    {
        return Err(RegistryError::InvalidLease);
    }
    Ok(entry)
}

fn launched_event_record<'a>(state: &'a State, token: &str) -> Option<(String, &'a Record)> {
    let matched = matching_key(state.event_token_runs.keys(), token)?;
    let run_id = state.event_token_runs.get(&matched)?.clone();
    let entry = state.records.get(&run_id)?;
    constant_time_eq(entry.event_token.as_bytes(), matched.as_bytes()).then_some((run_id, entry))
}

fn matching_key<'a>(mut keys: impl Iterator<Item = &'a String>, supplied: &str) -> Option<String> {
    if supplied.is_empty() {
        return None;
    }
    keys.find(|candidate| constant_time_eq(candidate.as_bytes(), supplied.as_bytes()))
        .cloned()
}

fn revoke_record_token(state: &mut State, id: &str) {
    let token = state
        .records
        .get(id)
        .map(|entry| entry.event_token.clone())
        .unwrap_or_default();
    if token.is_empty() {
        return;
    }
    state.event_token_runs.remove(&token);
    if let Some(entry) = state.records.get_mut(id) {
        entry.event_token.clear();
    }
}

fn evict_inactive(state: &mut State) -> bool {
    let oldest = state
        .records
        .iter()
        .filter(|(_, entry)| !run_is_active(&entry.run))
        .min_by(|(left_id, left), (right_id, right)| {
            left.run
                .last_activity_at
                .cmp(&right.run.last_activity_at)
                .then(left_id.cmp(right_id))
        })
        .map(|(id, _)| id.clone());
    let Some(id) = oldest else {
        return false;
    };
    revoke_record_token(state, &id);
    state.records.remove(&id);
    true
}

fn restore_history(inner: &RegistryInner) {
    if inner.state_path.as_os_str().is_empty() {
        return;
    }
    let mut persisted = match read_history(&inner.state_path) {
        Ok(Some(state)) => state,
        Err(PersistenceError::FutureVersion { .. }) => {
            lock(&inner.state).persistence_writable = false;
            return;
        }
        Ok(None) | Err(_) => return,
    };
    persisted
        .runs
        .sort_by_key(|record| std::cmp::Reverse(record.run.last_activity_at));
    persisted.runs.truncate(inner.max_records);
    let mut state = lock(&inner.state);
    for persisted in persisted.runs {
        let mut run = persisted.run;
        run.project_root = canonical_registry_path(Path::new(&run.project_root))
            .to_string_lossy()
            .into_owned();
        run.cwd = canonical_registry_path(Path::new(&run.cwd))
            .to_string_lossy()
            .into_owned();
        if run.id.is_empty()
            || Path::new(&run.project_root) != inner.project_root
            || !restored_cwd_allowed(inner, &run)
            || state.records.contains_key(&run.id)
        {
            continue;
        }
        if run.registration_kind == RegistrationKind::Launched && run.state != RunState::Exited {
            run.state = RunState::Stale;
            run.process_state = ProcessState::Unknown;
        }
        if let Some(exit) = run.exit.as_mut() {
            exit.result = classify_exit_result(&exit.result, exit.code);
        }
        run.association = None;
        let events = restore_events(inner, &run, persisted.events);
        let mut last_source_sequence = persisted.last_source_sequence;
        let mut next_host_sequence = persisted.next_host_sequence;
        let mut lifecycle_revision = 1_u64;
        for event in &events {
            last_source_sequence = last_source_sequence.max(event.source_sequence);
            next_host_sequence = next_host_sequence.max(event.host_sequence);
            lifecycle_revision = lifecycle_revision.max(event.lifecycle_revision.saturating_add(1));
        }
        let lease_token = if run.registration_kind == RegistrationKind::Launched {
            String::new()
        } else {
            persisted.lease_token
        };
        state.records.insert(
            run.id.clone(),
            Record {
                run,
                lease_token,
                lifecycle_revision,
                linked_launch: false,
                event_token: String::new(),
                events,
                last_source_sequence,
                next_host_sequence,
            },
        );
    }
    state.persistence_dirty = true;
}

fn restored_cwd_allowed(inner: &RegistryInner, run: &Run) -> bool {
    let cwd = Path::new(&run.cwd);
    path_within(&inner.project_root, cwd)
        || (run.registration_kind == RegistrationKind::Launched
            && inner
                .additional_cwd_validator
                .as_ref()
                .is_some_and(|validator| validator(cwd)))
}

fn restore_events(inner: &RegistryInner, run: &Run, mut events: Vec<Event>) -> Vec<Event> {
    if !inner.event_policy.collection_enabled {
        return Vec::new();
    }
    events.sort_by_key(|event| event.host_sequence);
    let now = (inner.now)();
    let mut validated = Vec::with_capacity(events.len());
    let mut seen_ids = std::collections::BTreeSet::new();
    let mut last_source_sequence = 0;
    let mut last_host_sequence = 0;
    for mut event in events {
        event.correlation.project_root =
            canonical_registry_path(Path::new(&event.correlation.project_root))
                .to_string_lossy()
                .into_owned();
        if !event.correlation.repository_root.is_empty() {
            event.correlation.repository_root =
                canonical_registry_path(Path::new(&event.correlation.repository_root))
                    .to_string_lossy()
                    .into_owned();
        }
        if event.model_version != EVENT_MODEL_VERSION
            || event.id.is_empty()
            || event.run_id != run.id
            || event.provider != run.provider
            || event.source_sequence <= last_source_sequence
            || event.host_sequence <= last_host_sequence
            || seen_ids.contains(&event.id)
            || event.observed_at.is_zero()
            || event.observed_at > now.add_seconds(5 * 60)
            || !valid_persisted_event_correlation(inner, run, &event)
            || event.lifecycle_revision == u64::MAX
        {
            continue;
        }
        let Ok(normalized) = normalize_event_observation(
            &inner.project_root,
            event.observed_at,
            inner.event_policy,
            observation_from_persisted_event(&event),
        ) else {
            continue;
        };
        event.source_id = normalized.source_id;
        event.subject = normalized.subject;
        event.paths = normalized.paths;
        event.commit_sha = normalized.commit_sha;
        event.exit_code = normalized.exit_code;
        event.error_class = normalized.error_class;
        event.summary = normalized.summary;
        event.occurred_at = normalized.occurred_at;
        event.notification = normalized.notification;
        if event.lifecycle_revision == 0 {
            event.lifecycle_revision = 1;
        }
        last_source_sequence = event.source_sequence;
        last_host_sequence = event.host_sequence;
        seen_ids.insert(event.id.clone());
        validated.push(event);
    }
    retain_events(&validated, now, inner.event_policy).unwrap_or_default()
}

fn valid_persisted_event_correlation(inner: &RegistryInner, run: &Run, event: &Event) -> bool {
    let correlation = &event.correlation;
    let repository_root = inner
        .repository_root
        .as_ref()
        .map(|path| path.to_string_lossy())
        .unwrap_or_default();
    if Path::new(&correlation.project_root) != inner.project_root
        || Path::new(&run.project_root) != inner.project_root
        || correlation.terminal_id != run.terminal_id
        || correlation.repository_root != repository_root
        || (correlation.task_id != 0 && correlation.plan_id == 0)
    {
        return false;
    }
    let has_association = correlation.generation != 0
        || correlation.association_revision != 0
        || correlation.plan_id != 0
        || correlation.task_id != 0;
    !has_association || (correlation.generation != 0 && correlation.association_revision != 0)
}

fn persist_locked(inner: &RegistryInner, state: &mut State) {
    if inner.state_path.as_os_str().is_empty() || !state.persistence_writable {
        return;
    }
    let now = (inner.now)();
    for entry in state.records.values_mut() {
        let Ok(events) = retain_events(&entry.events, now, inner.event_policy) else {
            state.persistence_dirty = true;
            return;
        };
        entry.events = events;
    }
    let mut runs: Vec<PersistedRecord> = state
        .records
        .values()
        .map(|entry| {
            let mut run = entry.run.clone();
            run.association = None;
            PersistedRecord {
                run,
                lease_token: entry.lease_token.clone(),
                events: entry.events.clone(),
                last_source_sequence: entry.last_source_sequence,
                next_host_sequence: entry.next_host_sequence,
            }
        })
        .collect();
    runs.sort_by(|left, right| {
        right
            .run
            .last_activity_at
            .cmp(&left.run.last_activity_at)
            .then(left.run.id.cmp(&right.run.id))
    });
    runs.truncate(inner.max_records);
    let persisted = PersistedRegistryState {
        version: PERSISTED_STATE_VERSION,
        saved_at: now,
        runs,
    };
    match write_history(&inner.state_path, &persisted) {
        Ok(WriteHistoryOutcome::Written) => {
            state.persistence_dirty = false;
            state.persistence_error = None;
        }
        Ok(WriteHistoryOutcome::FutureVersion) => {
            state.persistence_writable = false;
            state.persistence_dirty = false;
            state.persistence_error = None;
        }
        Err(error) => {
            state.persistence_dirty = true;
            let error = error.to_string();
            state.persistence_error = Some(if error.starts_with("write AgentRun history:") {
                error
            } else {
                format!("write AgentRun history: {error}")
            });
        }
    }
}

fn associations_correspond(left: Option<&Association>, right: Option<&Association>) -> bool {
    left.zip(right).is_some_and(|(left, right)| {
        left.version == right.version
            && left.project_root == right.project_root
            && left.generation == right.generation
            && left.target == right.target
            && left.revision == right.revision
    })
}

fn elapsed_greater_than(now: Timestamp, earlier: Timestamp, duration: Duration) -> bool {
    let Some(now) = now.unix_nanoseconds() else {
        return false;
    };
    let Some(earlier) = earlier.unix_nanoseconds() else {
        return false;
    };
    now.saturating_sub(earlier)
        > i128::from(duration.as_secs()) * 1_000_000_000 + i128::from(duration.subsec_nanos())
}

fn positive_duration(value: Duration, fallback: Duration) -> Duration {
    if value.is_zero() { fallback } else { value }
}

fn path_within(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_ok()
}

fn canonical_registry_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |current| current.join(path))
    };
    if let Ok(canonical) = std::fs::canonicalize(&absolute) {
        return canonical;
    }
    let absolute = clean_path(&absolute);
    let mut probe = absolute.clone();
    let mut missing = Vec::new();
    loop {
        if let Ok(mut canonical) = std::fs::canonicalize(&probe) {
            for component in missing.iter().rev() {
                canonical.push(component);
            }
            return clean_path(&canonical);
        }
        let Some(name) = probe.file_name().map(std::borrow::ToOwned::to_owned) else {
            return absolute;
        };
        missing.push(name);
        if !probe.pop() {
            return absolute;
        }
    }
}

fn clean_path(path: &Path) -> PathBuf {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop();
            }
            _ => clean.push(component.as_os_str()),
        }
    }
    clean
}

fn trim_string(value: &mut String) {
    let trimmed = value.trim();
    let start = trimmed.as_ptr() as usize - value.as_ptr() as usize;
    let end = start + trimmed.len();
    value.truncate(end);
    value.drain(..start);
}

fn raw_url_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        result.push(char::from(ALPHABET[usize::from(first >> 2)]));
        result.push(char::from(
            ALPHABET[usize::from(((first & 0x03) << 4) | (second >> 4))],
        ));
        if chunk.len() > 1 {
            result.push(char::from(
                ALPHABET[usize::from(((second & 0x0f) << 2) | (third >> 6))],
            ));
        }
        if chunk.len() > 2 {
            result.push(char::from(ALPHABET[usize::from(third & 0x3f)]));
        }
    }
    result
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

fn notify_signal(signal: &PendingSignal) {
    let (ready, wake) = &**signal;
    *lock(ready) = true;
    wake.notify_all();
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn message<T>(value: impl Into<String>) -> Result<T, RegistryError> {
    Err(RegistryError::Message(value.into()))
}
