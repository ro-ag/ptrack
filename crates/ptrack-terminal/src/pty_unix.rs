use std::fmt;
use std::io::{self, Read, Write};
use std::sync::Mutex;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use rustix::process::{Pid, Signal, kill_process_group};

use super::{PtyProcess, StartRequest};

pub(super) fn start(request: &StartRequest) -> io::Result<Box<dyn PtyProcess>> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: request.rows,
            cols: request.columns,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| io::Error::other(format!("create PTY: {error}")))?;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| io::Error::other(format!("clone PTY reader: {error}")))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| io::Error::other(format!("take PTY writer: {error}")))?;
    let mut command = CommandBuilder::new(&request.executable);
    command.args(&request.args);
    command.cwd(&request.cwd);
    command.env_clear();
    for entry in &request.env {
        let (key, value) = valid_environment_entry(entry)?;
        command.env(key, value);
    }
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| io::Error::other(format!("start PTY process: {error}")))?;
    let pid = child.process_id().unwrap_or(0);
    drop(pair.slave);
    Ok(Box::new(UnixPtyProcess {
        pid,
        child: Mutex::new(child),
        master: Mutex::new(Some(pair.master)),
        reader: Mutex::new(Some(reader)),
        writer: Mutex::new(Some(writer)),
    }))
}

fn valid_environment_entry(entry: &str) -> io::Result<(&str, &str)> {
    let (key, value) = entry.split_once('=').ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid PTY environment entry {entry:?}"),
        )
    })?;
    if key.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid PTY environment entry {entry:?}"),
        ));
    }
    Ok((key, value))
}

struct UnixPtyProcess {
    pid: u32,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    master: Mutex<Option<Box<dyn portable_pty::MasterPty + Send>>>,
    reader: Mutex<Option<Box<dyn Read + Send>>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
}

impl fmt::Debug for UnixPtyProcess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UnixPtyProcess")
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

impl PtyProcess for UnixPtyProcess {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut reader = self
            .reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = reader
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "PTY reader is closed"))?
            .read(buffer);
        normalize_read(result)
    }

    fn write(&self, buffer: &[u8]) -> io::Result<usize> {
        self.writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "PTY writer is closed"))?
            .write(buffer)
    }

    fn resize(&self, rows: u16, columns: u16) -> io::Result<()> {
        self.master
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "PTY is closed"))?
            .resize(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| io::Error::other(error.to_string()))
    }

    fn wait(&self) -> io::Result<i32> {
        self.child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .wait()
            .map(|status| i32::try_from(status.exit_code()).unwrap_or(i32::MAX))
    }

    fn terminate(&self) -> io::Result<()> {
        signal_process_group(self.pid, Signal::TERM)
    }
    fn kill(&self) -> io::Result<()> {
        signal_process_group(self.pid, Signal::KILL)
    }

    fn close(&self) -> io::Result<()> {
        let kill_error = self.kill().err();
        self.writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.master
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        kill_error.map_or(Ok(()), Err)
    }
}

fn signal_process_group(pid: u32, signal: Signal) -> io::Result<()> {
    let raw = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid PTY process ID"))?;
    match kill_process_group(raw, signal) {
        Ok(()) => Ok(()),
        Err(error) if error == rustix::io::Errno::SRCH => Ok(()),
        Err(error) => Err(io::Error::from_raw_os_error(error.raw_os_error())),
    }
}

pub(super) fn normalize_read(result: io::Result<usize>) -> io::Result<usize> {
    match result {
        Err(error) if error.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error()) => Ok(0),
        other => other,
    }
}
