use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    MAX_INPUT_FRAME_BYTES, OUTPUT_CHUNK_BYTES, STREAM_PATH_PREFIX, StreamAttachment, StreamServer,
    StreamSession, StreamSessionError, StreamSessionHost, allowed_stream_origin_str,
};

const TEST_WAIT: Duration = Duration::from_secs(3);

struct TestSession {
    id: String,
    token: String,
    startup: Vec<u8>,
    live: Mutex<Option<mpsc::Receiver<Vec<u8>>>>,
    attached: AtomicBool,
    input: mpsc::UnboundedSender<Vec<u8>>,
    input_block: Option<Arc<BlockingInput>>,
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

    fn stream_token(&self) -> &str {
        &self.token
    }

    fn attach_output(&self) -> Result<StreamAttachment, StreamSessionError> {
        if self.attached.swap(true, Ordering::AcqRel) {
            return Err(StreamSessionError("already attached".into()));
        }
        let live = self
            .live
            .lock()
            .expect("test live lock poisoned")
            .take()
            .ok_or_else(|| StreamSessionError("output unavailable".into()))?;
        Ok(StreamAttachment {
            startup: self.startup.clone(),
            live,
        })
    }

    fn write_input(&self, input: &[u8]) -> Result<(), StreamSessionError> {
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
    close_count: AtomicUsize,
    closed: Notify,
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

    fn close_stream_session(
        &self,
        session_id: &str,
        force: bool,
    ) -> Result<(), StreamSessionError> {
        assert!(!force, "stream teardown must be graceful");
        if let Some(session) = self
            .sessions
            .lock()
            .expect("test sessions lock poisoned")
            .remove(session_id)
            && let Some(block) = &session.input_block
        {
            block.release();
        }
        self.close_count.fetch_add(1, Ordering::AcqRel);
        self.closed.notify_waiters();
        Ok(())
    }
}

struct Fixture {
    server: Arc<StreamServer>,
    host: Arc<TestHost>,
    session: Arc<TestSession>,
    output: mpsc::Sender<Vec<u8>>,
    input: mpsc::UnboundedReceiver<Vec<u8>>,
    url: String,
}

async fn fixture(startup: Vec<u8>) -> Fixture {
    fixture_with_input_block(startup, None).await
}

async fn fixture_with_input_block(
    startup: Vec<u8>,
    input_block: Option<Arc<BlockingInput>>,
) -> Fixture {
    let (output, live) = mpsc::channel(32);
    let (input, input_rx) = mpsc::unbounded_channel();
    let session = Arc::new(TestSession {
        id: "0123456789abcdefghijklmnopqrstuvwxyzABCDEFG".into(),
        token: "token_0123456789abcdefghijklmnopqrstuvwxyzA".into(),
        startup,
        live: Mutex::new(Some(live)),
        attached: AtomicBool::new(false),
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
    let url = server.session_url(session.as_ref());
    Fixture {
        server,
        host,
        session,
        output,
        input: input_rx,
        url,
    }
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

    let second_url = fixture.server.session_url(fixture.session.as_ref());
    assert_eq!(second_url, fixture.url);
    fixture.server.shutdown().await.unwrap();
}

#[tokio::test]
async fn http_gate_rejects_method_origin_path_session_token_and_second_attach() {
    let invalid_method = fixture(Vec::new()).await;
    let request = format!(
        "POST {STREAM_PATH_PREFIX}{}?token={} HTTP/1.1\r\nHost: {}\r\nOrigin: wails://wails\r\nConnection: close\r\n\r\n",
        invalid_method.session.id,
        invalid_method.session.token,
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
    let wrong_url = wrong_token.url.replace(&wrong_token.session.token, "wrong");
    assert_dial_status(&wrong_url, "wails://wails", 401).await;
    wrong_token.server.shutdown().await.unwrap();

    let attached = fixture(Vec::new()).await;
    let connection = dial(&attached.url, "wails://wails").await.unwrap();
    assert_dial_status(&attached.url, "wails://wails", 409).await;
    drop(connection);
    await_closed(&attached.host).await;
    fixture_shutdown(&attached).await;
}

#[tokio::test]
async fn stream_carries_binary_io_and_normal_close_is_single_use() {
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

    fixture.output.send(b"live output".to_vec()).await.unwrap();
    assert_eq!(
        timeout_message(&mut connection).await,
        Message::Binary(b"live output".as_slice().into())
    );
    connection
        .send(Message::Text(r#"{"type":"ack","bytes":11}"#.into()))
        .await
        .unwrap();
    drop(fixture.output);

    let Message::Close(frame) = timeout_message(&mut connection).await else {
        panic!("expected normal close frame");
    };
    let frame = frame.expect("close frame");
    assert_eq!(frame.code, CloseCode::Normal);
    assert!(frame.reason.is_empty());
    await_closed(&fixture.host).await;
    assert!(dial(&fixture.url, "wails://wails").await.is_err());
    fixture.server.shutdown().await.unwrap();
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
        await_closed(&fixture.host).await;
        assert!(dial(&fixture.url, "wails://wails").await.is_err());
        fixture_shutdown(&fixture).await;
    }

    let fixture = fixture(Vec::new()).await;
    let connection = dial(&fixture.url, "wails://wails").await.unwrap();
    drop(connection);
    await_closed(&fixture.host).await;
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
    assert_eq!(fixture.host.close_count.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn output_window_stalls_at_448_kib_and_resumes_by_exact_ack() {
    let fixture = fixture(Vec::new()).await;
    let mut connection = dial(&fixture.url, "wails://wails").await.unwrap();
    let output = vec![b'o'; OUTPUT_CHUNK_BYTES];
    for _ in 0..10 {
        fixture.output.send(output.clone()).await.unwrap();
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
    await_closed(&fixture.host).await;
    fixture_shutdown(&fixture).await;
}

#[tokio::test]
async fn sustained_hundred_mebibyte_stream_is_lossless_with_acknowledgements() {
    const TOTAL: usize = 100 * 1024 * 1024;
    let fixture = fixture(Vec::new()).await;
    let mut connection = dial(&fixture.url, "wails://wails").await.unwrap();
    let sender = fixture.output.clone();
    let producer = tokio::spawn(async move {
        let mut remaining = TOTAL;
        let mut sequence = 0_u8;
        while remaining > 0 {
            let length = remaining.min(OUTPUT_CHUNK_BYTES);
            let output = vec![sequence; length];
            sender.send(output).await.unwrap();
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
    await_closed(&fixture.host).await;
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

async fn await_closed(host: &TestHost) {
    loop {
        let notified = host.closed.notified();
        if host.close_count.load(Ordering::Acquire) > 0 {
            return;
        }
        tokio::time::timeout(TEST_WAIT, notified)
            .await
            .expect("timed out waiting for graceful session close");
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
