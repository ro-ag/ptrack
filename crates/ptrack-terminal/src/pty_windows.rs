use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, TryLockError};
use std::time::Duration;

use conpty_oxide::blocking::{Child, Command, OwnedReadHalf, OwnedWriteHalf};
use conpty_oxide::{PtyController, SessionOptions, Size};

use super::{PtyProcess, StartRequest, split_windows_environment_entry};

#[cfg(test)]
mod pty_windows_test;

pub(super) fn start(request: &StartRequest) -> io::Result<Box<dyn PtyProcess>> {
    let mut command = Command::new(&request.executable);
    command.args(&request.args);
    command.current_dir(simplify_verbatim_cwd(&request.cwd));
    command.env_clear();
    for entry in &request.env {
        // A cmd.exe ancestor exports per-drive working-directory bookkeeping
        // (`=C:`, `=ExitCode`). Windows accepts no '=' in an environment name,
        // so forwarding one fails the whole spawn; drop them instead.
        if entry.starts_with('=') {
            continue;
        }
        let (key, value) = split_windows_environment_entry(entry)?;
        command.env(key, value);
    }
    let size = Size::try_new(request.columns, request.rows)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let session = command
        .spawn_with(SessionOptions::new().size(size))
        .map_err(|error| io::Error::other(format!("start ConPTY process: {error}")))?;
    let pid = session.id();
    let parts = session.into_parts();
    Ok(Box::new(WindowsPtyProcess {
        pid,
        child: Mutex::new(Some(parts.child)),
        output: Mutex::new(Some(parts.output)),
        input: Mutex::new(Some(parts.input)),
        controller: Mutex::new(Some(parts.controller)),
    }))
}

/// Rewrites a canonicalized verbatim working directory into its plain form.
///
/// `fs::canonicalize` produces `\\?\C:\...` paths on Windows. cmd.exe rejects
/// that form as a working directory ("UNC paths are not supported.") and
/// silently starts in the Windows directory instead, so hand the spawn a
/// plain drive path (`C:\...`) or classic UNC path (`\\server\share\...`).
fn simplify_verbatim_cwd(cwd: &Path) -> PathBuf {
    let Some(text) = cwd.to_str() else {
        return cwd.to_path_buf();
    };
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        let bytes = rest.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            return PathBuf::from(rest);
        }
    }
    cwd.to_path_buf()
}

struct WindowsPtyProcess {
    pid: u32,
    child: Mutex<Option<Child>>,
    output: Mutex<Option<OwnedReadHalf>>,
    input: Mutex<Option<OwnedWriteHalf>>,
    controller: Mutex<Option<PtyController>>,
}

impl fmt::Debug for WindowsPtyProcess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowsPtyProcess")
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

impl PtyProcess for WindowsPtyProcess {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        self.output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "ConPTY output is closed"))?
            .read(buffer)
    }

    fn write(&self, buffer: &[u8]) -> io::Result<usize> {
        self.input
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "ConPTY input is closed"))?
            .write(buffer)
    }

    fn resize(&self, rows: u16, columns: u16) -> io::Result<()> {
        let size = Size::try_new(columns, rows)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        self.controller
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "ConPTY is closed"))?
            .resize(size)
            .map_err(|error| io::Error::other(error.to_string()))
    }

    fn wait(&self) -> io::Result<i32> {
        loop {
            let status = self
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_mut()
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotConnected, "ConPTY child is closed")
                })?
                .try_wait()
                .map_err(|error| io::Error::other(error.to_string()))?;
            if let Some(status) = status {
                return Ok(i32::try_from(status.code()).unwrap_or(i32::MAX));
            }
            // Never hold the child mutex across the blocking wait: close and
            // graceful-timeout paths must be able to terminate the Job tree.
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn terminate(&self) -> io::Result<()> {
        // ConPTY has no stdin half-close. Retiring input requests console
        // teardown, which delivers the graceful close event to clients.
        match self.input.try_lock() {
            Ok(mut input) => {
                input.take();
            }
            Err(TryLockError::Poisoned(error)) => {
                error.into_inner().take();
            }
            Err(TryLockError::WouldBlock) => {
                // A stalled writer owns the pipe. The session's bounded
                // graceful timeout will fall back to the independently
                // synchronized Job-tree kill path.
            }
        }
        Ok(())
    }

    fn kill(&self) -> io::Result<()> {
        self.child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "ConPTY child is closed"))?
            .kill()
            .map_err(|error| io::Error::other(error.to_string()))
    }

    fn close(&self) -> io::Result<()> {
        let kill_error = self.kill().err();
        // Child owns the kill-on-close Job and is dropped first. The backend
        // then retires input/controller; output reaches EOF for its reader.
        self.child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.input
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.controller
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        kill_error.map_or(Ok(()), Err)
    }
}
