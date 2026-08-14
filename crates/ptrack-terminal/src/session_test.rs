use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crate::profile::ProfileKind;
use crate::pty::{PtyFactory, PtyProcess, StartRequest};
use crate::session::{
    Session, SessionMetadata, SessionOptions, SessionState, TerminalAssociationPointer,
};
use crate::shell_integration::ShellIntegrationDescriptor;

#[derive(Default)]
struct FakeState {
    output: VecDeque<u8>,
    eof: bool,
    exit: Option<(i32, Option<io::ErrorKind>)>,
    writes: Vec<u8>,
    max_write: usize,
    resizes: Vec<(u16, u16)>,
    terminate_calls: usize,
    kill_calls: usize,
    close_calls: usize,
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

    fn exit(&self, code: i32) {
        let mut state = self.state.lock().unwrap();
        state.exit = Some((code, None));
        state.eof = true;
        self.changed.notify_all();
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
        error.map_or(Ok(code), |kind| Err(io::Error::from(kind)))
    }

    fn terminate(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.terminate_calls += 1;
        state.exit = Some((0, None));
        state.eof = true;
        self.changed.notify_all();
        Ok(())
    }

    fn kill(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.kill_calls += 1;
        state.exit = Some((1, None));
        state.eof = true;
        self.changed.notify_all();
        Ok(())
    }

    fn close(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.close_calls += 1;
        state.eof = true;
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
            stream_token: "token".to_owned(),
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

    session.write_input(b"abcdef").unwrap();
    assert_eq!(process.state.lock().unwrap().writes, b"abcdef");
    session.resize(0, u16::MAX).unwrap();
    session.resize(0, u16::MAX).unwrap();
    assert_eq!(
        process.state.lock().unwrap().resizes,
        Vec::<(u16, u16)>::new()
    );
    session.resize(30, 100).unwrap();
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
fn startup_output_is_bounded_then_replayed_before_live_output() {
    let options = SessionOptions {
        startup_buffer_bytes: 8,
        ..SessionOptions::default()
    };
    let (session, process, _) = harness(options);
    session.start().unwrap();
    process.output(b"0123456789abcdef");
    thread::sleep(Duration::from_millis(30));
    let mut attachment = session.attach_output().unwrap();
    assert_eq!(attachment.startup, b"01234567");
    assert_eq!(attachment.live.blocking_recv().unwrap(), b"89abcdef");
    process.output(b"live");
    assert_eq!(attachment.live.blocking_recv().unwrap(), b"live");
    session.close(true).unwrap();
}

#[test]
fn attachment_and_expiry_race_has_exactly_one_winner() {
    for _ in 0..100 {
        let (session, _, _) = harness(SessionOptions::default());
        let attach = Arc::clone(&session);
        let expire = Arc::clone(&session);
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let a = Arc::clone(&barrier);
        let b = Arc::clone(&barrier);
        let attach_thread = thread::spawn(move || {
            a.wait();
            attach.attach_output().is_ok()
        });
        let expire_thread = thread::spawn(move || {
            b.wait();
            expire.attachment_expiry_wins()
        });
        barrier.wait();
        assert_eq!(
            u8::from(attach_thread.join().unwrap()) + u8::from(expire_thread.join().unwrap()),
            1
        );
    }
}

#[test]
fn graceful_and_force_close_use_distinct_paths() {
    let (graceful, process, _) = harness(SessionOptions::default());
    graceful.start().unwrap();
    graceful.close(false).unwrap();
    let state = process.state.lock().unwrap();
    assert_eq!((state.terminate_calls, state.kill_calls), (1, 0));
    drop(state);

    let (forced, process, _) = harness(SessionOptions::default());
    forced.start().unwrap();
    forced.close(true).unwrap();
    let state = process.state.lock().unwrap();
    assert_eq!((state.terminate_calls, state.kill_calls), (0, 1));
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
