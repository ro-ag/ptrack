use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::profile::ProfileKind;
use crate::pty::{PtyFactory, PtyProcess, StartRequest};
use crate::session::{
    Session, SessionMetadata, SessionOptions, SessionState, TerminalAssociationPointer,
};
use crate::shell_integration::ShellIntegrationDescriptor;
use crate::stream::StreamAttachRefusal;

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
struct FakeState {
    output: VecDeque<u8>,
    eof: bool,
    exit: Option<(i32, Option<io::ErrorKind>)>,
    writes: Vec<u8>,
    max_write: usize,
    resizes: Vec<(u16, u16)>,
    block_write: bool,
    write_started: bool,
    terminate_calls: usize,
    kill_calls: usize,
    close_calls: usize,
    /// A background job keeps the slave open, so the leader's exit is not end
    /// of file: output ends only when the PTY is closed, as on a real PTY.
    background_job: bool,
    /// The reader cannot be cancelled at all, even by closing the PTY.
    stuck_reader: bool,
    /// The leader ignores SIGTERM and only a kill ends it.
    ignore_terminate: bool,
    background_killed: bool,
    reaped: bool,
    signals_after_reap: usize,
}

#[derive(Default)]
struct FakeProcess {
    state: Mutex<FakeState>,
    changed: Condvar,
}

impl FakeProcess {
    fn output(&self, bytes: &[u8]) {
        let mut state = self.state.lock().unwrap();
        state.output.extend(bytes);
        self.changed.notify_all();
    }

    fn await_write_start(&self) {
        let mut state = self.state.lock().unwrap();
        while !state.write_started {
            state = self.changed.wait(state).unwrap();
        }
    }

    fn unblock_writes(&self) {
        self.state.lock().unwrap().block_write = false;
        self.changed.notify_all();
    }

    fn exit(&self, code: i32) {
        let mut state = self.state.lock().unwrap();
        state.exit = Some((code, None));
        state.eof = !state.background_job && !state.stuck_reader;
        self.changed.notify_all();
    }

    fn configure(&self, change: impl FnOnce(&mut FakeState)) {
        change(&mut self.state.lock().unwrap());
    }
}

impl PtyProcess for FakeProcess {
    fn pid(&self) -> u32 {
        77
    }

    fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut state = self.state.lock().unwrap();
        while state.output.is_empty() && !state.eof {
            state = self.changed.wait(state).unwrap();
        }
        if state.output.is_empty() {
            return Ok(0);
        }
        let count = buffer.len().min(state.output.len());
        for slot in &mut buffer[..count] {
            *slot = state.output.pop_front().unwrap();
        }
        Ok(count)
    }

    fn write(&self, buffer: &[u8]) -> io::Result<usize> {
        let mut state = self.state.lock().unwrap();
        state.write_started = true;
        self.changed.notify_all();
        while state.block_write {
            state = self.changed.wait(state).unwrap();
        }
        let count = if state.max_write == 0 {
            buffer.len()
        } else {
            state.max_write.min(buffer.len())
        };
        state.writes.extend_from_slice(&buffer[..count]);
        Ok(count)
    }

    fn resize(&self, rows: u16, columns: u16) -> io::Result<()> {
        self.state.lock().unwrap().resizes.push((rows, columns));
        Ok(())
    }

    fn wait(&self) -> io::Result<i32> {
        let mut state = self.state.lock().unwrap();
        while state.exit.is_none() {
            state = self.changed.wait(state).unwrap();
        }
        let (code, error) = state.exit.unwrap();
        state.reaped = true;
        error.map_or(Ok(code), |kind| Err(io::Error::from(kind)))
    }

    fn terminate(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.terminate_calls += 1;
        if state.reaped {
            state.signals_after_reap += 1;
        }
        if !state.ignore_terminate && state.exit.is_none() {
            state.exit = Some((0, None));
            state.eof = !state.background_job && !state.stuck_reader;
        }
        self.changed.notify_all();
        Ok(())
    }

    fn kill(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.kill_calls += 1;
        if state.reaped {
            state.signals_after_reap += 1;
        }
        if state.exit.is_none() {
            state.exit = Some((1, None));
        }
        // Killing the tree takes the background job with it.
        state.background_killed = true;
        state.eof = !state.stuck_reader;
        self.changed.notify_all();
        Ok(())
    }

    fn close(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.close_calls += 1;
        state.eof = !state.stuck_reader;
        self.changed.notify_all();
        Ok(())
    }
}

struct FakeFactory {
    process: Arc<FakeProcess>,
    starts: Mutex<Vec<StartRequest>>,
}

impl PtyFactory for FakeFactory {
    fn start(&self, request: StartRequest) -> io::Result<Box<dyn PtyProcess>> {
        self.starts.lock().unwrap().push(request);
        Ok(Box::new(SharedFake(Arc::clone(&self.process))))
    }
}

struct SharedFake(Arc<FakeProcess>);

impl PtyProcess for SharedFake {
    fn pid(&self) -> u32 {
        self.0.pid()
    }
    fn read(&self, value: &mut [u8]) -> io::Result<usize> {
        self.0.read(value)
    }
    fn write(&self, value: &[u8]) -> io::Result<usize> {
        self.0.write(value)
    }
    fn resize(&self, rows: u16, columns: u16) -> io::Result<()> {
        self.0.resize(rows, columns)
    }
    fn wait(&self) -> io::Result<i32> {
        self.0.wait()
    }
    fn terminate(&self) -> io::Result<()> {
        self.0.terminate()
    }
    fn kill(&self) -> io::Result<()> {
        self.0.kill()
    }
    fn live_processes(&self) -> bool {
        let state = self.0.state.lock().unwrap();
        state.exit.is_none() || (state.background_job && !state.background_killed)
    }
    fn close(&self) -> io::Result<()> {
        self.0.close()
    }
}

fn harness(options: SessionOptions) -> (Arc<Session>, Arc<FakeProcess>, Arc<FakeFactory>) {
    let process = Arc::new(FakeProcess::default());
    let factory = Arc::new(FakeFactory {
        process: Arc::clone(&process),
        starts: Mutex::new(Vec::new()),
    });
    let session = Session::new_with_options(
        StartRequest {
            executable: "/bin/test".to_owned(),
            args: vec!["--owned".to_owned()],
            env: vec!["TERM=xterm-256color".to_owned()],
            cwd: "/tmp".into(),
            rows: 0,
            columns: u16::MAX,
        },
        SessionMetadata {
            id: "session".to_owned(),
            profile_id: "shell".to_owned(),
            profile_kind: ProfileKind::Shell,
            provider: String::new(),
            cwd: "/tmp".to_owned(),
            shell_integration: ShellIntegrationDescriptor::none(),
        },
        factory.clone(),
        options,
    );
    (session, process, factory)
}

fn wait_for_output(session: &Session, sequence: u64) {
    for _ in 0..200 {
        if session.replay_bounds().1 >= sequence {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for terminal output");
}

#[test]
fn session_state_values_are_frozen() {
    assert_eq!(SessionState::Starting.to_string(), "starting");
    assert_eq!(SessionState::Running.to_string(), "running");
    assert_eq!(SessionState::Exited.to_string(), "exited");
    assert_eq!(SessionState::Closing.to_string(), "closing");
    assert_eq!(SessionState::Closed.to_string(), "closed");
    assert_eq!(SessionState::Failed.to_string(), "failed");
}

#[test]
fn lifecycle_clamps_launch_retries_writes_and_delivers_one_exit() {
    let (session, process, factory) = harness(SessionOptions::default());
    process.state.lock().unwrap().max_write = 2;
    let exits = session.take_exit_results().unwrap();
    session.start().unwrap();
    assert_eq!(session.state(), SessionState::Running);
    let launch = factory.starts.lock().unwrap()[0].clone();
    assert_eq!((launch.rows, launch.columns), (1, 1_000));

    // An unfenced resize is admitted only before any renderer has attached.
    session.resize(None, 0, u16::MAX).unwrap();
    let attachment = session.attach_output(0).unwrap();
    assert!(session.resize(None, 30, 100).is_err());
    session.write_input(attachment.lease, b"abcdef").unwrap();
    assert_eq!(process.state.lock().unwrap().writes, b"abcdef");
    session.resize(Some(attachment.lease), 0, u16::MAX).unwrap();
    assert_eq!(
        process.state.lock().unwrap().resizes,
        Vec::<(u16, u16)>::new()
    );
    session.resize(Some(attachment.lease), 30, 100).unwrap();
    assert_eq!(process.state.lock().unwrap().resizes, vec![(30, 100)]);

    process.exit(23);
    let result = exits.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(result.exit_code, 23);
    assert_eq!(result.state, SessionState::Exited);
    assert!(result.error.is_none());
    assert!(exits.recv_timeout(Duration::from_millis(50)).is_err());
    session.close(false).unwrap();
    session.close(true).unwrap();
    assert_eq!(session.state(), SessionState::Closed);
    assert_eq!(process.state.lock().unwrap().close_calls, 1);
}

#[test]
fn replay_window_is_bounded_sequenced_and_resumed_before_live_output() {
    let options = SessionOptions {
        replay_buffer_bytes: 8,
        ..SessionOptions::default()
    };
    let (session, process, _) = harness(options);
    session.start().unwrap();
    process.output(b"0123456789abcdef");
    wait_for_output(&session, 16);
    // The PTY is never stalled by an unattached renderer: the oldest bytes are
    // dropped instead, and the dropped prefix can no longer be replayed.
    assert_eq!(session.replay_bounds(), (8, 16));
    // A wrapped buffer resumes from the oldest retained byte and reports the
    // gap: refusing here would make a re-attach impossible to complete, which
    // §4 forbids. Claiming output the PTY never produced is still refused.
    let wrapped = session.attach_output(0).unwrap();
    assert!(wrapped.gap);
    assert_eq!(wrapped.replay, b"89abcdef");
    assert!(session.release_output(wrapped.lease));
    assert!(session.attach_output(17).is_err());

    let mut attachment = session.attach_output(8).unwrap();
    assert!(!attachment.gap);
    assert_eq!(attachment.replay, b"89abcdef");
    process.output(b"live");
    assert_eq!(attachment.live.blocking_recv().unwrap(), b"live");
    assert_eq!(session.replay_bounds(), (12, 20));
    session.close(true).unwrap();
}

#[test]
fn releasing_a_lease_keeps_the_session_and_refuses_a_second_renderer() {
    let (session, process, _) = harness(SessionOptions::default());
    session.start().unwrap();
    let attachment = session.attach_output(0).unwrap();
    assert_eq!(attachment.lease, 1);

    // A second renderer is refused without touching the session or its PTY.
    assert!(session.attach_output(0).is_err());
    assert_eq!(session.state(), SessionState::Running);
    session.write_input(attachment.lease, b"held").unwrap();

    assert!(session.release_output(attachment.lease));
    assert!(!session.release_output(attachment.lease));
    let calls = {
        let state = process.state.lock().unwrap();
        (state.terminate_calls, state.kill_calls, state.close_calls)
    };
    assert_eq!(calls, (0, 0, 0));
    assert_eq!(session.state(), SessionState::Running);

    // The released renderer keeps no authority over the live session.
    assert!(session.write_input(attachment.lease, b"stale").is_err());
    assert!(session.resize(Some(attachment.lease), 30, 100).is_err());

    let reclaimed = session.attach_output(0).unwrap();
    assert_eq!(reclaimed.lease, 3);
    session.write_input(reclaimed.lease, b"fresh").unwrap();
    session.resize(Some(reclaimed.lease), 30, 100).unwrap();
    assert_eq!(process.state.lock().unwrap().writes, b"heldfresh");
    session.close(true).unwrap();
}

#[tokio::test]
async fn attaching_after_the_shell_exited_replays_once_and_then_ends() {
    let (session, process, _) = harness(SessionOptions::default());
    session.start().unwrap();
    process.output(b"bye");
    wait_for_output(&session, 3);
    process.exit(0);
    for _ in 0..200 {
        if session.state() == SessionState::Exited {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(session.state(), SessionState::Exited);

    // The reader thread is gone, so the replay must be all there is: parking on
    // live output would pin the session and the grace monitor could never reap it.
    let mut attachment = session.attach_output(0).unwrap();
    assert_eq!(attachment.replay, b"bye");
    assert!(
        tokio::time::timeout(Duration::from_secs(1), attachment.live.recv())
            .await
            .expect("live output never ended for an exited session")
            .is_none()
    );
    assert!(session.release_output(attachment.lease));
    session.reclaim_window_expiry_wins(Duration::ZERO).unwrap();
    session.close(true).unwrap();
}

#[test]
fn a_stalled_write_stops_applying_bytes_once_its_lease_is_released() {
    let (session, process, _) = harness(SessionOptions::default());
    session.start().unwrap();
    {
        let mut state = process.state.lock().unwrap();
        state.max_write = 2;
        state.block_write = true;
    }
    let attachment = session.attach_output(0).unwrap();
    let lease = attachment.lease;
    let writer = {
        let session = Arc::clone(&session);
        thread::spawn(move || session.write_input(lease, b"abcdef"))
    };
    process.await_write_start();

    // The renderer is released while its write is stalled inside the PTY.
    assert!(session.release_output(lease));
    process.unblock_writes();
    assert!(writer.join().unwrap().is_err());
    assert_eq!(process.state.lock().unwrap().writes, b"ab");
    session.close(true).unwrap();
}

#[test]
fn stream_tickets_are_single_use_and_rotate() {
    let (session, _, _) = harness(SessionOptions::default());
    assert!(!session.consume_ticket(""));
    session.set_ticket("first-ticket".to_owned());
    assert!(!session.consume_ticket("wrong"));
    session.set_ticket("second-ticket".to_owned());
    assert!(!session.consume_ticket("first-ticket"));
    assert!(session.consume_ticket("second-ticket"));
    assert!(!session.consume_ticket("second-ticket"));
}

#[test]
fn a_refused_attachment_keeps_the_ticket_and_a_granted_one_burns_it() {
    let (session, _, _) = harness(SessionOptions::default());
    session.start().unwrap();
    session.set_ticket("ticket".to_owned());
    let held = session.attach_output(0).unwrap();
    // The lease is held, so the ticket buys nothing — but burning it here would
    // charge every re-claim race a full round trip for a replacement.
    assert_eq!(
        session.attach_with_ticket("ticket", 0).err(),
        Some(StreamAttachRefusal::Unavailable)
    );
    assert_eq!(
        session.attach_with_ticket("wrong", 0).err(),
        Some(StreamAttachRefusal::Unauthorized)
    );

    assert!(session.release_output(held.lease));
    let reclaimed = session.attach_with_ticket("ticket", 0).unwrap();
    assert!(session.release_output(reclaimed.lease));
    // Granting the lease burns it, so the ticket stays single-use.
    assert_eq!(
        session.attach_with_ticket("ticket", 0).err(),
        Some(StreamAttachRefusal::Unauthorized)
    );
    session.close(true).unwrap();
}

#[test]
fn attachment_and_reclaim_expiry_race_has_exactly_one_winner() {
    for _ in 0..100 {
        let (session, _, _) = harness(SessionOptions::default());
        let attach = Arc::clone(&session);
        let expire = Arc::clone(&session);
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let a = Arc::clone(&barrier);
        let b = Arc::clone(&barrier);
        let attach_thread = thread::spawn(move || {
            a.wait();
            attach.attach_output(0).is_ok()
        });
        let expire_thread = thread::spawn(move || {
            b.wait();
            expire.reclaim_window_expiry_wins(Duration::ZERO).is_ok()
        });
        barrier.wait();
        assert_eq!(
            u8::from(attach_thread.join().unwrap()) + u8::from(expire_thread.join().unwrap()),
            1
        );
    }
}

#[test]
fn the_reclaim_window_restarts_on_release_and_expires_when_unclaimed() {
    let (session, _, _) = harness(SessionOptions::default());
    session.start().unwrap();
    // A generous window has not elapsed whatever the machine was doing; a zero
    // one always has. Neither depends on how long construction took.
    let generous = Duration::from_secs(30);
    assert!(session.reclaim_window_expiry_wins(generous).is_err());

    let attachment = session.attach_output(0).unwrap();
    // An attached session is never reclaimed, however long it holds the lease.
    assert!(session.reclaim_window_expiry_wins(Duration::ZERO).is_err());

    assert!(session.release_output(attachment.lease));
    // The window restarts on release, and expires once it elapses.
    assert!(session.reclaim_window_expiry_wins(generous).is_err());
    session.reclaim_window_expiry_wins(Duration::ZERO).unwrap();
    // The winner is exclusive, and an expired session refuses further claims.
    assert!(session.reclaim_window_expiry_wins(Duration::ZERO).is_err());
    assert!(session.attach_output(0).is_err());
    session.close(true).unwrap();
}

#[test]
fn graceful_and_force_close_use_distinct_paths() {
    // A leader that honours SIGTERM is never killed by a graceful close.
    let (graceful, process, _) = harness(SessionOptions::default());
    graceful.start().unwrap();
    graceful.close(false).unwrap();
    let state = process.state.lock().unwrap();
    assert_eq!((state.terminate_calls, state.kill_calls), (1, 0));
    drop(state);

    // Force always ends with the kill sweep, after a short SIGTERM grace.
    let (forced, process, _) = harness(SessionOptions::default());
    forced.start().unwrap();
    forced.close(true).unwrap();
    let state = process.state.lock().unwrap();
    assert_eq!((state.terminate_calls, state.kill_calls), (1, 1));
}

#[test]
fn force_close_escalates_after_a_short_grace_and_graceful_waits_the_full_timeout() {
    let options = SessionOptions {
        graceful_timeout: Duration::from_millis(600),
        ..SessionOptions::default()
    };
    let (forced, process, _) = harness(options);
    process.configure(|state| state.ignore_terminate = true);
    forced.start().unwrap();
    let started = Instant::now();
    forced.close(true).unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(process.state.lock().unwrap().kill_calls, 1);

    let (graceful, process, _) = harness(options);
    process.configure(|state| state.ignore_terminate = true);
    graceful.start().unwrap();
    let started = Instant::now();
    graceful.close(false).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(600));
    assert_eq!(process.state.lock().unwrap().kill_calls, 1);
}

#[test]
fn an_exit_is_recorded_while_a_background_job_holds_the_slave_open() {
    let (session, process, _) = harness(SessionOptions::default());
    process.configure(|state| state.background_job = true);
    let exits = session.take_exit_results().unwrap();
    session.start().unwrap();
    process.exit(4);
    // End of file never comes on its own; the drain window closes the PTY,
    // which cancels the reader, and the exit is delivered anyway.
    let result = exits.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(result.exit_code, 4);
    assert_eq!(result.state, SessionState::Exited);
    let started = Instant::now();
    session.close(false).unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(process.state.lock().unwrap().close_calls, 1);
}

#[test]
fn closing_a_shell_with_a_background_job_never_hangs() {
    for force in [false, true] {
        let (session, process, _) = harness(SessionOptions::default());
        process.configure(|state| state.background_job = true);
        session.start().unwrap();
        let started = Instant::now();
        session.close(force).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "close(force = {force}) took {:?}",
            started.elapsed()
        );
        assert_eq!(session.state(), SessionState::Closed);
        assert!(process.state.lock().unwrap().kill_calls >= 1);
    }
}

#[test]
fn a_reader_that_cannot_be_cancelled_is_detached_not_joined_forever() {
    let (session, process, _) = harness(SessionOptions::default());
    process.configure(|state| state.stuck_reader = true);
    session.start().unwrap();
    let started = Instant::now();
    session.close(true).unwrap();
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(session.state(), SessionState::Closed);
}

#[test]
fn a_reaped_leader_is_never_signalled_by_a_later_close() {
    for force in [false, true] {
        let (session, process, _) = harness(SessionOptions::default());
        let exits = session.take_exit_results().unwrap();
        session.start().unwrap();
        process.exit(0);
        exits.recv_timeout(Duration::from_secs(1)).unwrap();
        for _ in 0..200 {
            if process.state.lock().unwrap().close_calls == 1 {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        session.close(force).unwrap();
        let state = process.state.lock().unwrap();
        assert_eq!(state.signals_after_reap, 0, "force = {force}");
        assert_eq!(state.close_calls, 1);
    }
}

#[test]
fn association_changes_are_revision_fenced_and_live_fenced() {
    let (session, process, _) = harness(SessionOptions::default());
    session.start().unwrap();
    let first = session
        .associate(TerminalAssociationPointer {
            version: 1,
            plan_id: 7,
            task_id: 9,
        })
        .unwrap();
    assert_eq!(first.revision, 1);
    let change = session
        .prepare_association_change(
            TerminalAssociationPointer {
                version: 1,
                plan_id: 7,
                task_id: 10,
            },
            1,
        )
        .unwrap();
    session.commit_association_change(&change).unwrap();
    assert!(session.commit_association_change(&change).is_err());
    assert_eq!(
        session
            .with_live_association(2, |value| value.pointer.task_id)
            .unwrap(),
        10
    );
    session.rollback_association_change(&change).unwrap();
    process.exit(0);
    thread::sleep(Duration::from_millis(30));
    assert!(session.with_live_association(1, |_| ()).is_err());
    session.close(false).unwrap();
}

/// A real shell with a job-control background job that holds the slave open.
/// Returns the running session, its exit receiver, and the job's pid.
#[cfg(unix)]
fn native_shell_with_background_job() -> (
    Arc<Session>,
    std::sync::mpsc::Receiver<crate::session::ExitResult>,
    rustix::process::Pid,
) {
    let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_owned());
    let session = Session::new_with_options(
        StartRequest {
            executable: "/bin/sh".to_owned(),
            args: vec!["-i".to_owned()],
            env: vec![
                format!("PATH={path}"),
                "TERM=dumb".to_owned(),
                "PS1=$ ".to_owned(),
                "ENV=/dev/null".to_owned(),
            ],
            cwd: std::env::temp_dir(),
            rows: 24,
            columns: 80,
        },
        SessionMetadata {
            id: "native".to_owned(),
            profile_id: "shell".to_owned(),
            profile_kind: ProfileKind::Shell,
            provider: String::new(),
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            shell_integration: ShellIntegrationDescriptor::none(),
        },
        Arc::new(crate::pty::NativePtyFactory),
        SessionOptions::default(),
    );
    let exits = session.take_exit_results().unwrap();
    session.start().unwrap();
    let mut attachment = session.attach_output(0).unwrap();
    session
        // Interactive bash history-expands the `!` in `$!` for the whole
        // line, so expansion is switched off on a line of its own first.
        .write_input(
            attachment.lease,
            b"set +H 2>/dev/null\nsleep 30 & echo \"bg=[$!]\"\n",
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut output = String::from_utf8_lossy(&attachment.replay).into_owned();
    let pid = loop {
        // The echoed command line holds the literal `$!`; only the expanded
        // line carries digits between the brackets.
        if let Some(pid) = output.split("bg=[").skip(1).find_map(|rest| {
            rest.split(']')
                .next()
                .and_then(|digits| digits.parse::<i32>().ok())
        }) {
            break rustix::process::Pid::from_raw(pid).unwrap();
        }
        assert!(Instant::now() < deadline, "no background pid in {output:?}");
        match attachment.live.try_recv() {
            Ok(chunk) => output.push_str(&String::from_utf8_lossy(&chunk)),
            Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    };
    assert!(rustix::process::test_kill_process(pid).is_ok());
    assert!(session.release_output(attachment.lease));
    (session, exits, pid)
}

#[cfg(unix)]
fn assert_process_gone(pid: rustix::process::Pid) {
    // Killed, the job is a zombie until the reaper collects it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while rustix::process::test_kill_process(pid).is_ok() {
        assert!(Instant::now() < deadline, "background job {pid:?} survived");
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
#[test]
fn native_close_with_a_background_job_returns_and_ends_the_job() {
    for force in [false, true] {
        let (session, _exits, job) = native_shell_with_background_job();
        let started = Instant::now();
        session.close(force).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "close(force = {force}) took {:?}",
            started.elapsed()
        );
        assert_eq!(session.state(), SessionState::Closed);
        assert_process_gone(job);
    }
}

#[cfg(unix)]
#[test]
fn native_exit_with_a_background_job_is_recorded_and_close_ends_the_job() {
    let (session, exits, job) = native_shell_with_background_job();
    let attachment = session.attach_output(0).unwrap();
    session.write_input(attachment.lease, b"exit\n").unwrap();
    // Keep rendering: a shell restoring its terminal modes on exit waits for
    // its output to drain, exactly as it would behind a real renderer.
    let mut attachment = attachment;
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        while attachment.live.try_recv().is_ok() {}
        if let Ok(result) = exits.try_recv() {
            break result;
        }
        assert!(
            Instant::now() < deadline,
            "the exit must be recorded although the job holds the slave"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(result.state, SessionState::Exited);
    let started = Instant::now();
    session.close(false).unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_process_gone(job);
}

#[test]
fn an_attachment_reports_the_sequence_its_replay_actually_resumes_from() {
    let options = SessionOptions {
        replay_buffer_bytes: 4,
        ..SessionOptions::default()
    };
    let (session, process, _) = harness(options);
    session.start().unwrap();
    process.output(b"abcdef");
    wait_for_output(&session, 6);
    // A mint clamped to 2 is stale once more output wrapped the buffer.
    let attachment = session.attach_output(1).unwrap();
    assert!(attachment.gap);
    assert_eq!(attachment.resumed, 2);
    assert_eq!(attachment.replay, b"cdef");
    assert!(session.release_output(attachment.lease));
    let exact = session.attach_output(3).unwrap();
    assert!(!exact.gap);
    assert_eq!(exact.resumed, 3);
    session.close(true).unwrap();
}
