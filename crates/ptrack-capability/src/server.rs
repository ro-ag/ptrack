use std::convert::Infallible;
use std::fmt;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::thread::JoinHandle;
use std::time::Duration;

use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, ORIGIN};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use ptrack_agent::{
    process_alive, publish_runtime_json, read_runtime_json, remove_runtime_json_if_equal,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener as TokioTcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::{Broker, BrokerConfig, ToolCall, tool_definitions};

const DESCRIPTOR_NAME: &str = "capability-broker.json";
const MAX_BROKER_BODY_BYTES: usize = 48 << 20;
const READ_HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerDescriptor {
    pub version: u64,
    pub project_root: PathBuf,
    pub generation: u64,
    pub url: String,
    pub pid: i32,
}

#[derive(Clone)]
pub struct BrokerServerConfig {
    pub global_home: PathBuf,
    pub broker: BrokerConfig,
}

/// Host-injected, transport-neutral session environment for task #70 launchers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionEnvironment {
    pub token: String,
    pub project: PathBuf,
    pub generation: u64,
    pub profile: String,
}

impl SessionEnvironment {
    #[must_use]
    pub fn variables(&self) -> Vec<(String, String)> {
        vec![
            ("PTRACK_CAPABILITY_TOKEN".to_owned(), self.token.clone()),
            (
                "PTRACK_CAPABILITY_PROJECT".to_owned(),
                self.project.to_string_lossy().into_owned(),
            ),
            (
                "PTRACK_CAPABILITY_GENERATION".to_owned(),
                self.generation.to_string(),
            ),
            ("PTRACK_CAPABILITY_PROFILE".to_owned(), self.profile.clone()),
        ]
    }
}

/// Loopback-only broker server with a secret-free generation descriptor.
pub struct BrokerServer {
    broker: Arc<Broker>,
    global_home: PathBuf,
    descriptor: BrokerDescriptor,
    descriptor_path: PathBuf,
    shutdown: Mutex<Option<watch::Sender<bool>>>,
    thread: Mutex<Option<JoinHandle<Result<(), ServerError>>>>,
    result: Mutex<Option<Result<(), ServerError>>>,
}

impl BrokerServer {
    /// Starts an IPv4 loopback listener and atomically publishes its descriptor.
    ///
    /// # Errors
    /// Fails closed on broker, listener, runtime-thread, or publication errors.
    pub fn start(config: BrokerServerConfig) -> Result<Self, ServerError> {
        let broker = Arc::new(Broker::new(config.broker).map_err(ServerError::external)?);
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .map_err(|_| ServerError::new("capability broker listener is unavailable"))?;
        listener
            .set_nonblocking(true)
            .map_err(|_| ServerError::new("capability broker listener is unavailable"))?;
        let address = listener
            .local_addr()
            .map_err(|_| ServerError::new("capability broker listener is unavailable"))?;
        let pid = i32::try_from(std::process::id())
            .map_err(|_| ServerError::new("capability broker process ID is invalid"))?;
        let descriptor = BrokerDescriptor {
            version: 1,
            project_root: broker.project_root().to_path_buf(),
            generation: broker.generation(),
            url: format!("http://{address}"),
            pid,
        };
        let (shutdown_sender, shutdown_receiver) = watch::channel(false);
        let thread_broker = Arc::clone(&broker);
        let thread = std::thread::Builder::new()
            .name("ptrack-capability-http".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| ServerError::new("capability broker runtime is unavailable"))?;
                runtime.block_on(serve(listener, thread_broker, shutdown_receiver))
            })
            .map_err(|_| ServerError::new("capability broker thread is unavailable"))?;
        let Ok(descriptor_path) = publish_runtime_json(
            &config.global_home,
            broker.project_root(),
            DESCRIPTOR_NAME,
            &descriptor,
        ) else {
            let _ = shutdown_sender.send(true);
            let _ = thread.join();
            return Err(ServerError::new(
                "capability broker descriptor could not be published",
            ));
        };
        Ok(Self {
            broker,
            global_home: config.global_home,
            descriptor,
            descriptor_path,
            shutdown: Mutex::new(Some(shutdown_sender)),
            thread: Mutex::new(Some(thread)),
            result: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn broker(&self) -> &Arc<Broker> {
        &self.broker
    }

    #[must_use]
    pub const fn descriptor(&self) -> &BrokerDescriptor {
        &self.descriptor
    }

    #[must_use]
    pub fn descriptor_path(&self) -> &Path {
        &self.descriptor_path
    }

    /// Stops serving, revokes authority, and compare-removes only this descriptor.
    ///
    /// # Errors
    /// Returns a stable cleanup error; repeat calls return the same result.
    pub fn shutdown(&self) -> Result<(), ServerError> {
        if let Some(result) = lock(&self.result).clone() {
            return result;
        }
        self.broker.shutdown();
        if let Some(sender) = lock(&self.shutdown).take() {
            let _ = sender.send(true);
        }
        let thread = lock(&self.thread).take();
        let thread_result = thread.map_or(Ok(()), |thread| {
            thread
                .join()
                .map_err(|_| ServerError::new("capability broker thread failed"))?
        });
        let remove_result = remove_runtime_json_if_equal(
            &self.global_home,
            self.broker.project_root(),
            DESCRIPTOR_NAME,
            &self.descriptor,
        )
        .map_err(|_| ServerError::new("capability broker descriptor cleanup failed"));
        let result = thread_result.and(remove_result);
        *lock(&self.result) = Some(result.clone());
        result
    }
}

impl Drop for BrokerServer {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

async fn serve(
    listener: TcpListener,
    broker: Arc<Broker>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ServerError> {
    let listener = TokioTcpListener::from_std(listener)
        .map_err(|_| ServerError::new("capability broker listener is unavailable"))?;
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break; }
            }
            accepted = listener.accept() => {
                let (stream, peer) = accepted.map_err(|_| ServerError::new("capability broker accept failed"))?;
                if !peer.ip().is_loopback() { continue; }
                let broker = Arc::clone(&broker);
                let connection_shutdown = shutdown.clone();
                connections.spawn(async move { serve_connection(stream, broker, connection_shutdown).await; });
            }
            completed = connections.join_next(), if !connections.is_empty() => { let _ = completed; }
        }
    }
    drop(listener);
    let drain = async { while connections.join_next().await.is_some() {} };
    if tokio::time::timeout(SHUTDOWN_TIMEOUT, drain).await.is_err() {
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    }
    Ok(())
}

async fn serve_connection(
    stream: TcpStream,
    broker: Arc<Broker>,
    mut shutdown: watch::Receiver<bool>,
) {
    let _ = stream.set_nodelay(true);
    let (stream, activity, activity_sender) = TimedStream::new(stream);
    let service = service_fn(move |request| {
        let broker = Arc::clone(&broker);
        let activity = activity_sender.clone();
        async move {
            let _active = ActiveRequest::new(activity);
            let response = tokio::time::timeout(REQUEST_TIMEOUT, handle_request(request, broker))
                .await
                .unwrap_or_else(|_| {
                    text_response(StatusCode::REQUEST_TIMEOUT, "capability request timed out")
                });
            Ok::<_, Infallible>(response)
        }
    });
    let mut builder = http1::Builder::new();
    builder
        .keep_alive(true)
        .header_read_timeout(READ_HEADER_TIMEOUT)
        .timer(TokioTimer::new());
    let connection = builder.serve_connection(TokioIo::new(stream), service);
    tokio::pin!(connection);
    tokio::select! {
        _ = &mut connection => {}
        changed = shutdown.changed() => {
            if changed.is_ok() && *shutdown.borrow() {
                connection.as_mut().graceful_shutdown();
                let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut connection).await;
            }
        }
        () = wait_for_idle(activity) => {}
    }
}

type ResponseBody = Full<Bytes>;

async fn handle_request(request: Request<Incoming>, broker: Arc<Broker>) -> Response<ResponseBody> {
    if request.method() != Method::POST || request.headers().contains_key(ORIGIN) {
        return text_response(StatusCode::FORBIDDEN, "capability request rejected");
    }
    let token = bearer_token(&request).to_owned();
    if token.is_empty() || broker.authenticate_token(&token).is_err() {
        return text_response(StatusCode::UNAUTHORIZED, "capability session rejected");
    }
    match request.uri().path() {
        "/v1/tools/list" => json_response(StatusCode::OK, &json!({"tools": tool_definitions()})),
        "/v1/tools/call" => {
            let body = match Limited::new(request.into_body(), MAX_BROKER_BODY_BYTES)
                .collect()
                .await
            {
                Ok(body) => body.to_bytes(),
                Err(_) => {
                    return text_response(StatusCode::BAD_REQUEST, "capability request is invalid");
                }
            };
            let mut deserializer = serde_json::Deserializer::from_slice(&body);
            let Ok(call) = ToolCall::deserialize(&mut deserializer) else {
                return text_response(StatusCode::BAD_REQUEST, "capability request is invalid");
            };
            if deserializer.end().is_err() {
                return text_response(StatusCode::BAD_REQUEST, "capability request is invalid");
            }
            let result = broker.call(&CancellationToken::new(), &token, call).await;
            match result {
                Ok(value) => json_response(StatusCode::OK, &json!({"result": value})),
                Err(error) => {
                    json_response(StatusCode::FORBIDDEN, &json!({"error": error.to_string()}))
                }
            }
        }
        _ => text_response(StatusCode::NOT_FOUND, "capability route is unavailable"),
    }
}

fn bearer_token(request: &Request<Incoming>) -> &str {
    request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or_default()
}

fn text_response(status: StatusCode, message: &str) -> Response<ResponseBody> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header("x-content-type-options", "nosniff")
        .body(Full::new(Bytes::from(format!("{message}\n"))))
        .expect("fixed response")
}

fn json_response(status: StatusCode, value: &Value) -> Response<ResponseBody> {
    let mut bytes = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    bytes.push(b'\n');
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .header("x-content-type-options", "nosniff")
        .body(Full::new(Bytes::from(bytes)))
        .expect("fixed response")
}

/// Reads and validates the active secret-free descriptor for one project.
///
/// # Errors
/// Fails for malformed, stale, cross-project, dead-process, or non-loopback data.
pub fn read_broker_descriptor(
    global_home: &Path,
    project_root: &Path,
) -> Result<BrokerDescriptor, ServerError> {
    let canonical = project_root
        .canonicalize()
        .map_err(|_| ServerError::new("capability project root is unavailable"))?;
    let descriptor: BrokerDescriptor = read_runtime_json(global_home, &canonical, DESCRIPTOR_NAME)
        .map_err(|_| ServerError::new("capability broker descriptor is invalid"))?;
    if descriptor.version != 1
        || descriptor.project_root != canonical
        || !process_alive(descriptor.pid)
    {
        return Err(ServerError::new(
            "capability broker descriptor is stale or belongs to another project",
        ));
    }
    parse_loopback_url(&descriptor.url)?;
    Ok(descriptor)
}

#[derive(Clone)]
pub struct BrokerClient {
    descriptor: BrokerDescriptor,
    client: reqwest::Client,
}

impl BrokerClient {
    /// Creates a proxy-independent client for an already validated descriptor.
    ///
    /// # Errors
    /// Fails when the fixed local HTTP client cannot be built.
    pub fn new(descriptor: BrokerDescriptor) -> Result<Self, ServerError> {
        parse_loopback_url(&descriptor.url)?;
        crate::http::ensure_tls_provider()
            .map_err(|_| ServerError::new("capability broker client is unavailable"))?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ServerError::new("capability broker client is unavailable"))?;
        Ok(Self { descriptor, client })
    }

    #[must_use]
    pub const fn descriptor(&self) -> &BrokerDescriptor {
        &self.descriptor
    }

    /// Calls one broker tool and returns its exact structured result.
    ///
    /// # Errors
    /// Enforces the 48 MiB response limit and returns the broker's sanitized error.
    pub async fn call(&self, token: &str, call: &ToolCall) -> Result<Value, ServerError> {
        self.call_cancellable(&CancellationToken::new(), token, call)
            .await
    }

    /// Calls one broker tool while observing an owned session cancellation.
    ///
    /// # Errors
    /// Returns a stable cancellation, request, response, or broker error.
    pub async fn call_cancellable(
        &self,
        cancellation: &CancellationToken,
        token: &str,
        call: &ToolCall,
    ) -> Result<Value, ServerError> {
        let body = serde_json::to_vec(call)
            .map_err(|_| ServerError::new("capability broker request is invalid"))?;
        let request = self
            .client
            .post(format!("{}/v1/tools/call", self.descriptor.url))
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send();
        let response = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                return Err(ServerError::new("capability broker request cancelled"));
            }
            response = request => response
                .map_err(|_| ServerError::new("capability broker request failed"))?,
        };
        let status = response.status();
        let mut response = response;
        let mut bytes = Vec::new();
        loop {
            let chunk = tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    return Err(ServerError::new("capability broker request cancelled"));
                }
                chunk = response.chunk() => chunk
                    .map_err(|_| ServerError::new("capability broker response is invalid"))?,
            };
            let Some(chunk) = chunk else { break };
            if bytes.len().saturating_add(chunk.len()) > MAX_BROKER_BODY_BYTES {
                return Err(ServerError::new("capability broker response is too large"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let envelope: ClientEnvelope = serde_json::from_slice(&bytes)
            .map_err(|_| ServerError::new("capability broker response is invalid"))?;
        if status != reqwest::StatusCode::OK {
            return Err(ServerError::new(if envelope.error.is_empty() {
                "capability broker rejected the request".to_owned()
            } else {
                envelope.error
            }));
        }
        envelope
            .result
            .ok_or_else(|| ServerError::new("capability broker response is invalid"))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientEnvelope {
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: String,
}

/// Loads a validated local client for one canonical project.
///
/// # Errors
/// Returns descriptor or client-validation errors.
pub fn client_for_project(
    global_home: &Path,
    project_root: &Path,
) -> Result<BrokerClient, ServerError> {
    BrokerClient::new(read_broker_descriptor(global_home, project_root)?)
}

/// Enforces the project and generation fences injected by the host launcher.
///
/// # Errors
/// Fails before any request when an injected fence differs.
pub fn validate_session_environment(
    descriptor: &BrokerDescriptor,
    project: Option<&Path>,
    generation: Option<&str>,
) -> Result<(), ServerError> {
    if let Some(project) = project {
        let project = project.canonicalize().map_err(|_| {
            ServerError::new("capability broker project does not match the launched session")
        })?;
        if project != descriptor.project_root {
            return Err(ServerError::new(
                "capability broker project does not match the launched session",
            ));
        }
    }
    if generation.is_some_and(|value| value != descriptor.generation.to_string()) {
        return Err(ServerError::new(
            "capability broker generation does not match the launched session",
        ));
    }
    Ok(())
}

fn parse_loopback_url(value: &str) -> Result<SocketAddrV4, ServerError> {
    let authority = value
        .strip_prefix("http://")
        .filter(|value| {
            !value.is_empty()
                && !value.contains(['/', '?', '#', '@'])
                && !value.chars().any(char::is_whitespace)
        })
        .ok_or_else(|| ServerError::new("capability broker descriptor URL is invalid"))?;
    let address: SocketAddr = authority
        .parse()
        .map_err(|_| ServerError::new("capability broker descriptor URL is invalid"))?;
    match address {
        SocketAddr::V4(address) if address.ip().is_loopback() => Ok(address),
        _ => Err(ServerError::new(
            "capability broker descriptor URL is invalid",
        )),
    }
}

struct TimedStream {
    stream: TcpStream,
    write_timeout: Pin<Box<tokio::time::Sleep>>,
    activity: watch::Sender<ConnectionActivity>,
}

impl TimedStream {
    fn new(
        stream: TcpStream,
    ) -> (
        Self,
        watch::Receiver<ConnectionActivity>,
        watch::Sender<ConnectionActivity>,
    ) {
        let (activity, receiver) =
            watch::channel(ConnectionActivity::Idle(tokio::time::Instant::now()));
        (
            Self {
                stream,
                write_timeout: Box::pin(tokio::time::sleep(REQUEST_TIMEOUT)),
                activity: activity.clone(),
            },
            receiver,
            activity,
        )
    }

    fn record_activity(&self) {
        self.activity.send_if_modified(|activity| {
            let ConnectionActivity::Idle(last) = activity else {
                return false;
            };
            *last = tokio::time::Instant::now();
            true
        });
    }

    fn reset_write_timeout(&mut self) {
        self.write_timeout
            .as_mut()
            .reset(tokio::time::Instant::now() + REQUEST_TIMEOUT);
    }
}

impl AsyncRead for TimedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(context, buffer);
        if matches!(result, Poll::Ready(Ok(()))) && buffer.filled().len() > before {
            self.record_activity();
        }
        result
    }
}

impl AsyncWrite for TimedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.stream).poll_write(context, bytes) {
            Poll::Ready(result) => {
                if result.as_ref().is_ok_and(|written| *written > 0) {
                    self.record_activity();
                }
                self.reset_write_timeout();
                Poll::Ready(result)
            }
            Poll::Pending => match self.write_timeout.as_mut().poll(context) {
                Poll::Ready(()) => Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "capability response write timed out",
                ))),
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(context)
    }
}

#[derive(Clone, Copy, Debug)]
enum ConnectionActivity {
    Active,
    Idle(tokio::time::Instant),
}

struct ActiveRequest(watch::Sender<ConnectionActivity>);

impl ActiveRequest {
    fn new(activity: watch::Sender<ConnectionActivity>) -> Self {
        activity.send_replace(ConnectionActivity::Active);
        Self(activity)
    }
}

impl Drop for ActiveRequest {
    fn drop(&mut self) {
        self.0
            .send_replace(ConnectionActivity::Idle(tokio::time::Instant::now()));
    }
}

async fn wait_for_idle(mut activity: watch::Receiver<ConnectionActivity>) {
    loop {
        let current = *activity.borrow_and_update();
        match current {
            ConnectionActivity::Active => {
                if activity.changed().await.is_err() {
                    return;
                }
            }
            ConnectionActivity::Idle(last) => {
                let deadline = last + IDLE_TIMEOUT;
                tokio::select! {
                    () = tokio::time::sleep_until(deadline) => {
                        if matches!(
                            *activity.borrow(),
                            ConnectionActivity::Idle(current)
                                if tokio::time::Instant::now().duration_since(current) >= IDLE_TIMEOUT
                        ) {
                            return;
                        }
                    }
                    changed = activity.changed() => if changed.is_err() { return; }
                }
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerError(String);

impl ServerError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    fn external(error: impl fmt::Display) -> Self {
        Self(error.to_string())
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ServerError {}
