use std::io;
use std::path::PathBuf;

#[cfg(unix)]
#[path = "pty_unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "pty_windows.rs"]
mod platform;
#[cfg(all(unix, test))]
pub(crate) use platform::parse_stat_session;

/// Independently owned, already validated launch parameters passed to a PTY.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartRequest {
    pub executable: String,
    pub args: Vec<String>,
    pub env: Vec<String>,
    pub cwd: PathBuf,
    pub rows: u16,
    pub columns: u16,
}

// Every method here returns `io::Result`; the OS error is the whole story.
#[allow(clippy::missing_errors_doc)]
pub trait PtyFactory: Send + Sync + 'static {
    /// Start one process attached to a native pseudo-terminal.
    fn start(&self, request: StartRequest) -> io::Result<Box<dyn PtyProcess>>;
}

#[allow(clippy::missing_errors_doc)]
pub trait PtyProcess: Send + Sync + 'static {
    fn pid(&self) -> u32;
    /// Read terminal output.
    fn read(&self, buffer: &mut [u8]) -> io::Result<usize>;
    /// Write terminal input.
    fn write(&self, buffer: &[u8]) -> io::Result<usize>;
    /// Resize the terminal.
    fn resize(&self, rows: u16, columns: u16) -> io::Result<()>;
    /// Wait for and report the process exit code. Errors only when no process
    /// status can be obtained.
    fn wait(&self) -> io::Result<i32>;
    /// Request graceful process-tree termination. It must never signal a
    /// leader that `wait` already reaped: its pid may belong to someone else.
    fn terminate(&self) -> io::Result<()>;
    /// Force process-tree termination.
    fn kill(&self) -> io::Result<()>;
    /// Whether any process of the terminal's tree may still be running,
    /// including background jobs that outlived the leader. An implementation
    /// that cannot tell reports `false` and relies on `close` for containment.
    fn live_processes(&self) -> bool {
        false
    }
    /// Close PTY resources and process containment.
    fn close(&self) -> io::Result<()>;
}

/// Factory backed by the operating system PTY implementation.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativePtyFactory;

impl PtyFactory for NativePtyFactory {
    fn start(&self, request: StartRequest) -> io::Result<Box<dyn PtyProcess>> {
        platform::start(&request)
    }
}

#[cfg(any(test, windows))]
pub(crate) fn split_windows_environment_entry(entry: &str) -> io::Result<(&str, &str)> {
    entry
        .split_once('=')
        .filter(|(key, _)| !key.is_empty())
        .ok_or_else(|| invalid_environment_entry(entry))
}

#[cfg(any(test, windows))]
fn invalid_environment_entry(entry: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("invalid PTY environment entry {entry:?}"),
    )
}

#[cfg(all(test, unix))]
pub(crate) fn normalize_pty_read(result: io::Result<usize>) -> io::Result<usize> {
    platform::normalize_read(result)
}
