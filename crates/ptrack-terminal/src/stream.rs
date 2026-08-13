//! Authenticated, single-use terminal streams on an IPv4 loopback listener.

use std::convert::Infallible;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::header::ORIGIN;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use subtle::ConstantTimeEq;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::{
    CloseFrame, Message, WebSocketConfig, frame::coding::CloseCode,
};
use tokio_util::sync::CancellationToken;

use crate::{
    FlowLedger, MAX_INPUT_FRAME_BYTES, OUTPUT_CHUNK_BYTES, OUTPUT_WINDOW_BYTES, parse_ack_control,
    split_output,
};

pub const STREAM_PATH_PREFIX: &str = "/terminal/";
pub const STREAM_PONG_WAIT: Duration = Duration::from_secs(60);
pub const STREAM_PING_EVERY: Duration = Duration::from_secs(25);
pub const STREAM_WRITE_WAIT: Duration = Duration::from_secs(10);
pub const STREAM_READ_HEADER_WAIT: Duration = Duration::from_secs(5);

type ResponseBody = Full<Bytes>;

/// The output lease returned by a successful, single-use stream attachment.
#[derive(Debug)]
pub struct StreamAttachment {
    pub startup: Vec<u8>,
    pub live: mpsc::Receiver<Vec<u8>>,
}

/// A stream-facing session error. Details are intentionally not sent over HTTP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSessionError(pub String);

impl std::fmt::Display for StreamSessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StreamSessionError {}

/// Narrow adapter implemented by the terminal session.
pub trait StreamSession: Send + Sync + 'static {
    fn id(&self) -> &str;
    fn stream_token(&self) -> &str;
    /// Claim the single stream attachment lease.
    ///
    /// # Errors
    ///
    /// Returns an error when the lease has already been claimed or expired.
    fn attach_output(&self) -> Result<StreamAttachment, StreamSessionError>;
    /// Write raw client bytes to the owned PTY.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is no longer live or the PTY write fails.
    fn write_input(&self, input: &[u8]) -> Result<(), StreamSessionError>;
}

/// Narrow adapter implemented by the manager that owns the listener.
pub trait StreamSessionHost: Send + Sync + 'static {
    fn stream_session(&self, session_id: &str) -> Option<Arc<dyn StreamSession>>;
    /// Remove and gracefully close a claimed stream's session.
    ///
    /// # Errors
    ///
    /// Returns an error when host-owned session teardown fails.
    fn close_stream_session(&self, session_id: &str, force: bool)
    -> Result<(), StreamSessionError>;
}

#[derive(Debug)]
struct ActiveGuard {
    active: Arc<AtomicUsize>,
    idle: Arc<Notify>,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        if self.active.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.idle.notify_waiters();
        }
    }
}

/// One loopback stream server owned by one terminal manager.
pub struct StreamServer {
    host: Weak<dyn StreamSessionHost>,
    address: SocketAddr,
    stopping: AtomicBool,
    cancellation: CancellationToken,
    serve_task: Mutex<Option<JoinHandle<io::Result<()>>>>,
    active: Arc<AtomicUsize>,
    idle: Arc<Notify>,
}

impl std::fmt::Debug for StreamServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StreamServer")
            .field("address", &self.address)
            .field("stopping", &self.stopping.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl StreamServer {
    /// Bind exactly one IPv4-only listener to an OS-assigned loopback port.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the listener cannot bind or report its address.
    pub async fn bind(host: Weak<dyn StreamSessionHost>) -> io::Result<Arc<Self>> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let server = Arc::new(Self {
            host,
            address,
            stopping: AtomicBool::new(false),
            cancellation: CancellationToken::new(),
            serve_task: Mutex::new(None),
            active: Arc::new(AtomicUsize::new(0)),
            idle: Arc::new(Notify::new()),
        });
        let task_server = Arc::clone(&server);
        let task = tokio::spawn(async move { task_server.serve(listener).await });
        *server
            .serve_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(task);
        Ok(server)
    }

    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.address
    }

    /// Mint the authority-bearing URL for an already registered session.
    #[must_use]
    pub fn session_url(&self, session: &dyn StreamSession) -> String {
        format!(
            "ws://{}{STREAM_PATH_PREFIX}{}?token={}",
            self.address,
            session.id(),
            session.stream_token()
        )
    }

    async fn serve(self: Arc<Self>, listener: TcpListener) -> io::Result<()> {
        loop {
            tokio::select! {
                biased;
                () = self.cancellation.cancelled() => break,
                accepted = listener.accept() => {
                    let (stream, peer) = accepted?;
                    if !peer.ip().is_loopback() {
                        continue;
                    }
                    self.spawn_connection(stream);
                }
            }
        }
        drop(listener);
        self.wait_until_idle().await;
        Ok(())
    }

    fn spawn_connection(self: &Arc<Self>, stream: TcpStream) {
        self.active.fetch_add(1, Ordering::AcqRel);
        let guard = ActiveGuard {
            active: Arc::clone(&self.active),
            idle: Arc::clone(&self.idle),
        };
        let server = Arc::clone(self);
        tokio::spawn(async move {
            let _guard = guard;
            server.serve_connection(stream).await;
        });
    }

    async fn serve_connection(self: Arc<Self>, stream: TcpStream) {
        let service_server = Arc::clone(&self);
        let service = service_fn(move |request| {
            let server = Arc::clone(&service_server);
            async move { Ok::<_, Infallible>(server.handle(request).await) }
        });
        let mut builder = http1::Builder::new();
        builder
            .keep_alive(true)
            .header_read_timeout(STREAM_READ_HEADER_WAIT)
            .timer(TokioTimer::new());
        let connection = builder
            .serve_connection(TokioIo::new(stream), service)
            .with_upgrades();
        tokio::pin!(connection);
        tokio::select! {
            _ = &mut connection => {}
            () = self.cancellation.cancelled() => {
                connection.as_mut().graceful_shutdown();
                let _ = tokio::time::timeout(STREAM_WRITE_WAIT, &mut connection).await;
            }
        }
    }

    async fn handle(self: Arc<Self>, mut request: Request<Incoming>) -> Response<ResponseBody> {
        if self.stopping.load(Ordering::Acquire) {
            return text_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "terminal stream server is stopping",
            );
        }
        if request.method() != Method::GET || !allowed_stream_origin(request.headers().get(ORIGIN))
        {
            return text_response(StatusCode::FORBIDDEN, "terminal stream rejected");
        }
        let Some(session_id) = stream_session_id(request.uri().path()).map(str::to_owned) else {
            return text_response(StatusCode::NOT_FOUND, "404 page not found");
        };
        let Some(host) = self.host.upgrade() else {
            return text_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "terminal stream server is stopping",
            );
        };
        let Some(session) = host.stream_session(&session_id) else {
            return text_response(StatusCode::NOT_FOUND, "404 page not found");
        };
        let token = query_parameter(request.uri().query(), "token").unwrap_or_default();
        if token.is_empty()
            || token
                .as_bytes()
                .ct_eq(session.stream_token().as_bytes())
                .unwrap_u8()
                != 1
        {
            return text_response(StatusCode::UNAUTHORIZED, "terminal stream rejected");
        }
        let Ok(attachment) = session.attach_output() else {
            return text_response(StatusCode::CONFLICT, "terminal stream unavailable");
        };

        let Ok(response) =
            tokio_tungstenite::tungstenite::handshake::server::create_response_with_body(
                &request,
                || Full::new(Bytes::new()),
            )
        else {
            let _ = close_session_blocking(host, session_id, false).await;
            return text_response(StatusCode::BAD_REQUEST, "terminal stream rejected");
        };
        let upgrade = hyper::upgrade::on(&mut request);
        let cancellation = self.cancellation.child_token();
        let claimed_id = session_id;
        let host = self.host.clone();
        self.active.fetch_add(1, Ordering::AcqRel);
        let guard = ActiveGuard {
            active: Arc::clone(&self.active),
            idle: Arc::clone(&self.idle),
        };
        tokio::spawn(async move {
            let _guard = guard;
            if let Ok(Ok(upgraded)) = tokio::time::timeout(STREAM_WRITE_WAIT, upgrade).await {
                let config = WebSocketConfig::default()
                    .max_message_size(Some(MAX_INPUT_FRAME_BYTES))
                    .max_frame_size(Some(MAX_INPUT_FRAME_BYTES));
                let websocket = WebSocketStream::from_raw_socket(
                    TokioIo::new(upgraded),
                    tokio_tungstenite::tungstenite::protocol::Role::Server,
                    Some(config),
                )
                .await;
                run_stream(websocket, session, attachment, cancellation).await;
            }
            if let Some(host) = host.upgrade() {
                let _ = close_session_blocking(host, claimed_id, false).await;
            }
        });
        response
    }

    async fn wait_until_idle(&self) {
        loop {
            let notified = self.idle.notified();
            if self.active.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    /// Stop accepting requests, close active streams, and wait for owned tasks.
    ///
    /// # Errors
    ///
    /// Returns an I/O error from the listener task or if that task panics.
    pub async fn shutdown(&self) -> io::Result<()> {
        self.request_shutdown();
        let task = self
            .serve_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            task.await.map_err(io::Error::other)??;
        } else {
            self.wait_until_idle().await;
        }
        Ok(())
    }

    /// Synchronously reject new requests and cancel listener/stream work.
    pub fn request_shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        self.cancellation.cancel();
    }
}

async fn run_stream<S>(
    websocket: WebSocketStream<S>,
    session: Arc<dyn StreamSession>,
    attachment: StreamAttachment,
    cancellation: CancellationToken,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (sink, stream) = websocket.split();
    let ledger = Arc::new(FlowLedger::new(OUTPUT_WINDOW_BYTES));
    let writer_cancel = cancellation.child_token();
    let reader_cancel = cancellation.child_token();
    let writer =
        write_terminal_stream(sink, Arc::clone(&ledger), attachment, writer_cancel.clone());
    let reader = read_terminal_stream(stream, session, Arc::clone(&ledger), reader_cancel.clone());
    tokio::pin!(writer, reader);
    tokio::select! {
        _ = &mut writer => {
            reader_cancel.cancel();
        }
        _ = &mut reader => {
            writer_cancel.cancel();
        }
        () = cancellation.cancelled() => {
            writer_cancel.cancel();
            reader_cancel.cancel();
        }
    }
}

async fn write_terminal_stream<S>(
    mut sink: S,
    ledger: Arc<FlowLedger>,
    mut attachment: StreamAttachment,
    cancellation: CancellationToken,
) -> Result<(), StreamWireError>
where
    S: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    for chunk in split_output(&attachment.startup) {
        ledger.reserve_pending(chunk.len(), &cancellation).await?;
        send_output(&mut sink, &ledger, chunk).await?;
    }

    let mut ping = tokio::time::interval_at(Instant::now() + STREAM_PING_EVERY, STREAM_PING_EVERY);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        reserve_maximum_with_ping(&mut sink, &ledger, &mut ping, &cancellation).await?;
        tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                ledger.release(OUTPUT_CHUNK_BYTES).await;
                return Err(StreamWireError::Cancelled);
            }
            output = attachment.live.recv() => {
                let Some(output) = output else {
                    ledger.release(OUTPUT_CHUNK_BYTES).await;
                    send_with_deadline(
                        &mut sink,
                        Message::Close(Some(CloseFrame {
                            code: CloseCode::Normal,
                            reason: "".into(),
                        })),
                    )
                    .await?;
                    return Ok(());
                };
                if output.is_empty() || output.len() > OUTPUT_CHUNK_BYTES {
                    ledger.release(OUTPUT_CHUNK_BYTES).await;
                    return Err(StreamWireError::InvalidOutputFrame);
                }
                ledger.release(OUTPUT_CHUNK_BYTES - output.len()).await;
                send_output(&mut sink, &ledger, &output).await?;
            }
            _ = ping.tick() => {
                ledger.release(OUTPUT_CHUNK_BYTES).await;
                send_with_deadline(&mut sink, Message::Ping(Bytes::new())).await?;
            }
        }
    }
}

async fn reserve_maximum_with_ping<S>(
    sink: &mut S,
    ledger: &FlowLedger,
    ping: &mut tokio::time::Interval,
    cancellation: &CancellationToken,
) -> Result<(), StreamWireError>
where
    S: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    loop {
        tokio::select! {
            result = ledger.reserve_pending(OUTPUT_CHUNK_BYTES, cancellation) => {
                result?;
                return Ok(());
            }
            _ = ping.tick() => send_with_deadline(sink, Message::Ping(Bytes::new())).await?,
            () = cancellation.cancelled() => return Err(StreamWireError::Cancelled),
        }
    }
}

async fn send_output<S>(
    sink: &mut S,
    ledger: &FlowLedger,
    output: &[u8],
) -> Result<(), StreamWireError>
where
    S: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    ledger
        .commit(output.len(), || async {
            send_with_deadline(sink, Message::Binary(Bytes::copy_from_slice(output))).await
        })
        .await
}

async fn send_with_deadline<S>(sink: &mut S, message: Message) -> Result<(), StreamWireError>
where
    S: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    tokio::time::timeout(STREAM_WRITE_WAIT, sink.send(message))
        .await
        .map_err(|_| StreamWireError::WriteTimeout)??;
    Ok(())
}

async fn read_terminal_stream<S>(
    mut stream: S,
    session: Arc<dyn StreamSession>,
    ledger: Arc<FlowLedger>,
    cancellation: CancellationToken,
) -> Result<(), StreamWireError>
where
    S: Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let pong_deadline = tokio::time::sleep(STREAM_PONG_WAIT);
    tokio::pin!(pong_deadline);
    loop {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(StreamWireError::Cancelled),
            () = &mut pong_deadline => return Err(StreamWireError::PongTimeout),
            message = stream.next() => {
                match message.ok_or(StreamWireError::ConnectionClosed)?? {
                    Message::Binary(input) => {
                        if input.is_empty() || input.len() > MAX_INPUT_FRAME_BYTES {
                            return Err(StreamWireError::InvalidInputFrame);
                        }
                        let input_session = Arc::clone(&session);
                        tokio::task::spawn_blocking(move || input_session.write_input(&input))
                            .await
                            .map_err(|_| StreamWireError::BlockingWorkerFailed)??;
                    }
                    Message::Text(control) => {
                        let acknowledged = parse_ack_control(control.as_bytes())?;
                        ledger.acknowledge(acknowledged).await?;
                    }
                    Message::Pong(_) => {
                        pong_deadline.as_mut().reset(Instant::now() + STREAM_PONG_WAIT);
                    }
                    Message::Ping(_) => {}
                    Message::Close(_) | Message::Frame(_) => {
                        return Err(StreamWireError::UnsupportedFrame);
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
enum StreamWireError {
    Protocol(crate::ProtocolError),
    Session(StreamSessionError),
    WebSocket(tokio_tungstenite::tungstenite::Error),
    WriteTimeout,
    PongTimeout,
    ConnectionClosed,
    InvalidInputFrame,
    InvalidOutputFrame,
    UnsupportedFrame,
    BlockingWorkerFailed,
    Cancelled,
}

impl From<crate::ProtocolError> for StreamWireError {
    fn from(error: crate::ProtocolError) -> Self {
        Self::Protocol(error)
    }
}

impl From<StreamSessionError> for StreamWireError {
    fn from(error: StreamSessionError) -> Self {
        Self::Session(error)
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for StreamWireError {
    fn from(error: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::WebSocket(error)
    }
}

impl std::fmt::Display for StreamWireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol(error) => write!(formatter, "terminal stream protocol: {error}"),
            Self::Session(error) => write!(formatter, "terminal stream session: {error}"),
            Self::WebSocket(error) => write!(formatter, "terminal WebSocket: {error}"),
            Self::WriteTimeout => formatter.write_str("terminal stream write timed out"),
            Self::PongTimeout => formatter.write_str("terminal stream pong timed out"),
            Self::ConnectionClosed => formatter.write_str("terminal stream connection closed"),
            Self::InvalidInputFrame => formatter.write_str("invalid terminal input frame"),
            Self::InvalidOutputFrame => formatter.write_str("invalid terminal output frame"),
            Self::UnsupportedFrame => formatter.write_str("unsupported terminal stream frame"),
            Self::BlockingWorkerFailed => {
                formatter.write_str("terminal stream blocking worker failed")
            }
            Self::Cancelled => formatter.write_str("terminal stream cancelled"),
        }
    }
}

async fn close_session_blocking(
    host: Arc<dyn StreamSessionHost>,
    session_id: String,
    force: bool,
) -> Result<(), StreamSessionError> {
    tokio::task::spawn_blocking(move || host.close_stream_session(&session_id, force))
        .await
        .map_err(|_| StreamSessionError("terminal session close worker failed".to_owned()))?
}

impl std::error::Error for StreamWireError {}

fn stream_session_id(path: &str) -> Option<&str> {
    let id = path.strip_prefix(STREAM_PATH_PREFIX)?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

fn query_parameter<'a>(query: Option<&'a str>, name: &str) -> Option<&'a str> {
    query?.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == name).then_some(value)
    })
}

fn text_response(status: StatusCode, message: &str) -> Response<ResponseBody> {
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(format!("{message}\n"))))
        .expect("static terminal stream response must be valid")
}

fn allowed_stream_origin(origin: Option<&hyper::header::HeaderValue>) -> bool {
    origin
        .and_then(|value| value.to_str().ok())
        .is_some_and(allowed_stream_origin_str)
}

#[must_use]
pub fn allowed_stream_origin_str(raw_origin: &str) -> bool {
    if raw_origin.is_empty() || raw_origin == "null" || raw_origin.contains('@') {
        return false;
    }
    let Some((scheme, authority)) = raw_origin.split_once("://") else {
        return false;
    };
    if scheme == "wails" {
        return authority == "wails";
    }
    if scheme != "http" && scheme != "https" {
        return false;
    }
    let authority = authority.split('/').next().unwrap_or_default();
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        let Some((host, suffix)) = bracketed.split_once(']') else {
            return false;
        };
        if !suffix.is_empty() && !suffix.starts_with(':') {
            return false;
        }
        host
    } else {
        authority
            .split_once(':')
            .map_or(authority, |(host, _)| host)
    };
    if host.eq_ignore_ascii_case("wails.localhost") || host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}
