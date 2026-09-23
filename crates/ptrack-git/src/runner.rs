use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const MAX_READER_THREADS: usize = 64;
/// How long a terminated command's pipe readers get to observe end-of-file
/// before the runner stops waiting for them.
const READER_JOIN_GRACE: Duration = Duration::from_millis(250);

/// Diff options that keep repository-configured external diff drivers and
/// text conversion filters from running. Pass them to any command that can
/// render a diff (`show`, `log -p`, `diff`).
pub const NO_EXTERNAL_DIFF_ARGS: [&str; 2] = ["--no-ext-diff", "--no-textconv"];

static ACTIVE_READER_THREADS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// All returned errors are content-free: they contain no repository paths,
/// arguments, output, environment values, or credentials.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RepositoryError {
    Cancelled,
    CommandTimeout,
    OutputLimit,
    CommandFailed,
    AggregateLimit,
    InvalidData(&'static str),
    Filesystem(&'static str),
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("git command cancelled"),
            Self::CommandTimeout => formatter.write_str("git command timed out"),
            Self::OutputLimit => formatter.write_str("git command output limit exceeded"),
            Self::CommandFailed => formatter.write_str("git command failed"),
            Self::AggregateLimit => formatter.write_str("git snapshot aggregate limit exceeded"),
            Self::InvalidData(message) | Self::Filesystem(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for RepositoryError {}

pub(crate) trait Runner: Send + Sync {
    fn output(
        &self,
        cancellation: &CancellationToken,
        root: &Path,
        args: &[OsString],
    ) -> Result<Vec<u8>, RepositoryError>;
}

#[derive(Clone, Debug)]
pub(crate) struct ExecRunner {
    git_path: OsString,
    timeout: Duration,
    max_output_bytes: usize,
    reader_counter: &'static AtomicUsize,
    reader_limit: usize,
}

impl Default for ExecRunner {
    fn default() -> Self {
        Self {
            git_path: OsString::from("git"),
            timeout: DEFAULT_COMMAND_TIMEOUT,
            max_output_bytes: DEFAULT_OUTPUT_LIMIT,
            reader_counter: &ACTIVE_READER_THREADS,
            reader_limit: MAX_READER_THREADS,
        }
    }
}

impl ExecRunner {
    /// Constructs a runner with explicit bounds, for the tracked-path scan
    /// whose listing is far larger and slower than any snapshot command.
    pub(crate) fn with_limits(timeout: Duration, max_output_bytes: usize) -> Self {
        Self {
            git_path: OsString::from("git"),
            timeout,
            max_output_bytes,
            reader_counter: &ACTIVE_READER_THREADS,
            reader_limit: MAX_READER_THREADS,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        git_path: impl Into<OsString>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Self {
        Self {
            git_path: git_path.into(),
            timeout: if timeout.is_zero() {
                DEFAULT_COMMAND_TIMEOUT
            } else {
                timeout
            },
            max_output_bytes: if max_output_bytes == 0 {
                DEFAULT_OUTPUT_LIMIT
            } else {
                max_output_bytes
            },
            reader_counter: &ACTIVE_READER_THREADS,
            reader_limit: MAX_READER_THREADS,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_reader_counter_for_test(
        git_path: impl Into<OsString>,
        timeout: Duration,
        reader_counter: &'static AtomicUsize,
    ) -> Self {
        Self {
            git_path: git_path.into(),
            timeout,
            max_output_bytes: DEFAULT_OUTPUT_LIMIT,
            reader_counter,
            reader_limit: MAX_READER_THREADS,
        }
    }

    #[cfg(test)]
    pub(crate) fn without_reader_capacity_for_test(
        git_path: impl Into<OsString>,
        timeout: Duration,
    ) -> Self {
        static NO_READER_THREADS: AtomicUsize = AtomicUsize::new(0);
        Self {
            git_path: git_path.into(),
            timeout,
            max_output_bytes: DEFAULT_OUTPUT_LIMIT,
            reader_counter: &NO_READER_THREADS,
            reader_limit: 0,
        }
    }
}

impl Runner for ExecRunner {
    fn output(
        &self,
        cancellation: &CancellationToken,
        root: &Path,
        args: &[OsString],
    ) -> Result<Vec<u8>, RepositoryError> {
        if cancellation.is_cancelled() {
            return Err(RepositoryError::Cancelled);
        }

        let mut command = hardened_command(&self.git_path, root, args);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|_| RepositoryError::CommandFailed)?;
        let stdout = child.stdout.take().ok_or(RepositoryError::CommandFailed)?;
        let stderr = child.stderr.take().ok_or(RepositoryError::CommandFailed)?;
        let output = Arc::new(Mutex::new(CommandOutput::new(self.max_output_bytes)));
        let [stdout_permit, stderr_permit] =
            acquire_reader_permits(self.reader_counter, self.reader_limit)
                .ok_or_else(|| terminate_spawn_failure(&mut child))?;
        let stdout_reader = spawn_reader(stdout, Arc::clone(&output), true, stdout_permit)
            .map_err(|_| terminate_spawn_failure(&mut child))?;
        let Ok(stderr_reader) = spawn_reader(stderr, Arc::clone(&output), false, stderr_permit)
        else {
            let _ = kill_and_reap(&mut child);
            join_if_finished(stdout_reader);
            return Err(RepositoryError::CommandFailed);
        };
        let started = Instant::now();

        // The leader is reaped only after both pipes reach end-of-file, or
        // after its whole process group has been killed. While it is
        // unreaped its PID cannot be reused, so signalling the group can
        // never reach an unrelated process.
        let mut status = None;
        let termination = loop {
            if cancellation.is_cancelled() {
                reap_if_running(&mut child, &mut status);
                break Some(RepositoryError::Cancelled);
            }
            if output
                .lock()
                .expect("command output lock poisoned")
                .exceeded
            {
                reap_if_running(&mut child, &mut status);
                break Some(RepositoryError::OutputLimit);
            }
            if started.elapsed() >= self.timeout {
                reap_if_running(&mut child, &mut status);
                break Some(RepositoryError::CommandTimeout);
            }
            if stdout_reader.is_finished() && stderr_reader.is_finished() {
                match child.try_wait() {
                    Ok(Some(exit_status)) => {
                        status = Some(Ok(exit_status));
                        break None;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        reap_if_running(&mut child, &mut status);
                        break Some(RepositoryError::CommandFailed);
                    }
                }
            }
            thread::sleep(POLL_INTERVAL);
        };

        let [stdout_ok, stderr_ok] = join_readers([stdout_reader, stderr_reader]);
        let status = status
            .unwrap_or(Err(RepositoryError::CommandFailed))
            .map_err(|_| RepositoryError::CommandFailed)?;

        // Match the compatibility precedence after the process has been
        // killed and reaped and both pipes have been drained.
        if cancellation.is_cancelled() {
            return Err(RepositoryError::Cancelled);
        }
        if matches!(termination, Some(RepositoryError::CommandTimeout)) {
            return Err(RepositoryError::CommandTimeout);
        }
        let mut output = output.lock().expect("command output lock poisoned");
        if output.exceeded {
            return Err(RepositoryError::OutputLimit);
        }
        if termination.is_some() || !stdout_ok || !stderr_ok || !status.success() {
            return Err(RepositoryError::CommandFailed);
        }
        Ok(std::mem::take(&mut output.stdout))
    }
}

/// Kills the command and every descendant still in its process group, then
/// reaps the leader. Must run before the leader is reaped.
fn kill_and_reap(child: &mut Child) -> std::io::Result<ExitStatus> {
    kill_process_group(child);
    let _ = child.kill();
    child.wait()
}

#[cfg(unix)]
fn kill_process_group(child: &Child) {
    // The runner forbids unsafe code, so the group is signalled through the
    // system `kill` utility. The leader is unreaped here, which keeps its PID
    // (and so the group ID) from being reused.
    let group = format!("-{}", child.id());
    for program in ["/bin/kill", "/usr/bin/kill"] {
        let signalled = Command::new(program)
            .args(["-KILL", "--", &group])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if signalled.is_ok() {
            return;
        }
    }
}

#[cfg(not(unix))]
fn kill_process_group(_child: &Child) {}

fn reap_if_running(child: &mut Child, status: &mut Option<Result<ExitStatus, RepositoryError>>) {
    if status.is_none() {
        *status = Some(kill_and_reap(child).map_err(|_| RepositoryError::CommandFailed));
    }
}

fn terminate_spawn_failure(child: &mut Child) -> RepositoryError {
    let _ = kill_and_reap(child);
    RepositoryError::CommandFailed
}

fn join_if_finished(reader: thread::JoinHandle<bool>) -> bool {
    reader.is_finished() && reader.join().unwrap_or(false)
}

/// Joins both pipe readers once the command is gone. Killing the process
/// group closes every pipe a descendant inherited, so the readers normally
/// finish at once; the bounded grace covers only a descendant that left the
/// group, whose reader then keeps its slot until that pipe closes.
fn join_readers(readers: [thread::JoinHandle<bool>; 2]) -> [bool; 2] {
    let deadline = Instant::now() + READER_JOIN_GRACE;
    while readers.iter().any(|reader| !reader.is_finished()) && Instant::now() < deadline {
        thread::sleep(POLL_INTERVAL);
    }
    readers.map(join_if_finished)
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    output: Arc<Mutex<CommandOutput>>,
    stdout: bool,
    permit: ReaderPermit,
) -> std::io::Result<thread::JoinHandle<bool>> {
    thread::Builder::new()
        .name("ptrack-git-pipe".to_owned())
        .spawn(move || {
            let _permit = permit;
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => return true,
                    Ok(read) => output
                        .lock()
                        .expect("command output lock poisoned")
                        .write(&buffer[..read], stdout),
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => return false,
                }
            }
        })
}

#[derive(Debug)]
struct ReaderPermit {
    counter: &'static AtomicUsize,
}

impl Drop for ReaderPermit {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

fn acquire_reader_permits(
    counter: &'static AtomicUsize,
    limit: usize,
) -> Option<[ReaderPermit; 2]> {
    let mut active = counter.load(Ordering::Acquire);
    loop {
        if active.checked_add(2).is_none_or(|next| next > limit) {
            return None;
        }
        match counter.compare_exchange_weak(active, active + 2, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => return Some([ReaderPermit { counter }, ReaderPermit { counter }]),
            Err(observed) => active = observed,
        }
    }
}

#[derive(Debug)]
struct CommandOutput {
    remaining: usize,
    exceeded: bool,
    stdout: Vec<u8>,
}

impl CommandOutput {
    fn new(limit: usize) -> Self {
        Self {
            remaining: limit,
            exceeded: false,
            stdout: Vec::new(),
        }
    }

    fn write(&mut self, input: &[u8], stdout: bool) {
        let accepted = input.len().min(self.remaining);
        if stdout && accepted > 0 {
            self.stdout.extend_from_slice(&input[..accepted]);
        }
        self.remaining -= accepted;
        if accepted < input.len() {
            self.exceeded = true;
        }
    }
}

/// Builds a `git` command with the hardening every repository read uses:
/// `--no-optional-locks`, `-c core.fsmonitor=false`, `-C <root>`, a scrubbed
/// environment (every inherited `GIT_*` variable removed; fixed locale,
/// pager, prompt, and lazy-fetch values), a null stdin, and, on Unix, its
/// own process group so a caller can signal every descendant.
///
/// Callers choose stdout and stderr handling. Commands that can render a
/// diff should also pass [`NO_EXTERNAL_DIFF_ARGS`], and a caller-supplied
/// revision belongs after `--end-of-options`.
#[must_use]
pub fn hardened_git_command(root: &Path, args: &[OsString]) -> Command {
    hardened_command(OsStr::new("git"), root, args)
}

fn hardened_command(program: &OsStr, root: &Path, args: &[OsString]) -> Command {
    let mut command = Command::new(program);
    command
        .args(git_command_args(root, args))
        .env_clear()
        .envs(git_environment(std::env::vars_os()))
        .stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command
}

pub(crate) fn git_command_args(root: &Path, args: &[OsString]) -> Vec<OsString> {
    let mut command_args = Vec::with_capacity(args.len() + 5);
    command_args.push(OsString::from("--no-optional-locks"));
    command_args.push(OsString::from("-c"));
    command_args.push(OsString::from("core.fsmonitor=false"));
    command_args.push(OsString::from("-C"));
    command_args.push(root.as_os_str().to_owned());
    command_args.extend_from_slice(args);
    command_args
}

pub(crate) fn git_environment<I>(source: I) -> Vec<(OsString, OsString)>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    const OVERRIDES: [(&str, &str); 6] = [
        ("LANG", "C"),
        ("LC_ALL", "C"),
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_PAGER", "cat"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_NO_LAZY_FETCH", "1"),
    ];
    let mut environment = Vec::new();
    for (key, value) in source {
        let key_lossy = key.to_string_lossy();
        if starts_with_ascii_case_insensitive(&key_lossy, "GIT_")
            || OVERRIDES
                .iter()
                .any(|(fixed, _)| key_lossy.eq_ignore_ascii_case(fixed))
        {
            continue;
        }
        environment.push((key, value));
    }
    environment.extend(
        OVERRIDES
            .into_iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value))),
    );
    environment
}

fn starts_with_ascii_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

pub(crate) fn args<const N: usize>(values: [&str; N]) -> [OsString; N] {
    values.map(OsString::from)
}

pub(crate) fn os(value: impl AsRef<OsStr>) -> OsString {
    value.as_ref().to_owned()
}
