use std::io;
use std::path::PathBuf;

#[cfg(unix)]
#[path = "pty_unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "pty_windows.rs"]
mod platform;

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

pub trait PtyFactory: Send + Sync + 'static {
    /// Start one process attached to a native pseudo-terminal.
    ///
    /// # Errors
    ///
    /// Returns a contextual I/O error when allocation or process start fails.
    fn start(&self, request: StartRequest) -> io::Result<Box<dyn PtyProcess>>;
}

pub trait PtyProcess: Send + Sync + 'static {
    fn pid(&self) -> u32;
    /// Read terminal output.
    ///
    /// # Errors
    ///
    /// Returns an OS I/O error.
    fn read(&self, buffer: &mut [u8]) -> io::Result<usize>;
    /// Write terminal input.
    ///
    /// # Errors
    ///
    /// Returns an OS I/O error.
    fn write(&self, buffer: &[u8]) -> io::Result<usize>;
    /// Resize the terminal.
    ///
    /// # Errors
    ///
    /// Returns an OS PTY error.
    fn resize(&self, rows: u16, columns: u16) -> io::Result<()>;
    /// Wait for and report the process exit code.
    ///
    /// # Errors
    ///
    /// Returns an error only when no process status can be obtained.
    fn wait(&self) -> io::Result<i32>;
    /// Request graceful process-tree termination.
    ///
    /// # Errors
    ///
    /// Returns an OS signalling error.
    fn terminate(&self) -> io::Result<()>;
    /// Force process-tree termination.
    ///
    /// # Errors
    ///
    /// Returns an OS signalling error.
    fn kill(&self) -> io::Result<()>;
    /// Close PTY resources and process containment.
    ///
    /// # Errors
    ///
    /// Returns an OS teardown error.
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
    let separator = if let Some(rest) = entry.strip_prefix('=') {
        rest.find('=').map(|offset| offset + 1)
    } else {
        entry.find('=')
    }
    .ok_or_else(|| invalid_environment_entry(entry))?;
    let key = &entry[..separator];
    if key.is_empty() {
        return Err(invalid_environment_entry(entry));
    }
    Ok((key, &entry[separator + 1..]))
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
