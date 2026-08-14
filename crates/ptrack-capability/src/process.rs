use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub(crate) struct ProcessSpec {
    pub name: OsString,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub max_output_bytes: u64,
    pub timeout: Duration,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProcessResult {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessError {
    Cancelled,
    Spawn,
    Timeout,
    Wait,
}

impl fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "process cancelled",
            Self::Spawn => "process could not be started",
            Self::Timeout => "process timed out",
            Self::Wait => "process failed",
        })
    }
}

impl std::error::Error for ProcessError {}

#[derive(Default)]
struct OutputBudget {
    remaining: usize,
    truncated: bool,
}

const MAX_CONCURRENT_PROCESS_READERS: usize = 16;

fn reader_permits() -> &'static Arc<Semaphore> {
    static PERMITS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    PERMITS.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_PROCESS_READERS)))
}

pub(crate) async fn run_process(
    spec: &ProcessSpec,
    cancellation: &CancellationToken,
) -> Result<ProcessResult, ProcessError> {
    let deadline = Instant::now() + spec.timeout;
    let _reader_permit = acquire_reader_permit(cancellation, deadline).await?;
    let mut command = Command::new(&spec.name);
    command
        .args(&spec.args)
        .env_clear()
        .envs(safe_environment(&spec.env))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_process_tree(&mut command);
    let mut child = command.spawn().map_err(|_| ProcessError::Spawn)?;
    let Ok(process_tree) = ProcessTree::attach(&child) else {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err(ProcessError::Spawn);
    };
    let stdout = child.stdout.take().ok_or(ProcessError::Spawn)?;
    let stderr = child.stderr.take().ok_or(ProcessError::Spawn)?;
    let maximum = usize::try_from(spec.max_output_bytes).unwrap_or(usize::MAX);
    let budget = Arc::new(Mutex::new(OutputBudget {
        remaining: maximum,
        truncated: false,
    }));
    let mut stdout_task = tokio::spawn(read_bounded(stdout, Arc::clone(&budget)));
    let mut stderr_task = tokio::spawn(read_bounded(stderr, Arc::clone(&budget)));
    let exit_code = tokio::select! {
        biased;
        () = cancellation.cancelled() => {
            terminate_and_cleanup(&mut child, &process_tree, &mut stdout_task, &mut stderr_task).await;
            return Err(ProcessError::Cancelled);
        }
        () = tokio::time::sleep_until(deadline) => {
            terminate_and_cleanup(&mut child, &process_tree, &mut stdout_task, &mut stderr_task).await;
            return Err(ProcessError::Timeout);
        }
        result = child.wait() => {
            let exit_code = result
                .map_err(|_| ProcessError::Wait)?
                .code()
                .unwrap_or(-1);
            // A bounded operation never permits daemonized descendants to
            // outlive their direct parent or keep inherited pipes open.
            process_tree.terminate();
            exit_code
        },
    };
    let stdout = await_reader(
        &mut stdout_task,
        cancellation,
        deadline,
        &mut child,
        &process_tree,
        &mut stderr_task,
    )
    .await?;
    let stderr = await_reader(
        &mut stderr_task,
        cancellation,
        deadline,
        &mut child,
        &process_tree,
        &mut stdout_task,
    )
    .await?;
    let truncated = budget.lock().await.truncated;
    Ok(ProcessResult {
        exit_code,
        stdout,
        stderr,
        truncated,
    })
}

async fn acquire_reader_permit(
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<OwnedSemaphorePermit, ProcessError> {
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(ProcessError::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(ProcessError::Timeout),
        permit = Arc::clone(reader_permits()).acquire_owned() => permit.map_err(|_| ProcessError::Spawn),
    }
}

async fn await_reader(
    reader: &mut JoinHandle<Result<Vec<u8>, ProcessError>>,
    cancellation: &CancellationToken,
    deadline: Instant,
    child: &mut tokio::process::Child,
    process_tree: &ProcessTree,
    sibling: &mut JoinHandle<Result<Vec<u8>, ProcessError>>,
) -> Result<Vec<u8>, ProcessError> {
    tokio::select! {
        biased;
        () = cancellation.cancelled() => {
            terminate_and_cleanup(child, process_tree, reader, sibling).await;
            Err(ProcessError::Cancelled)
        }
        () = tokio::time::sleep_until(deadline) => {
            terminate_and_cleanup(child, process_tree, reader, sibling).await;
            Err(ProcessError::Timeout)
        }
        result = &mut *reader => result.map_err(|_| ProcessError::Wait)?,
    }
}

async fn terminate_and_cleanup(
    child: &mut tokio::process::Child,
    process_tree: &ProcessTree,
    stdout_task: &mut JoinHandle<Result<Vec<u8>, ProcessError>>,
    stderr_task: &mut JoinHandle<Result<Vec<u8>, ProcessError>>,
) {
    process_tree.terminate();
    let _ = child.start_kill();
    stdout_task.abort();
    stderr_task.abort();
    let cleanup_deadline = Instant::now() + Duration::from_millis(100);
    let _ = tokio::time::timeout_at(cleanup_deadline, async {
        let _ = child.wait().await;
        let _ = stdout_task.await;
        let _ = stderr_task.await;
    })
    .await;
}

struct ProcessTree {
    #[cfg(not(windows))]
    process_id: Option<u32>,
    #[cfg(windows)]
    job: crate::private_windows::ProcessJob,
}

impl ProcessTree {
    #[allow(
        clippy::unnecessary_wraps,
        reason = "Windows job attachment is fallible"
    )]
    fn attach(child: &tokio::process::Child) -> Result<Self, ()> {
        #[cfg(windows)]
        let job = crate::private_windows::ProcessJob::attach(child)?;
        Ok(Self {
            #[cfg(not(windows))]
            process_id: child.id(),
            #[cfg(windows)]
            job,
        })
    }

    fn terminate(&self) {
        #[cfg(unix)]
        if let Some(pid) = self
            .process_id
            .and_then(|raw| i32::try_from(raw).ok())
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        #[cfg(windows)]
        self.job.terminate();
        #[cfg(not(any(unix, windows)))]
        let _ = self.process_id;
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(unix)]
fn configure_process_tree(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_tree(command: &mut Command) {
    #[cfg(windows)]
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    #[cfg(not(windows))]
    let _ = command;
}

async fn read_bounded<R>(
    mut reader: R,
    budget: Arc<Mutex<OutputBudget>>,
) -> Result<Vec<u8>, ProcessError>
where
    R: AsyncRead + Unpin,
{
    let mut collected = Vec::new();
    let mut chunk = [0_u8; 8_192];
    loop {
        let count = reader
            .read(&mut chunk)
            .await
            .map_err(|_| ProcessError::Wait)?;
        if count == 0 {
            return Ok(collected);
        }
        let take = {
            let mut budget = budget.lock().await;
            let take = count.min(budget.remaining);
            budget.remaining -= take;
            if take < count {
                budget.truncated = true;
            }
            take
        };
        collected.extend_from_slice(&chunk[..take]);
    }
}

pub(crate) fn safe_environment(extra: &[(OsString, OsString)]) -> HashMap<OsString, OsString> {
    let mut env: HashMap<OsString, OsString> = std::env::vars_os()
        .filter(|(name, _)| {
            !name
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
        })
        .collect();
    env.extend(extra.iter().cloned());
    env
}
