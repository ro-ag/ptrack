use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, mpsc};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};

use super::{
    MAX_INPUT_FRAME_BYTES, OUTPUT_CHUNK_BYTES, STREAM_GAP_CONTROL, STREAM_PATH_PREFIX,
    StreamAttachRefusal, StreamAttachment, StreamServer, StreamSession, StreamSessionError,
    StreamSessionHost, allowed_stream_origin_str,
};

const TEST_WAIT: Duration = Duration::from_secs(3);

struct TestSession {
    id: String,
    ticket: Mutex<Option<String>>,
    tickets: AtomicUsize,
    scrollback: Vec<u8>,
    gap: AtomicBool,
    live: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    attached: AtomicU64,
    leases: AtomicU64,
    released: AtomicUsize,
    changed: Notify,
    input: mpsc::UnboundedSender<Vec<u8>>,
    input_block: Option<Arc<BlockingInput>>,
}

impl TestSession {
    /// Mint the next single-use ticket, retiring any earlier one.
    fn mint(&self) -> String {
        let ticket = format!("ticket-{}", self.tickets.fetch_add(1, Ordering::AcqRel));
        *self.ticket.lock().expect("test ticket lock poisoned") = Some(ticket.clone());
        ticket
    }

    async fn send(&self, output: Vec<u8>) {
        let sender = self
            .live
            .lock()
            .expect("test live lock poisoned")
            .clone()
            .expect("terminal output requires an attached renderer");
        sender.send(output).await.expect("live output send failed");
    }

    fn end_output(&self) {
        self.live.lock().expect("test live lock poisoned").take();
    }
}

#[derive(Default)]
struct BlockingInput {
    started: AtomicBool,
    released: Mutex<bool>,
    changed: Condvar,
}

impl BlockingInput {
    fn wait(&self) {
        self.started.store(true, Ordering::Release);
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.changed.wait(released).unwrap();
        }
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

impl StreamSession for TestSession {
    fn id(&self) -> &str {
        &self.id
    }

    fn attach_with_ticket(
        &self,
        presented: &str,
        from_sequence: u64,
    ) -> Result<StreamAttachment, StreamAttachRefusal> {
        let mut ticket = self.ticket.lock().expect("test ticket lock poisoned");
        if presented.is_empty() || ticket.as_deref() != Some(presented) {
            return Err(StreamAttachRefusal::Unauthorized);
        }
        if self.attached.load(Ordering::Acquire) != 0 {
            return Err(StreamAttachRefusal::Unavailable);
        }
        let start = usize::try_from(from_sequence)
            .ok()
            .filter(|start| *start <= self.scrollback.len())
            .ok_or(StreamAttachRefusal::Unavailable)?;
        let lease = self.leases.fetch_add(1, Ordering::AcqRel) + 1;
        let (sender, live) = mpsc::channel(32);
        *self.live.lock().expect("test live lock poisoned") = Some(sender);
        self.attached.store(lease, Ordering::Release);
        // Burned only now that the lease is actually granted.
        *ticket = None;
        Ok(StreamAttachment {
            lease,
            gap: self.gap.load(Ordering::Acquire),
            replay: self.scrollback[start..].to_vec(),
            live,
        })
    }

    fn release_output(&self, lease: u64) {
        if self
            .attached
            .compare_exchange(lease, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.end_output();
        // Deliberately does not unblock a stalled input write: the real
        // `Session::release_output` touches neither the PTY nor its writers,
        // so a double that released the block would hide a stream that cannot
        // tear down while a write is stalled.
        self.released.fetch_add(1, Ordering::AcqRel);
        self.changed.notify_waiters();
    }

    fn write_input(&self, lease: u64, input: &[u8]) -> Result<(), StreamSessionError> {
        if self.attached.load(Ordering::Acquire) != lease {
            return Err(StreamSessionError("stale lease".into()));
        }
        if let Some(block) = &self.input_block {
            block.wait();
        }
        self.input
            .send(input.to_vec())
            .map_err(|_| StreamSessionError("input unavailable".into()))
    }
}

#[derive(Default)]
struct TestHost {
    sessions: Mutex<HashMap<String, Arc<TestSession>>>,
}

impl StreamSessionHost for TestHost {
    fn stream_session(&self, session_id: &str) -> Option<Arc<dyn StreamSession>> {
        self.sessions
            .lock()
            .expect("test sessions lock poisoned")
            .get(session_id)
            .cloned()
            .map(|session| session as Arc<dyn StreamSession>)
    }
}

struct Fixture {
    server: Arc<StreamServer>,
    host: Arc<TestHost>,
    session: Arc<TestSession>,
    input: mpsc::UnboundedReceiver<Vec<u8>>,
    url: String,
}

impl Fixture {
    /// A URL carrying a freshly minted single-use ticket.
    fn mint(&self, from_sequence: u64) -> String {
        self.server
            .session_url(&self.session.id, &self.session.mint(), from_sequence)
    }
}

async fn fixture(scrollback: Vec<u8>) -> Fixture {
    fixture_with_input_block(scrollback, None).await
}

async fn fixture_with_input_block(
    scrollback: Vec<u8>,
    input_block: Option<Arc<BlockingInput>>,
) -> Fixture {
    let (input, input_rx) = mpsc::unbounded_channel();
    let session = Arc::new(TestSession {
        id: "0123456789abcdefghijklmnopqrstuvwxyzABCDEFG".into(),
        ticket: Mutex::new(None),
        tickets: AtomicUsize::new(0),
        scrollback,
        gap: AtomicBool::new(false),
        live: Mutex::new(None),
        attached: AtomicU64::new(0),
        leases: AtomicU64::new(0),
        released: AtomicUsize::new(0),
        changed: Notify::new(),
        input,
        input_block,
    });
    let host = Arc::new(TestHost::default());
    host.sessions
        .lock()
        .unwrap()
        .insert(session.id.clone(), Arc::clone(&session));
    let trait_host: Arc<dyn StreamSessionHost> = host.clone();
    let server = StreamServer::bind(Arc::downgrade(&trait_host))
        .await
        .unwrap();
    let url = server.session_url(&session.id, &session.mint(), 0);
    Fixture {
        server,
        host,
        session,
        input: input_rx,
        url,
    }
}

fn ticket_of(url: &str) -> &str {
    url.split("?token=")
        .nth(1)
        .and_then(|query| query.split('&').next())
        .expect("stream URL carries a ticket")
}

async fn dial(
    url: &str,
    origin: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    WebSocketError,
> {
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Origin", HeaderValue::from_str(origin).unwrap());
    connect_async(request)
        .await
        .map(|(connection, _)| connection)
}

async fn assert_dial_status(url: &str, origin: &str, status: u16) {
    let error = dial(url, origin)
        .await
        .expect_err("WebSocket upgrade unexpectedly succeeded");
    let WebSocketError::Http(response) = error else {
        panic!("upgrade failed without HTTP response: {error}");
    };
    assert_eq!(response.status().as_u16(), status);
}

async fn raw_status(server: &StreamServer, request: &str) -> u16 {
    let mut stream = tokio::net::TcpStream::connect(server.local_addr())
        .await
        .unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(TEST_WAIT, stream.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    let first = std::str::from_utf8(&response)
        .unwrap()
        .lines()
        .next()
        .unwrap();
    first.split_whitespace().nth(1).unwrap().parse().unwrap()
}

#[test]
fn stream_origin_policy_matches_the_frozen_allowlist() {
    for accepted in [
        "wails://wails",
        "tauri://localhost",
        "http://wails.localhost",
        "https://wails.localhost",
        "https://tauri.localhost",
        "http://localhost:5173",
        "https://LOCALHOST:34115",
        "http://127.0.0.1:5173",
        "http://[::1]:5173",
    ] {
        assert!(allowed_stream_origin_str(accepted), "rejected {accepted}");
    }
    for rejected in [
        "",
        "null",
        "https://evil.example",
        "http://wails.localhost.evil.example",
        "http://192.0.2.10:5173",
        "file:///tmp/frontend",
        "http://user@localhost:5173",
    ] {
        assert!(!allowed_stream_origin_str(rejected), "accepted {rejected}");
    }
}

#[tokio::test]
async fn one_ipv4_listener_serves_opaque_authenticated_session_urls() {
    let fixture = fixture(Vec::new()).await;
    assert!(fixture.server.local_addr().is_ipv4());
    assert_eq!(fixture.server.local_addr().ip().to_string(), "127.0.0.1");
    assert!(fixture.server.local_addr().port() > 0);
    assert!(fixture.url.starts_with(&format!(
        "ws://{}{STREAM_PATH_PREFIX}{}?token=",
        fixture.server.local_addr(),
        fixture.session.id
    )));

    // Every mint rotates the ticket; only the host and path stay stable.
    let second_url = fixture.mint(0);
    assert_ne!(second_url, fixture.url);
    assert_eq!(
        second_url.split("?token=").next(),
        fixture.url.split("?token=").next()
    );
    fixture.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn http_gate_rejects_method_origin_path_session_token_and_second_attach() {
    let invalid_method = fixture(Vec::new()).await;
    let request = format!(
        "POST {STREAM_PATH_PREFIX}{}?token={} HTTP/1.1\r\nHost: {}\r\nOrigin: wails://wails\r\nConnection: close\r\n\r\n",
        invalid_method.session.id,
        ticket_of(&invalid_method.url),
        invalid_method.server.local_addr()
    );
    assert_eq!(raw_status(&invalid_method.server, &request).await, 403);
    invalid_method.server.shutdown().await.unwrap();

    let invalid_origin = fixture(Vec::new()).await;
    assert_dial_status(&invalid_origin.url, "https://evil.example", 403).await;
    invalid_origin.server.shutdown().await.unwrap();

    let invalid_path = fixture(Vec::new()).await;
    let mut parsed = invalid_path.url.clone();
    parsed = parsed.replace(
        &format!("{STREAM_PATH_PREFIX}{}", invalid_path.session.id),
        STREAM_PATH_PREFIX,
    );
    assert_dial_status(&parsed, "wails://wails", 404).await;
    invalid_path.server.shutdown().await.unwrap();

    let unknown = fixture(Vec::new()).await;
    let unknown_url = unknown.url.replace(&unknown.session.id, "unknown-session");
    assert_dial_status(&unknown_url, "wails://wails", 404).await;
    unknown.server.shutdown().await.unwrap();

    let missing_token = fixture(Vec::new()).await;
    let no_token = missing_token.url.split('?').next().unwrap();
    assert_dial_status(no_token, "wails://wails", 401).await;
    missing_token.server.shutdown().await.unwrap();

    let wrong_token = fixture(Vec::new()).await;
    let wrong_url = wrong_token
        .url
        .replace(ticket_of(&wrong_token.url), "wrong");
    assert_dial_status(&wrong_url, "wails://wails", 401).await;
    wrong_token.server.shutdown().await.unwrap();

    let malformed_sequence = fixture(Vec::new()).await;
    let malformed = malformed_sequence.url.replace("&from=0", "&from=-1");
    assert_dial_status(&malformed, "wails://wails", 400).await;
    malformed_sequence.server.shutdown().await.unwrap();

    let attached = fixture(Vec::new()).await;
    let connection = dial(&attached.url, "wails://wails").await.unwrap();
    // A second renderer is refused even with a valid fresh ticket, and the
    // refusal leaves the held lease and the session alive.
    assert_dial_status(&attached.mint(0), "wails://wails", 409).await;
    // The spent ticket is dead, so a leaked URL cannot re-claim anything.
    assert_dial_status(&attached.url, "wails://wails", 401).await;
    assert_eq!(attached.session.released.load(Ordering::Acquire), 0);
    assert_eq!(attached.host.sessions.lock().unwrap().len(), 1);
    drop(connection);
    await_released(&attached.session).await;
    fixture_shutdown(&attached).await;
}

#[tokio::test]
async fn stream_carries_binary_io_and_normal_close_releases_without_terminating() {
    let startup = vec![b's'; OUTPUT_CHUNK_BYTES + 3];
    let mut fixture = fixture(startup.clone()).await;
    let mut connection = dial(&fixture.url, "wails://wails").await.unwrap();

    let mut received = Vec::new();
    while received.len() < startup.len() {
        let message = timeout_message(&mut connection).await;
        let Message::Binary(output) = message else {
            panic!("expected binary output, got {message:?}");
        };
        assert!(!output.is_empty() && output.len() <= OUTPUT_CHUNK_BYTES);
        received.extend_from_slice(&output);
    }
    assert_eq!(received, startup);
    connection
        .send(Message::Text(
            format!(r#"{{"type":"ack","bytes":{}}}"#, received.len()).into(),
        ))
        .await
        .unwrap();

    let input = BytesForTest::binary_input();
    connection
        .send(Message::Binary(input.clone().into()))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(TEST_WAIT, fixture.input.recv())
            .await
            .unwrap()
            .unwrap(),
        input
    );

    fixture.session.send(b"live output".to_vec()).await;
    assert_eq!(
        timeout_message(&mut connection).await,
        Message::Binary(b"live output".as_slice().into())
    );
    connection
        .send(Message::Text(r#"{"type":"ack","bytes":11}"#.into()))
        .await
        .unwrap();
    fixture.session.end_output();

    let Message::Close(frame) = timeout_message(&mut connection).await else {
        panic!("expected normal close frame");
    };
    let frame = frame.expect("close frame");
    assert_eq!(frame.code, CloseCode::Normal);
    assert!(frame.reason.is_empty());
    connection.flush().await.unwrap();
    await_released(&fixture.session).await;
    // The renderer is gone but the session is not: only its ticket is spent.
    assert_eq!(fixture.host.sessions.lock().unwrap().len(), 1);
    assert!(dial(&fixture.url, "wails://wails").await.is_err());
    fixture.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_released_session_is_reclaimed_by_a_fresh_ticket_and_replays() {
    let fixture = fixture(b"scrollback".to_vec()).await;
    let mut connection = dial(&fixture.url, "wails://wails").await.unwrap();
    assert_eq!(
        timeout_message(&mut connection).await,
        Message::Binary(b"scrollback".as_slice().into())
    );

    // Losing the renderer releases the lease; the session keeps running.
    drop(connection);
    await_released(&fixture.session).await;
    assert_eq!(fixture.host.sessions.lock().unwrap().len(), 1);

    let mut reclaimed = dial(&fixture.mint(6), "wails://wails").await.unwrap();
    assert_eq!(
        timeout_message(&mut reclaimed).await,
        Message::Binary(b"back".as_slice().into())
    );
    fixture.session.send(b"after".to_vec()).await;
    assert_eq!(
        timeout_message(&mut reclaimed).await,
        Message::Binary(b"after".as_slice().into())
    );
    drop(reclaimed);
    fixture_shutdown(&fixture).await;
}

#[tokio::test]
async fn malformed_control_oversized_input_and_disconnect_fail_closed() {
    for message in [
        Message::Text(r#"{"type":"ack","bytes":1}"#.into()),
        Message::Text(r#"{"type":"detach","bytes":1}"#.into()),
        Message::Binary(Vec::<u8>::new().into()),
        Message::Binary(vec![b'x'; MAX_INPUT_FRAME_BYTES + 1].into()),
    ] {
        let fixture = fixture(Vec::new()).await;
        let mut connection = dial(&fixture.url, "wails://wails").await.unwrap();
        let _ = connection.send(message).await;
        assert_stream_closes(&mut connection).await;
        await_released(&fixture.session).await;
        assert_eq!(fixture.host.sessions.lock().unwrap().len(), 1);
        assert!(dial(&fixture.url, "wails://wails").await.is_err());
        fixture_shutdown(&fixture).await;
    }

    let fixture = fixture(Vec::new()).await;
    let connection = dial(&fixture.url, "wails://wails").await.unwrap();
    drop(connection);
    await_released(&fixture.session).await;
    assert!(dial(&fixture.url, "wails://wails").await.is_err());
    fixture_shutdown(&fixture).await;
}

#[tokio::test]
async fn shutdown_unblocks_a_stalled_terminal_input_write() {
    let input_block = Arc::new(BlockingInput::default());
    let fixture = fixture_with_input_block(Vec::new(), Some(Arc::clone(&input_block))).await;
    let mut connection = dial(&fixture.url, "wails://wails").await.unwrap();
    connection
        .send(Message::Binary(b"blocked input".as_slice().into()))
        .await
        .unwrap();
    tokio::time::timeout(TEST_WAIT, async {
        while !input_block.started.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    tokio::time::timeout(TEST_WAIT, fixture.server.shutdown())
        .await
        .expect("stream shutdown remained blocked on PTY input")
        .unwrap();
    assert_eq!(fixture.session.released.load(Ordering::Acquire), 1);
    // Nothing else ever unblocks the PTY write, so let the parked blocking
    // worker finish before the test runtime is dropped on it.
    input_block.release();
}

#[tokio::test]
async fn a_wrapped_replay_announces_its_gap_before_the_replay() {
    let fixture = fixture(b"retained".to_vec()).await;
    fixture.session.gap.store(true, Ordering::Release);
    let mut connection = dial(&fixture.url, "wails://wails").await.unwrap();
    assert_eq!(
        timeout_message(&mut connection).await,
        Message::Text(STREAM_GAP_CONTROL.into())
    );
    assert_eq!(
        timeout_message(&mut connection).await,
        Message::Binary(b"retained".as_slice().into())
    );
    drop(connection);
    await_released(&fixture.session).await;
    fixture_shutdown(&fixture).await;
}

#[tokio::test]
async fn a_refused_attachment_leaves_the_ticket_unspent() {
    let fixture = fixture(Vec::new()).await;
    let held = dial(&fixture.url, "wails://wails").await.unwrap();
    let retry = fixture.mint(0);
    // The lease is held, so this ticket buys nothing — but it must survive the
    // refusal, or every re-claim race costs a full round trip for a new one.
    assert_dial_status(&retry, "wails://wails", 409).await;
    drop(held);
    await_released(&fixture.session).await;
    let mut reclaimed = dial(&retry, "wails://wails").await.unwrap();
    fixture.session.send(b"reclaimed".to_vec()).await;
    assert_eq!(
        timeout_message(&mut reclaimed).await,
        Message::Binary(b"reclaimed".as_slice().into())
    );
    drop(reclaimed);
    fixture_shutdown(&fixture).await;
}

#[tokio::test]
async fn output_window_stalls_at_448_kib_and_resumes_by_exact_ack() {
    let fixture = fixture(Vec::new()).await;
    let mut connection = dial(&fixture.url, "wails://wails").await.unwrap();
    let output = vec![b'o'; OUTPUT_CHUNK_BYTES];
    for _ in 0..10 {
        fixture.session.send(output.clone()).await;
    }
    let mut received = 0;
    while received < 7 * OUTPUT_CHUNK_BYTES {
        let Message::Binary(frame) = timeout_message(&mut connection).await else {
            panic!("expected output frame");
        };
        received += frame.len();
    }
    assert_eq!(received, 7 * OUTPUT_CHUNK_BYTES);
    assert!(
        tokio::time::timeout(Duration::from_millis(150), connection.next())
            .await
            .is_err()
    );
    connection
        .send(Message::Text(
            format!(r#"{{"type":"ack","bytes":{OUTPUT_CHUNK_BYTES}}}"#).into(),
        ))
        .await
        .unwrap();
    let Message::Binary(frame) = timeout_message(&mut connection).await else {
        panic!("expected resumed output frame");
    };
    assert_eq!(frame.len(), OUTPUT_CHUNK_BYTES);
    drop(connection);
    await_released(&fixture.session).await;
    fixture_shutdown(&fixture).await;
}

#[tokio::test]
async fn sustained_hundred_mebibyte_stream_is_lossless_with_acknowledgements() {
    const TOTAL: usize = 100 * 1024 * 1024;
    let fixture = fixture(Vec::new()).await;
    let mut connection = dial(&fixture.url, "wails://wails").await.unwrap();
    let session = Arc::clone(&fixture.session);
    let producer = tokio::spawn(async move {
        let mut remaining = TOTAL;
        let mut sequence = 0_u8;
        while remaining > 0 {
            let length = remaining.min(OUTPUT_CHUNK_BYTES);
            session.send(vec![sequence; length]).await;
            remaining -= length;
            sequence = sequence.wrapping_add(1);
        }
    });

    let mut received = 0;
    let mut sequence = 0_u8;
    while received < TOTAL {
        let Message::Binary(output) = timeout_message(&mut connection).await else {
            panic!("expected terminal output");
        };
        assert_eq!(output.len(), (TOTAL - received).min(OUTPUT_CHUNK_BYTES));
        assert!(output.iter().all(|byte| *byte == sequence));
        received += output.len();
        sequence = sequence.wrapping_add(1);
        connection
            .send(Message::Text(
                format!(r#"{{"type":"ack","bytes":{}}}"#, output.len()).into(),
            ))
            .await
            .unwrap();
    }
    producer.await.unwrap();
    assert_eq!(received, TOTAL);
    drop(connection);
    await_released(&fixture.session).await;
    fixture_shutdown(&fixture).await;
}

async fn timeout_message<S>(connection: &mut tokio_tungstenite::WebSocketStream<S>) -> Message
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    tokio::time::timeout(TEST_WAIT, connection.next())
        .await
        .expect("timed out reading stream")
        .expect("stream ended")
        .expect("stream read failed")
}

async fn assert_stream_closes<S>(connection: &mut tokio_tungstenite::WebSocketStream<S>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let result = tokio::time::timeout(TEST_WAIT, connection.next())
        .await
        .expect("timed out waiting for stream close");
    assert!(
        result
            .is_none_or(|message| { message.is_err() || matches!(message, Ok(Message::Close(_))) })
    );
}

async fn await_released(session: &TestSession) {
    loop {
        let notified = session.changed.notified();
        if session.released.load(Ordering::Acquire) > 0 {
            return;
        }
        tokio::time::timeout(TEST_WAIT, notified)
            .await
            .expect("timed out waiting for the renderer lease release");
    }
}

async fn fixture_shutdown(fixture: &Fixture) {
    fixture.server.shutdown().await.unwrap();
}

struct BytesForTest;

impl BytesForTest {
    fn binary_input() -> Vec<u8> {
        vec![0, 1, 2, b'\r', b'\n', 0xff]
    }
}
