use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::mpsc as tokio_mpsc;

use crate::profile::ProfileKind;
use crate::pty::{PtyFactory, PtyProcess, StartRequest};
use crate::shell_integration::ShellIntegrationDescriptor;
use crate::stream::{StreamAttachRefusal, StreamAttachment, StreamSession, StreamSessionError};

pub const DEFAULT_REPLAY_BUFFER_BYTES: usize = 256 * 1024;
pub const DEFAULT_GRACEFUL_TIMEOUT: Duration = Duration::from_millis(750);
pub const DEFAULT_OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);
pub const MAX_TERMINAL_ROWS: u16 = 1_000;
pub const MAX_TERMINAL_COLUMNS: u16 = 1_000;
const OUTPUT_READ_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Starting,
    Running,
    Exited,
    Closing,
    Closed,
    Failed,
}

impl fmt::Display for SessionState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Closing => "closing",
            Self::Closed => "closed",
            Self::Failed => "failed",
        };
        formatter.write_str(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionError(String);

impl SessionError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SessionError {}

impl From<io::Error> for SessionError {
    fn from(error: io::Error) -> Self {
        Self::new(error.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExitResult {
    pub exit_code: i32,
    pub state: SessionState,
    pub error: Option<String>,
}

/// Authority-free association pointer. Validation and host conversion belong
/// to the application adapter; a terminal session owns only revision fencing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TerminalAssociationPointer {
    pub version: u8,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub plan_id: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub task_id: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TerminalAssociation {
    pub pointer: TerminalAssociationPointer,
    pub revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalAssociationChange {
    pub session_id: String,
    pub previous: Option<TerminalAssociation>,
    pub next: TerminalAssociation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub profile_id: String,
    pub profile_kind: ProfileKind,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub provider: String,
    pub pid: u32,
    pub cwd: String,
    pub state: SessionState,
    pub started_at: String,
    pub last_activity_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub association: Option<TerminalAssociation>,
}

#[derive(Clone, Debug)]
pub struct SessionMetadata {
    pub id: String,
    pub profile_id: String,
    pub profile_kind: ProfileKind,
    pub provider: String,
    pub cwd: String,
    pub shell_integration: ShellIntegrationDescriptor,
}

#[derive(Clone, Copy, Debug)]
pub struct SessionOptions {
    pub replay_buffer_bytes: usize,
    pub graceful_timeout: Duration,
    pub output_drain_timeout: Duration,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            replay_buffer_bytes: DEFAULT_REPLAY_BUFFER_BYTES,
            graceful_timeout: DEFAULT_GRACEFUL_TIMEOUT,
            output_drain_timeout: DEFAULT_OUTPUT_DRAIN_TIMEOUT,
        }
    }
}

/// Bounded, sequenced, in-memory replay window.
///
/// Sequence numbers count every byte the PTY ever produced, so a re-attaching
/// renderer resumes mid-chunk exactly. Oldest bytes are dropped once the budget
/// is reached: the PTY is never stalled by a renderer that is not attached.
#[derive(Debug)]
struct ReplayRing {
    bytes: VecDeque<u8>,
    capacity: usize,
    end: u64,
}

impl ReplayRing {
    fn new(capacity: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            capacity,
            end: 0,
        }
    }

    fn start(&self) -> u64 {
        self.end
            .saturating_sub(u64::try_from(self.bytes.len()).unwrap_or(u64::MAX))
    }

    fn append(&mut self, output: &[u8]) {
        self.end = self
            .end
            .saturating_add(u64::try_from(output.len()).unwrap_or(u64::MAX));
        let retained = output.len().min(self.capacity);
        self.bytes.extend(&output[output.len() - retained..]);
        let overflow = self.bytes.len().saturating_sub(self.capacity);
        drop(self.bytes.drain(..overflow));
    }

    /// Retained bytes from `sequence` onwards, or `None` when that point is no
    /// longer retained or was never produced.
    fn replay_from(&self, sequence: u64) -> Option<Vec<u8>> {
        if sequence < self.start() || sequence > self.end {
            return None;
        }
        let skip = usize::try_from(sequence - self.start()).ok()?;
        let (front, back) = self.bytes.as_slices();
        let mut replay = Vec::with_capacity(self.bytes.len() - skip);
        replay.extend_from_slice(&front[skip.min(front.len())..]);
        replay.extend_from_slice(&back[skip.saturating_sub(front.len()).min(back.len())..]);
        Some(replay)
    }
}

#[allow(clippy::struct_excessive_bools)]
struct SessionInner {
    state: SessionState,
    process: Option<Arc<dyn PtyProcess>>,
    pid: u32,
    rows: u16,
    columns: u16,
    started_at: String,
    last_activity_at: String,
    replay: ReplayRing,
    ticket: Option<String>,
    lease: u64,
    attached: bool,
    unattached_since: Option<Instant>,
    reclaim_expired: bool,
    live_sender: Option<tokio_mpsc::Sender<Vec<u8>>>,
    output_done: bool,
    association: Option<TerminalAssociation>,
    stream_error: Option<String>,
    exit_done: bool,
    close_started: bool,
    close_done: bool,
    close_error: Option<SessionError>,
    process_closed: bool,
}

pub struct Session {
    metadata: SessionMetadata,
    request: StartRequest,
    factory: Arc<dyn PtyFactory>,
    options: SessionOptions,
    inner: Mutex<SessionInner>,
    changed: Condvar,
    closing: AtomicBool,
    exit_sender: Mutex<Option<mpsc::SyncSender<ExitResult>>>,
    exit_receiver: Mutex<Option<mpsc::Receiver<ExitResult>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl fmt::Debug for Session {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Session")
            .field("id", &self.metadata.id)
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

impl Session {
    #[must_use]
    pub fn new(
        mut request: StartRequest,
        metadata: SessionMetadata,
        factory: Arc<dyn PtyFactory>,
    ) -> Arc<Self> {
        let (rows, columns) = clamp_dimensions(request.rows, request.columns);
        request.rows = rows;
        request.columns = columns;
        Self::new_with_options(request, metadata, factory, SessionOptions::default())
    }

    #[must_use]
    pub fn new_with_options(
        mut request: StartRequest,
        metadata: SessionMetadata,
        factory: Arc<dyn PtyFactory>,
        mut options: SessionOptions,
    ) -> Arc<Self> {
        if options.replay_buffer_bytes == 0 {
            options.replay_buffer_bytes = DEFAULT_REPLAY_BUFFER_BYTES;
        }
        if options.graceful_timeout.is_zero() {
            options.graceful_timeout = DEFAULT_GRACEFUL_TIMEOUT;
        }
        if options.output_drain_timeout.is_zero() {
            options.output_drain_timeout = DEFAULT_OUTPUT_DRAIN_TIMEOUT;
        }
        let (rows, columns) = clamp_dimensions(request.rows, request.columns);
        request.rows = rows;
        request.columns = columns;
        let (exit_sender, exit_receiver) = mpsc::sync_channel(1);
        Arc::new(Self {
            metadata,
            request,
            factory,
            options,
            inner: Mutex::new(SessionInner {
                state: SessionState::Starting,
                process: None,
                pid: 0,
                rows,
                columns,
                started_at: String::new(),
                last_activity_at: String::new(),
                replay: ReplayRing::new(options.replay_buffer_bytes),
                ticket: None,
                lease: 0,
                attached: false,
                unattached_since: Some(Instant::now()),
                reclaim_expired: false,
                live_sender: None,
                output_done: false,
                association: None,
                stream_error: None,
                exit_done: false,
                close_started: false,
                close_done: false,
                close_error: None,
                process_closed: false,
            }),
            changed: Condvar::new(),
            closing: AtomicBool::new(false),
            exit_sender: Mutex::new(Some(exit_sender)),
            exit_receiver: Mutex::new(Some(exit_receiver)),
            workers: Mutex::new(Vec::new()),
        })
    }

    /// Start the session once and launch its owned output/wait workers.
    ///
    /// # Errors
    ///
    /// Returns the PTY start error and transitions the session to `failed`.
    pub fn start(self: &Arc<Self>) -> Result<(), SessionError> {
        {
            let inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if inner.state != SessionState::Starting {
                return Err(SessionError::new(format!(
                    "start terminal session in state {:?}",
                    inner.state.to_string()
                )));
            }
        }
        let process = match self.factory.start(self.request.clone()) {
            Ok(process) => Arc::<dyn PtyProcess>::from(process),
            Err(error) => {
                self.inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .state = SessionState::Failed;
                return Err(SessionError::new(format!("start terminal PTY: {error}")));
            }
        };
        let now = now_string();
        {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.pid = process.pid();
            inner.process = Some(Arc::clone(&process));
            inner.started_at.clone_from(&now);
            inner.last_activity_at = now;
            inner.state = SessionState::Running;
        }
        let (reader_done_tx, reader_done_rx) = mpsc::sync_channel(1);
        let output_session = Arc::clone(self);
        let output_process = Arc::clone(&process);
        let output = match thread::Builder::new()
            .name(format!("ptrack-terminal-output-{}", self.id()))
            .spawn(move || {
                output_session.read_output(&*output_process);
                let _ = reader_done_tx.send(());
            }) {
            Ok(output) => output,
            Err(error) => {
                return Err(self.fail_worker_start(
                    &*process,
                    format!("start terminal output worker: {error}"),
                ));
            }
        };
        self.workers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(output);
        let wait_session = Arc::clone(self);
        let wait_process = Arc::clone(&process);
        let wait = match thread::Builder::new()
            .name(format!("ptrack-terminal-wait-{}", self.id()))
            .spawn(move || wait_session.wait_for_exit(&*wait_process, &reader_done_rx))
        {
            Ok(wait) => wait,
            Err(error) => {
                return Err(self
                    .fail_worker_start(&*process, format!("start terminal wait worker: {error}")));
            }
        };
        self.workers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(wait);
        Ok(())
    }

    fn fail_worker_start(&self, process: &dyn PtyProcess, failure: String) -> SessionError {
        self.closing.store(true, Ordering::Release);
        let mut failures = vec![failure];
        if let Err(error) = process.kill() {
            failures.push(format!("kill failed terminal worker start: {error}"));
        }
        if let Err(error) = self.close_process(process) {
            failures.push(format!("close failed terminal worker start: {error}"));
        }
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.state = SessionState::Failed;
        inner.exit_done = true;
        inner.live_sender.take();
        drop(inner);
        self.exit_sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.changed.notify_all();
        SessionError::new(failures.join("; "))
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.metadata.id
    }

    #[must_use]
    pub fn shell_integration(&self) -> &ShellIntegrationDescriptor {
        &self.metadata.shell_integration
    }

    #[must_use]
    pub fn state(&self) -> SessionState {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state
    }

    #[must_use]
    pub fn info(&self) -> SessionInfo {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        SessionInfo {
            id: self.metadata.id.clone(),
            profile_id: self.metadata.profile_id.clone(),
            profile_kind: self.metadata.profile_kind,
            provider: self.metadata.provider.clone(),
            pid: inner.pid,
            cwd: self.metadata.cwd.clone(),
            state: inner.state,
            started_at: inner.started_at.clone(),
            last_activity_at: inner.last_activity_at.clone(),
            association: inner.association.clone(),
        }
    }

    pub fn take_exit_results(&self) -> Option<mpsc::Receiver<ExitResult>> {
        self.exit_receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    /// Race the renderer re-claim grace window exactly once.
    ///
    /// # Errors
    ///
    /// Returns the duration after which it is worth checking again when the
    /// session is attached, still re-claimable, or already torn down.
    pub fn reclaim_window_expiry_wins(&self, grace: Duration) -> Result<(), Duration> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.reclaim_expired
            || matches!(inner.state, SessionState::Closing | SessionState::Closed)
        {
            return Err(grace);
        }
        let Some(unattached_since) = inner.unattached_since else {
            return Err(grace);
        };
        let waited = unattached_since.elapsed();
        if waited < grace {
            return Err(grace.saturating_sub(waited));
        }
        inner.reclaim_expired = true;
        Ok(())
    }

    pub(crate) fn set_ticket(&self, ticket: String) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ticket = Some(ticket);
    }

    /// Compare a presented stream ticket in constant time and burn it on match.
    #[must_use]
    pub fn consume_ticket(&self, presented: &str) -> bool {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let matched = ticket_matches(&inner, presented);
        if matched {
            inner.ticket = None;
        }
        matched
    }

    /// Claim the lease with a presented single-use ticket, burning the ticket
    /// only once the lease is actually granted. Ticket check and attachment
    /// share one lock, so the ticket can never be spent on a refusal.
    ///
    /// # Errors
    ///
    /// Returns why the attachment was refused. A refusal never terminates the
    /// session and never spends the ticket.
    pub fn attach_with_ticket(
        &self,
        presented: &str,
        from_sequence: u64,
    ) -> Result<StreamAttachment, StreamAttachRefusal> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !ticket_matches(&inner, presented) {
            return Err(StreamAttachRefusal::Unauthorized);
        }
        let attachment = self
            .attach_locked(&mut inner, from_sequence)
            .map_err(|_| StreamAttachRefusal::Unavailable)?;
        inner.ticket = None;
        Ok(attachment)
    }

    /// Oldest retained and newest produced replay sequence numbers.
    #[must_use]
    pub fn replay_bounds(&self) -> (u64, u64) {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (inner.replay.start(), inner.replay.end)
    }

    /// Claim the single renderer lease, replaying retained output from
    /// `from_sequence`.
    ///
    /// # Errors
    ///
    /// Returns an error when a lease is held, the re-claim window expired, the
    /// requested sequence was never produced, or the session is terminal.
    /// A refusal never terminates the session.
    pub fn attach_output(&self, from_sequence: u64) -> Result<StreamAttachment, SessionError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.attach_locked(&mut inner, from_sequence)
    }

    fn attach_locked(
        &self,
        inner: &mut SessionInner,
        from_sequence: u64,
    ) -> Result<StreamAttachment, SessionError> {
        if inner.attached {
            return Err(SessionError::new("terminal output is already attached"));
        }
        if inner.reclaim_expired {
            return Err(SessionError::new(
                "terminal output attachment lease expired",
            ));
        }
        if matches!(inner.state, SessionState::Failed | SessionState::Closed) {
            return Err(SessionError::new(format!(
                "attach terminal output in state {:?}",
                inner.state.to_string()
            )));
        }
        // Claiming output the PTY never produced is a renderer bug and is
        // refused. A wrapped buffer is not: it resumes from the oldest retained
        // byte and reports the gap, because the clamp a mint computed is
        // already stale by the time the socket connects.
        if from_sequence > inner.replay.end {
            return Err(SessionError::new("terminal replay sequence is unavailable"));
        }
        let resumed = from_sequence.max(inner.replay.start());
        let replay = inner.replay.replay_from(resumed).unwrap_or_default();
        inner.lease = inner.lease.saturating_add(1);
        inner.attached = true;
        inner.unattached_since = None;
        let lease = inner.lease;
        let (sender, receiver) = tokio_mpsc::channel(1);
        // The reader thread is gone once output is done, so nothing would ever
        // send on or drop a fresh sender. Leaving it unset ends the stream after
        // the replay instead of pinning an exited session past its grace window.
        if !inner.output_done {
            inner.live_sender = Some(sender);
        }
        self.changed.notify_all();
        Ok(StreamAttachment {
            lease,
            gap: resumed != from_sequence,
            replay,
            live: receiver,
        })
    }

    /// Release an exact renderer lease without terminating the session. The PTY
    /// keeps running and its output keeps accumulating in the replay window.
    pub fn release_output(&self, lease: u64) -> bool {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !inner.attached || inner.lease != lease {
            return false;
        }
        inner.attached = false;
        inner.lease = inner.lease.saturating_add(1);
        inner.unattached_since = Some(Instant::now());
        inner.live_sender.take();
        self.changed.notify_all();
        true
    }

    /// The lease a renderer holds right now, if one does.
    #[must_use]
    pub fn current_lease(&self) -> Option<u64> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.attached.then_some(inner.lease)
    }

    /// Resize a running terminal, clamping dimensions to the frozen bounds.
    /// `lease` fences the request against a stale renderer; `None` means "no
    /// renderer has ever attached", never "skip the check".
    ///
    /// # Errors
    ///
    /// Returns an error for a stale or missing lease, a non-running session, or
    /// PTY resize failure.
    pub fn resize(&self, lease: Option<u64>, rows: u16, columns: u16) -> Result<(), SessionError> {
        let (rows, columns) = clamp_dimensions(rows, columns);
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match lease {
            Some(lease) if !inner.attached || inner.lease != lease => {
                return Err(SessionError::new("terminal resize lease is stale"));
            }
            None if inner.lease != 0 => {
                return Err(SessionError::new("terminal resize requires a lease"));
            }
            _ => {}
        }
        if inner.state != SessionState::Running {
            return Err(SessionError::new(format!(
                "resize terminal session in state {:?}",
                inner.state.to_string()
            )));
        }
        if (inner.rows, inner.columns) == (rows, columns) {
            return Ok(());
        }
        inner
            .process
            .as_ref()
            .ok_or_else(|| SessionError::new("running terminal session has no PTY"))?
            .resize(rows, columns)
            .map_err(SessionError::from)?;
        inner.rows = rows;
        inner.columns = columns;
        Ok(())
    }

    /// Fully write input from the exact lease holder, retrying short PTY writes.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale lease, a non-running session, PTY failure,
    /// or zero progress.
    pub fn write_input(&self, lease: u64, mut input: &[u8]) -> Result<(), SessionError> {
        while !input.is_empty() {
            // Re-fenced before every write: a PTY write can stall for as long as
            // the reader is slow, and bytes from a lease released meanwhile must
            // not land in a PTY another renderer now owns.
            let process = self.live_process(lease)?;
            let written = process.write(input).map_err(SessionError::from)?;
            if written == 0 {
                return Err(SessionError::new(
                    io::Error::from(io::ErrorKind::WriteZero).to_string(),
                ));
            }
            input = &input[written..];
        }
        Ok(())
    }

    fn live_process(&self, lease: u64) -> Result<Arc<dyn PtyProcess>, SessionError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !inner.attached || inner.lease != lease {
            return Err(SessionError::new("terminal input lease is stale"));
        }
        if inner.state != SessionState::Running {
            return Err(SessionError::new(format!(
                "write terminal input in state {:?}",
                inner.state.to_string()
            )));
        }
        inner.last_activity_at = now_string();
        Ok(Arc::clone(inner.process.as_ref().ok_or_else(|| {
            SessionError::new("running terminal session has no PTY")
        })?))
    }

    /// Store a new application-validated pointer at the next revision.
    ///
    /// # Errors
    ///
    /// Returns stale/not-live errors.
    pub fn associate(
        &self,
        pointer: TerminalAssociationPointer,
    ) -> Result<TerminalAssociation, SessionError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.state != SessionState::Running {
            return Err(SessionError::new("terminal session is not live"));
        }
        let revision = inner
            .association
            .as_ref()
            .map_or(1, |association| association.revision.saturating_add(1));
        if revision == 0 {
            return Err(SessionError::new("stale association"));
        }
        let association = TerminalAssociation { pointer, revision };
        inner.association = Some(association.clone());
        Ok(association)
    }

    /// Prepare a revision-fenced association replacement.
    ///
    /// # Errors
    ///
    /// Returns stale when the expected revision is not current.
    pub fn prepare_association_change(
        &self,
        pointer: TerminalAssociationPointer,
        expected_revision: u64,
    ) -> Result<TerminalAssociationChange, SessionError> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.state != SessionState::Running
            || inner.association.as_ref().map_or(0, |value| value.revision) != expected_revision
        {
            return Err(SessionError::new("stale association"));
        }
        Ok(TerminalAssociationChange {
            session_id: self.id().to_owned(),
            previous: inner.association.clone(),
            next: TerminalAssociation {
                pointer,
                revision: expected_revision
                    .checked_add(1)
                    .ok_or_else(|| SessionError::new("stale association"))?,
            },
        })
    }

    /// Commit an exact prepared association replacement.
    ///
    /// # Errors
    ///
    /// Returns stale when session state changed since prepare.
    pub fn commit_association_change(
        &self,
        change: &TerminalAssociationChange,
    ) -> Result<(), SessionError> {
        self.replace_association(change.previous.as_ref(), Some(&change.next), true)
    }

    /// Roll back an exact committed association replacement.
    ///
    /// # Errors
    ///
    /// Returns stale when the committed value is no longer current.
    pub fn rollback_association_change(
        &self,
        change: &TerminalAssociationChange,
    ) -> Result<(), SessionError> {
        self.replace_association(Some(&change.next), change.previous.as_ref(), false)
    }

    fn replace_association(
        &self,
        expected: Option<&TerminalAssociation>,
        replacement: Option<&TerminalAssociation>,
        require_running: bool,
    ) -> Result<(), SessionError> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if (require_running && inner.state != SessionState::Running)
            || inner.association.as_ref() != expected
        {
            return Err(SessionError::new("stale association"));
        }
        inner.association = replacement.cloned();
        Ok(())
    }

    /// Run a callback while holding the live state/revision fence.
    ///
    /// # Errors
    ///
    /// Returns stale when the session exited or the revision changed.
    pub fn with_live_association<T>(
        &self,
        expected_revision: u64,
        use_association: impl FnOnce(&TerminalAssociation) -> T,
    ) -> Result<T, SessionError> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(association) = inner.association.as_ref() else {
            return Err(SessionError::new("stale association"));
        };
        if inner.state != SessionState::Running || association.revision != expected_revision {
            return Err(SessionError::new("stale association"));
        }
        Ok(use_association(association))
    }

    /// Close once, gracefully or forcibly, and join all owned workers.
    ///
    /// # Errors
    ///
    /// Returns joined process signalling/close errors.
    pub fn close(&self, force: bool) -> Result<(), SessionError> {
        let (process, was_exited) = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if inner.close_started {
                while !inner.close_done {
                    inner = self
                        .changed
                        .wait(inner)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                return inner.close_error.clone().map_or(Ok(()), Err);
            }
            inner.close_started = true;
            if matches!(inner.state, SessionState::Starting | SessionState::Failed)
                && inner.process.is_none()
            {
                inner.state = SessionState::Closed;
                inner.close_done = true;
                self.changed.notify_all();
                return Ok(());
            }
            let was_exited = inner.state == SessionState::Exited;
            if !was_exited {
                inner.state = SessionState::Closing;
            }
            self.closing.store(true, Ordering::Release);
            self.changed.notify_all();
            (inner.process.as_ref().map(Arc::clone), was_exited)
        };

        let mut errors = Vec::new();
        if let Some(process) = process.as_ref() {
            if force {
                if let Err(error) = process.kill() {
                    errors.push(format!("kill terminal process: {error}"));
                }
            } else if !was_exited {
                if let Err(error) = process.terminate() {
                    errors.push(format!("terminate terminal process: {error}"));
                }
                let inner = self
                    .inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let (inner, timed_out) = self
                    .changed
                    .wait_timeout_while(inner, self.options.graceful_timeout, |value| {
                        !value.exit_done
                    })
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if timed_out.timed_out() && !inner.exit_done {
                    drop(inner);
                    if let Err(error) = process.kill() {
                        errors.push(format!("kill terminal process after timeout: {error}"));
                    }
                }
            }
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while !inner.exit_done {
                inner = self
                    .changed
                    .wait(inner)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            drop(inner);
            if let Err(error) = self.close_process(&**process) {
                errors.push(format!("close terminal PTY: {error}"));
            }
        }
        self.changed.notify_all();
        let workers = std::mem::take(
            &mut *self
                .workers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for worker in workers {
            if worker.join().is_err() {
                errors.push("terminal worker panicked".to_owned());
            }
        }
        let error = (!errors.is_empty()).then(|| SessionError::new(errors.join("; ")));
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.state = SessionState::Closed;
        inner.close_error.clone_from(&error);
        inner.close_done = true;
        inner.live_sender.take();
        self.changed.notify_all();
        error.map_or(Ok(()), Err)
    }

    fn read_output(&self, process: &dyn PtyProcess) {
        let mut buffer = vec![0_u8; OUTPUT_READ_BYTES];
        loop {
            match process.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => self.deliver_output(buffer[..read].to_vec()),
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(error) => {
                    let mut inner = self
                        .inner
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    inner.stream_error = Some(format!("read terminal output: {error}"));
                    if inner.state == SessionState::Running {
                        inner.state = SessionState::Failed;
                    }
                    drop(inner);
                    let _ = process.kill();
                    break;
                }
            }
        }
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.output_done = true;
        inner.live_sender.take();
        self.changed.notify_all();
    }

    fn deliver_output(&self, output: Vec<u8>) {
        let sender = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.last_activity_at = now_string();
            inner.replay.append(&output);
            if !inner.attached {
                return;
            }
            inner.live_sender.clone()
        };
        if let Some(sender) = sender {
            let mut pending = output;
            loop {
                match sender.try_send(pending) {
                    Ok(()) | Err(tokio_mpsc::error::TrySendError::Closed(_)) => break,
                    Err(tokio_mpsc::error::TrySendError::Full(output)) => {
                        if self.closing.load(Ordering::Acquire) {
                            break;
                        }
                        pending = output;
                        thread::sleep(Duration::from_millis(2));
                    }
                }
            }
        }
    }

    fn wait_for_exit(&self, process: &dyn PtyProcess, reader_done: &mpsc::Receiver<()>) {
        let wait = process.wait();
        if reader_done
            .recv_timeout(self.options.output_drain_timeout)
            .is_err()
        {
            self.closing.store(true, Ordering::Release);
            self.changed.notify_all();
            let _ = self.close_process(process);
            let _ = reader_done.recv();
        }
        let (exit_code, mut error) = match wait {
            Ok(code) => (code, None),
            Err(wait_error) => (-1, Some(format!("wait for terminal process: {wait_error}"))),
        };
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(stream_error) = inner.stream_error.take() {
            error = Some(error.map_or(stream_error.clone(), |value| {
                format!("{value}; {stream_error}")
            }));
        }
        let result_state = if error.is_some() {
            SessionState::Failed
        } else {
            SessionState::Exited
        };
        if inner.state == SessionState::Running {
            inner.state = result_state;
        }
        inner.exit_done = true;
        self.changed.notify_all();
        drop(inner);
        let _ = self.close_process(process);
        if let Some(sender) = self
            .exit_sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = sender.send(ExitResult {
                exit_code,
                state: result_state,
                error,
            });
        }
    }

    fn close_process(&self, process: &dyn PtyProcess) -> io::Result<()> {
        let should_close = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if inner.process_closed {
                false
            } else {
                inner.process_closed = true;
                true
            }
        };
        if should_close {
            process.close()
        } else {
            Ok(())
        }
    }
}

impl StreamSession for Session {
    fn id(&self) -> &str {
        self.id()
    }

    fn attach_with_ticket(
        &self,
        presented: &str,
        from_sequence: u64,
    ) -> Result<StreamAttachment, StreamAttachRefusal> {
        self.attach_with_ticket(presented, from_sequence)
    }

    fn release_output(&self, lease: u64) {
        self.release_output(lease);
    }

    fn write_input(&self, lease: u64, input: &[u8]) -> Result<(), StreamSessionError> {
        self.write_input(lease, input)
            .map_err(|error| StreamSessionError(error.to_string()))
    }
}

fn ticket_matches(inner: &SessionInner, presented: &str) -> bool {
    !presented.is_empty()
        && inner
            .ticket
            .as_ref()
            .is_some_and(|ticket| presented.as_bytes().ct_eq(ticket.as_bytes()).unwrap_u8() == 1)
}

#[must_use]
pub fn clamp_dimensions(rows: u16, columns: u16) -> (u16, u16) {
    (
        rows.clamp(1, MAX_TERMINAL_ROWS),
        columns.clamp(1, MAX_TERMINAL_COLUMNS),
    )
}

fn now_string() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}
