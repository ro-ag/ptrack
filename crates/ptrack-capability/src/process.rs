use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::Mutex;
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

pub(crate) async fn run_process(
    spec: &ProcessSpec,
    cancellation: &CancellationToken,
) -> Result<ProcessResult, ProcessError> {
    let mut command = Command::new(&spec.name);
    command
        .args(&spec.args)
        .env_clear()
        .envs(safe_environment(&spec.env))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| ProcessError::Spawn)?;
    let stdout = child.stdout.take().ok_or(ProcessError::Spawn)?;
    let stderr = child.stderr.take().ok_or(ProcessError::Spawn)?;
    let maximum = usize::try_from(spec.max_output_bytes).unwrap_or(usize::MAX);
    let budget = Arc::new(Mutex::new(OutputBudget {
        remaining: maximum,
        truncated: false,
    }));
    let stdout_task = tokio::spawn(read_bounded(stdout, Arc::clone(&budget)));
    let stderr_task = tokio::spawn(read_bounded(stderr, Arc::clone(&budget)));
    let wait = async {
        let status = child.wait().await.map_err(|_| ProcessError::Wait)?;
        Ok(status.code().unwrap_or(-1))
    };
    let exit_code = tokio::select! {
        biased;
        () = cancellation.cancelled() => {
            kill_and_reap(&mut child).await;
            return Err(ProcessError::Cancelled);
        }
        result = tokio::time::timeout(spec.timeout, wait) => match result {
            Ok(value) => value?,
            Err(_) => {
                kill_and_reap(&mut child).await;
                return Err(ProcessError::Timeout);
            }
        }
    };
    let stdout = stdout_task.await.map_err(|_| ProcessError::Wait)??;
    let stderr = stderr_task.await.map_err(|_| ProcessError::Wait)??;
    let truncated = budget.lock().await.truncated;
    Ok(ProcessResult {
        exit_code,
        stdout,
        stderr,
        truncated,
    })
}

async fn kill_and_reap(child: &mut tokio::process::Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
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

fn safe_environment(extra: &[(OsString, OsString)]) -> HashMap<OsString, OsString> {
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
