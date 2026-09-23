use std::fmt;
use std::io::{self, Write};
use std::os::fd::{BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use rustix::event::{PollFd, PollFlags, poll};
use rustix::process::{
    Pid, Signal, WaitId, WaitIdOptions, kill_process, kill_process_group, waitid,
};

use super::{PtyProcess, StartRequest};

/// A kill sweep repeats while members remain, so a job loop that forks between
/// enumeration and signal is still caught; a bounded count keeps it finite.
const KILL_SWEEP_PASSES: usize = 3;

pub(super) fn start(request: &StartRequest) -> io::Result<Box<dyn PtyProcess>> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: request.rows,
            cols: request.columns,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| io::Error::other(format!("create PTY: {error}")))?;
    let reader = duplicate_master(&*pair.master)?;
    let (wake_receiver, wake_sender) = UnixStream::pair()
        .map_err(|error| io::Error::other(format!("create PTY reader wake: {error}")))?;
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
        reaped: Mutex::new(false),
        master: Mutex::new(Some(pair.master)),
        reader: Mutex::new(reader),
        wake_receiver,
        wake_sender: Mutex::new(Some(wake_sender)),
        read_cancelled: AtomicBool::new(false),
        writer: Mutex::new(Some(writer)),
    }))
}

/// The reader owns its own descriptor for the master so it can wait on it
/// alongside the wake socket: a cloned reader from the PTY crate is an opaque
/// blocking `Read` that nothing can interrupt while a background job still
/// holds the slave open.
#[allow(unsafe_code)]
fn duplicate_master(master: &dyn MasterPty) -> io::Result<OwnedFd> {
    let raw = master
        .as_raw_fd()
        .ok_or_else(|| io::Error::other("PTY master has no descriptor"))?;
    // SAFETY: `raw` is the descriptor `master` owns, and `master` is borrowed
    // for this whole call, so it stays open for the borrow. The duplicate is
    // an independent owned descriptor.
    let borrowed = unsafe { BorrowedFd::borrow_raw(raw) };
    rustix::io::fcntl_dupfd_cloexec(borrowed, 0)
        .map_err(|error| io::Error::other(format!("clone PTY reader: {error}")))
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
    /// Set once the leader is known to have exited, before it is reaped, and
    /// held while its process group is signalled: a reaped pid can be reused,
    /// so no signal may ever target the old group after this flips.
    reaped: Mutex<bool>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    reader: Mutex<OwnedFd>,
    wake_receiver: UnixStream,
    wake_sender: Mutex<Option<UnixStream>>,
    read_cancelled: AtomicBool,
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

impl UnixPtyProcess {
    fn leader(&self) -> io::Result<Pid> {
        i32::try_from(self.pid)
            .ok()
            .and_then(Pid::from_raw)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid PTY process ID"))
    }

    /// Signal the leader's group while it is still unreaped, then every other
    /// process of its session. Job-control shells put each background job in
    /// its own group, so the leader's group alone misses them.
    fn signal_session(&self, signal: Signal) -> io::Result<usize> {
        let leader = self.leader()?;
        let group_result = {
            let reaped = self
                .reaped
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *reaped {
                Ok(())
            } else {
                signal_process_group(leader, signal)
            }
        };
        let members = session_members(leader);
        for member in &members {
            // A member that exited meanwhile is not an error.
            let _ = kill_process(*member, signal);
        }
        group_result.map(|()| members.len())
    }

    fn cancel_read(&self) {
        self.read_cancelled.store(true, Ordering::Release);
        if let Some(mut sender) = self
            .wake_sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            // A byte wakes the reader even if a forked child inherited the
            // sender before close-on-exec applied; dropping it then also
            // leaves the receiver at end of stream.
            let _ = sender.write_all(&[1]);
        }
    }
}

impl PtyProcess for UnixPtyProcess {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        let reader = self
            .reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if self.read_cancelled.load(Ordering::Acquire) {
                return Ok(0);
            }
            let mut descriptors = [
                PollFd::new(&*reader, PollFlags::IN),
                PollFd::new(&self.wake_receiver, PollFlags::IN),
            ];
            match poll(&mut descriptors, None) {
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(io::Error::from(error)),
            }
            if !descriptors[1].revents().is_empty() {
                return Ok(0);
            }
            let ready = descriptors[0].revents();
            if ready.contains(PollFlags::NVAL) {
                return Err(io::Error::other("PTY reader descriptor is invalid"));
            }
            if ready.intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR) {
                let result = rustix::io::read(&*reader, &mut *buffer).map_err(io::Error::from);
                return normalize_read(result);
            }
        }
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
        if let Ok(leader) = self.leader() {
            // Observe the exit without reaping, so the pid stays owned by the
            // zombie until the fence below is raised. Any failure falls back
            // to the plain wait, which still reaps and reports the status.
            while let Err(rustix::io::Errno::INTR) = waitid(
                WaitId::Pid(leader),
                WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
            ) {}
        }
        *self
            .reaped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .wait()
            .map(|status| i32::try_from(status.exit_code()).unwrap_or(i32::MAX))
    }

    fn terminate(&self) -> io::Result<()> {
        self.signal_session(Signal::TERM).map(|_| ())
    }

    fn kill(&self) -> io::Result<()> {
        let mut result = self.signal_session(Signal::KILL);
        for _ in 1..KILL_SWEEP_PASSES {
            match result {
                Ok(0) | Err(_) => break,
                Ok(_) => result = self.signal_session(Signal::KILL),
            }
        }
        result.map(|_| ())
    }

    fn live_processes(&self) -> bool {
        let reaped = *self
            .reaped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !reaped
            || self
                .leader()
                .is_ok_and(|leader| !session_members(leader).is_empty())
    }

    fn close(&self) -> io::Result<()> {
        let kill_error = self.kill().err();
        self.cancel_read();
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

fn signal_process_group(group: Pid, signal: Signal) -> io::Result<()> {
    match kill_process_group(group, signal) {
        Ok(()) => Ok(()),
        Err(error) if error == rustix::io::Errno::SRCH => Ok(()),
        Err(error) => Err(io::Error::from_raw_os_error(error.raw_os_error())),
    }
}

/// Every live process whose session is the one `leader` created, other than
/// the leader itself. The leader's own pid is never signalled directly: only
/// the fenced group signal may target it.
pub(super) fn session_members(leader: Pid) -> Vec<Pid> {
    candidate_pids()
        .into_iter()
        .filter_map(Pid::from_raw)
        .filter(|pid| *pid != leader && session_of(*pid) == Some(leader.as_raw_nonzero().get()))
        .collect()
}

/// The session id of `pid`, read from procfs. Kernel threads report session 0,
/// which rustix's `getsid` cannot represent (it asserts a positive pid), so
/// Linux never asks it.
#[cfg(target_os = "linux")]
fn session_of(pid: Pid) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid.as_raw_nonzero())).ok()?;
    parse_stat_session(&stat)
}

/// Field 6 of `/proc/<pid>/stat`, counted after the parenthesised command
/// name, which may itself contain spaces or parentheses.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_stat_session(stat: &str) -> Option<i32> {
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(3)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn session_of(pid: Pid) -> Option<i32> {
    rustix::process::getsid(Some(pid))
        .ok()
        .map(|sid| sid.as_raw_nonzero().get())
}

#[cfg(target_os = "linux")]
fn candidate_pids() -> Vec<i32> {
    std::fs::read_dir("/proc")
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// There is no procfs here and no safe process-list call, so the system `ps`
/// names the candidates; each one is still confirmed with `getsid` before it
/// is signalled.
#[cfg(not(target_os = "linux"))]
fn candidate_pids() -> Vec<i32> {
    std::process::Command::new("/bin/ps")
        .args(["-A", "-o", "pid="])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn normalize_read(result: io::Result<usize>) -> io::Result<usize> {
    match result {
        Err(error) if error.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error()) => Ok(0),
        other => other,
    }
}
