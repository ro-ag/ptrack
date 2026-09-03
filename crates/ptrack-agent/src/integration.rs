use std::convert::Infallible;
use std::fmt;
use std::future::Future;
use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, ORIGIN};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener as TokioTcpListener, TcpStream};
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;

use crate::persistence::{absolute_clean, remove_integration_descriptor_if_owned};
use crate::{
    AgentHandoffInbox, AgentObservation, AgentRunObservationV1, AgentRunsV2, CoordinationError,
    Event, IntegrationDescriptor, ProviderEvent, Registration, Registry, RegistryError, Timestamp,
    publish_runtime_json, read_integration_descriptor,
};

const MAX_INTEGRATION_BODY_BYTES: usize = 16 * 1_024;
const MAX_OBSERVATION_RESPONSE_BYTES: usize = 1_024 * 1_024;
const INTEGRATION_READ_TIMEOUT: Duration = Duration::from_secs(5);
const INTEGRATION_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const INTEGRATION_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const LAUNCHED_EVENT_BIND_WAIT: Duration = Duration::from_secs(2);
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PENDING_EVENT_WAITS: usize = 8;
const DESCRIPTOR_NAME: &str = "agent-registry.json";

type ResponseBody = Full<Bytes>;
type ThreadFactory = Arc<
    dyn Fn(
            SocketAddr,
            Box<dyn FnOnce() -> Result<(), IntegrationError> + Send>,
        ) -> Result<JoinHandle<Result<(), IntegrationError>>, IntegrationError>
        + Send
        + Sync,
>;

#[derive(Clone, Default)]
pub struct IntegrationConfig {
    pub global_home: PathBuf,
    pub project_root: PathBuf,
    pub generation: u64,
    /// Live generation owner used only for authenticated read-only projections.
    pub observer: Option<Arc<dyn AgentObservation>>,
    /// Host-owned exact mutation counter; unlike the bounded refresh channel,
    /// it cannot lose increments under notification pressure.
    pub mutation_revision: Option<Arc<AtomicU64>>,
    /// Bounded host-owned presentation invalidation carrying no payload or authority.
    pub runtime_changed: Option<SyncSender<()>>,
    #[cfg(test)]
    pub(crate) thread_factory: Option<ThreadFactory>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegrationError(pub(crate) String);

impl fmt::Display for IntegrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for IntegrationError {}

struct ServerState {
    registry: Arc<Registry>,
    observer: Option<Arc<dyn AgentObservation>>,
    registration_token: String,
    mutation_revision: Option<Arc<AtomicU64>>,
    runtime_changed: Option<SyncSender<()>>,
    event_wait_slots: Arc<Semaphore>,
}

pub struct IntegrationServer {
    global_home: PathBuf,
    project_root: PathBuf,
    generation: u64,
    registration_token: String,
    descriptor_path: PathBuf,
    event_endpoint: String,
    shutdown_sender: Mutex<Option<watch::Sender<bool>>>,
    serve_thread: Mutex<Option<JoinHandle<Result<(), IntegrationError>>>>,
    shutdown_guard: Mutex<()>,
    shutdown_result: Mutex<Option<Result<(), IntegrationError>>>,
}

impl IntegrationServer {
    /// Starts a loopback-only `AgentRun` integration server and publishes its
    /// private discovery descriptor.
    ///
    /// # Errors
    /// Returns path-resolution, listener, runtime, descriptor, or thread errors.
    pub fn start(
        registry: Arc<Registry>,
        config: IntegrationConfig,
    ) -> Result<Self, IntegrationError> {
        let project_root = absolute_clean(&config.project_root, "resolve AgentRun project root")
            .map_err(integration_error)?;
        let global_home = absolute_clean(&config.global_home, "resolve AgentRun runtime home")
            .map_err(integration_error)?;
        let registration_token = random_opaque_value()?;
        let listener =
            TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).map_err(|error| {
                IntegrationError(format!("listen for AgentRun integration: {error}"))
            })?;
        listener
            .set_nonblocking(true)
            .map_err(|error| IntegrationError(format!("configure AgentRun listener: {error}")))?;
        let address = listener
            .local_addr()
            .map_err(|error| IntegrationError(format!("inspect AgentRun listener: {error}")))?;
        let url = format!("http://{address}");
        let pid = i32::try_from(std::process::id())
            .map_err(|_| IntegrationError("AgentRun process ID exceeds i32".to_owned()))?;
        let descriptor = IntegrationDescriptor {
            project_root: project_root.to_string_lossy().into_owned(),
            url: url.clone(),
            generation: config.generation,
            registration_token: registration_token.clone(),
            pid,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| IntegrationError(format!("create AgentRun HTTP runtime: {error}")))?;
        let (shutdown_sender, shutdown_receiver) = watch::channel(false);
        let state = Arc::new(ServerState {
            registry,
            observer: config.observer,
            registration_token: registration_token.clone(),
            mutation_revision: config.mutation_revision,
            runtime_changed: config.runtime_changed,
            event_wait_slots: Arc::new(Semaphore::new(MAX_PENDING_EVENT_WAITS)),
        });
        #[cfg(test)]
        let thread_factory = config.thread_factory.unwrap_or_else(default_thread_factory);
        #[cfg(not(test))]
        let thread_factory = default_thread_factory();
        let serve_thread = thread_factory(
            address,
            Box::new(move || runtime.block_on(serve(listener, state, shutdown_receiver))),
        )?;
        let descriptor_path =
            match publish_runtime_json(&global_home, &project_root, DESCRIPTOR_NAME, &descriptor) {
                Ok(path) => path,
                Err(error) => {
                    let _ = shutdown_sender.send(true);
                    let _ = serve_thread.join();
                    return Err(integration_error(error));
                }
            };
        Ok(Self {
            global_home,
            project_root,
            generation: config.generation,
            registration_token,
            descriptor_path,
            event_endpoint: format!("{url}/v1/events"),
            shutdown_sender: Mutex::new(Some(shutdown_sender)),
            serve_thread: Mutex::new(Some(serve_thread)),
            shutdown_guard: Mutex::new(()),
            shutdown_result: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn descriptor_path(&self) -> &Path {
        &self.descriptor_path
    }

    /// Returns the run-ID-free endpoint injected into host-launched agents.
    #[must_use]
    pub fn event_endpoint(&self) -> &str {
        &self.event_endpoint
    }

    /// Gracefully stops HTTP work within two seconds and compare-removes only
    /// this generation's descriptor.
    ///
    /// # Errors
    /// Joins serving, thread, and descriptor cleanup failures.
    pub fn shutdown(&self) -> Result<(), IntegrationError> {
        self.shutdown_inner(None)
    }

    /// Requests shutdown and waits no longer than `timeout` for the owned
    /// serving thread. A timed-out thread remains owned and is joined by a
    /// later shutdown or drop; it is never detached.
    ///
    /// # Errors
    /// Returns a stable cleanup error or a bounded timeout error.
    pub fn shutdown_timeout(&self, timeout: Duration) -> Result<(), IntegrationError> {
        self.shutdown_inner(Some(timeout))
    }

    fn shutdown_inner(&self, timeout: Option<Duration>) -> Result<(), IntegrationError> {
        let deadline = timeout.map(|value| Instant::now() + value);
        let _shutdown_guard = lock(&self.shutdown_guard);
        if let Some(result) = lock(&self.shutdown_result).clone() {
            return result;
        }
        if let Some(sender) = lock(&self.shutdown_sender).take() {
            let _ = sender.send(true);
        }
        let mut failures = Vec::new();
        loop {
            let finished = lock(&self.serve_thread)
                .as_ref()
                .is_none_or(std::thread::JoinHandle::is_finished);
            if finished {
                break;
            }
            if deadline.is_some_and(|value| Instant::now() >= value) {
                return Err(IntegrationError(
                    "AgentRun integration shutdown timed out".to_owned(),
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if let Some(thread) = lock(&self.serve_thread).take() {
            match thread.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(error.to_string()),
                Err(_) => failures.push("AgentRun HTTP server thread panicked".to_owned()),
            }
        }
        if let Err(error) = remove_integration_descriptor_if_owned(
            &self.global_home,
            &self.project_root,
            self.generation,
            &self.registration_token,
        ) {
            failures.push(error.to_string());
        }
        let result = if failures.is_empty() {
            Ok(())
        } else {
            Err(IntegrationError(failures.join("\n")))
        };
        *lock(&self.shutdown_result) = Some(result.clone());
        result
    }
}

impl Drop for IntegrationServer {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Starts a loopback-only `AgentRun` integration server.
///
/// # Errors
/// Returns the same errors as [`IntegrationServer::start`].
pub fn start_integration_server(
    registry: Arc<Registry>,
    config: IntegrationConfig,
) -> Result<IntegrationServer, IntegrationError> {
    IntegrationServer::start(registry, config)
}

/// Proxy-independent observer for one validated live coordination host.
#[derive(Clone, Debug)]
pub struct AgentObservationClient {
    descriptor: IntegrationDescriptor,
    address: SocketAddr,
}

impl AgentObservationClient {
    /// Loads and validates the private descriptor for one canonical project.
    ///
    /// # Errors
    /// Refuses missing, stale, cross-project, non-loopback, or malformed data. The
    /// live descriptor owns the runtime generation used for each request.
    pub fn for_project(global_home: &Path, project_root: &Path) -> Result<Self, IntegrationError> {
        let canonical = project_root.canonicalize().map_err(|_| {
            IntegrationError("agent coordination project is unavailable".to_owned())
        })?;
        let descriptor = read_integration_descriptor(global_home, project_root)
            .map_err(|_| IntegrationError("no active agent coordination host".to_owned()))?;
        let descriptor_root = Path::new(&descriptor.project_root)
            .canonicalize()
            .map_err(|_| {
                IntegrationError("agent coordination host descriptor is invalid".to_owned())
            })?;
        if descriptor_root != canonical {
            return Err(IntegrationError(
                "agent coordination host belongs to another project".to_owned(),
            ));
        }
        if descriptor.registration_token.is_empty()
            || descriptor
                .registration_token
                .bytes()
                .any(|byte| byte.is_ascii_control())
        {
            return Err(IntegrationError(
                "agent coordination host descriptor is invalid".to_owned(),
            ));
        }
        let address = loopback_address(&descriptor.url)?;
        Ok(Self {
            descriptor,
            address,
        })
    }

    /// Returns the bounded live run registry projection.
    ///
    /// # Errors
    /// Returns a bounded connection, authentication, generation, or response error.
    pub fn runs(&self) -> Result<AgentRunsV2, IntegrationError> {
        self.call(
            "/v1/observe/runs",
            &ObservationRequest {
                generation: self.descriptor.generation,
                run_id: String::new(),
            },
        )
    }

    /// Returns one sanitized live run and its inferred intelligence.
    ///
    /// # Errors
    /// Returns not-found or a bounded host/response error.
    pub fn run(&self, run_id: &str) -> Result<AgentRunObservationV1, IntegrationError> {
        self.call(
            "/v1/observe/run",
            &ObservationRequest {
                generation: self.descriptor.generation,
                run_id: run_id.to_owned(),
            },
        )
    }

    /// Returns the live memory-only handoff inbox.
    ///
    /// # Errors
    /// Returns a bounded host or response error.
    pub fn inbox(&self) -> Result<AgentHandoffInbox, IntegrationError> {
        self.call(
            "/v1/observe/inbox",
            &ObservationRequest {
                generation: self.descriptor.generation,
                run_id: String::new(),
            },
        )
    }

    fn call<T: DeserializeOwned>(
        &self,
        path: &str,
        request: &ObservationRequest,
    ) -> Result<T, IntegrationError> {
        let body = serde_json::to_vec(request)
            .map_err(|_| IntegrationError("agent coordination request is invalid".to_owned()))?;
        let mut stream = StdTcpStream::connect_timeout(&self.address, INTEGRATION_READ_TIMEOUT)
            .map_err(|_| IntegrationError("agent coordination host is unavailable".to_owned()))?;
        stream
            .set_read_timeout(Some(INTEGRATION_READ_TIMEOUT))
            .map_err(|_| IntegrationError("agent coordination host is unavailable".to_owned()))?;
        stream
            .set_write_timeout(Some(INTEGRATION_WRITE_TIMEOUT))
            .map_err(|_| IntegrationError("agent coordination host is unavailable".to_owned()))?;
        let mut wire = Vec::with_capacity(body.len().saturating_add(512));
        write!(
            wire,
            "POST {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.address,
            self.descriptor.registration_token,
            body.len()
        )
        .map_err(|_| IntegrationError("agent coordination request is invalid".to_owned()))?;
        wire.extend_from_slice(&body);
        stream
            .write_all(&wire)
            .map_err(|_| IntegrationError("agent coordination request failed".to_owned()))?;
        let mut response = Vec::new();
        stream
            .take((MAX_OBSERVATION_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut response)
            .map_err(|_| IntegrationError("agent coordination response is invalid".to_owned()))?;
        if response.len() > MAX_OBSERVATION_RESPONSE_BYTES {
            return Err(IntegrationError(
                "agent coordination response is too large".to_owned(),
            ));
        }
        decode_observation_response(&response)
    }
}

fn loopback_address(url: &str) -> Result<SocketAddr, IntegrationError> {
    let address = url
        .strip_prefix("http://")
        .filter(|value| !value.contains('/'))
        .and_then(|value| value.parse::<SocketAddr>().ok())
        .filter(|value| value.ip().is_loopback())
        .ok_or_else(|| {
            IntegrationError("agent coordination host descriptor is invalid".to_owned())
        })?;
    Ok(address)
}

fn decode_observation_response<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, IntegrationError> {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| IntegrationError("agent coordination response is invalid".to_owned()))?;
    let headers = std::str::from_utf8(&bytes[..split])
        .map_err(|_| IntegrationError("agent coordination response is invalid".to_owned()))?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| IntegrationError("agent coordination response is invalid".to_owned()))?;
    match status {
        200 => serde_json::from_slice(&bytes[split + 4..])
            .map_err(|_| IntegrationError("agent coordination response is invalid".to_owned())),
        404 => Err(IntegrationError("AgentRun not found".to_owned())),
        409 => Err(IntegrationError(
            "agent coordination host generation changed".to_owned(),
        )),
        _ => Err(IntegrationError(
            "agent coordination host rejected the request".to_owned(),
        )),
    }
}

async fn serve(
    listener: TcpListener,
    state: Arc<ServerState>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), IntegrationError> {
    let listener = TokioTcpListener::from_std(listener)
        .map_err(|error| IntegrationError(format!("serve AgentRun integration: {error}")))?;
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted
                    .map_err(|error| IntegrationError(format!("accept AgentRun integration: {error}")))?;
                let state = Arc::clone(&state);
                let connection_shutdown = shutdown.clone();
                connections.spawn(async move {
                    serve_connection(stream, state, connection_shutdown).await;
                });
            }
            completed = connections.join_next(), if !connections.is_empty() => {
                let _ = completed;
            }
        }
    }
    drop(listener);
    let drain = async { while connections.join_next().await.is_some() {} };
    if tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, drain)
        .await
        .is_err()
    {
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    }
    Ok(())
}

async fn serve_connection(
    stream: TcpStream,
    state: Arc<ServerState>,
    mut shutdown: watch::Receiver<bool>,
) {
    let _ = stream.set_nodelay(true);
    let (stream, activity, read_budget) = TimedStream::new(stream);
    let service = service_fn(move |request| {
        let state = Arc::clone(&state);
        let read_deadline = read_budget.deadline();
        async move {
            let response = tokio::time::timeout(
                INTEGRATION_WRITE_TIMEOUT,
                handle_request(request, state, read_deadline),
            )
            .await
            .unwrap_or_else(|_| {
                error_response(StatusCode::REQUEST_TIMEOUT, "AgentRun request timed out")
            });
            Ok::<_, Infallible>(response)
        }
    });
    let mut builder = http1::Builder::new();
    builder
        .keep_alive(true)
        .header_read_timeout(INTEGRATION_READ_TIMEOUT)
        .timer(TokioTimer::new());
    let connection = builder.serve_connection(TokioIo::new(stream), service);
    tokio::pin!(connection);
    tokio::select! {
        _ = &mut connection => {}
        changed = shutdown.changed() => {
            if changed.is_ok() && *shutdown.borrow() {
                connection.as_mut().graceful_shutdown();
                let _ = tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, &mut connection).await;
            }
        }
        () = wait_for_idle(activity) => {}
    }
}

struct TimedStream {
    stream: TcpStream,
    write_timeout: Pin<Box<tokio::time::Sleep>>,
    activity: watch::Sender<tokio::time::Instant>,
    read_budget: ReadBudget,
}

impl TimedStream {
    fn new(stream: TcpStream) -> (Self, watch::Receiver<tokio::time::Instant>, ReadBudget) {
        let now = tokio::time::Instant::now();
        let (activity, receiver) = watch::channel(now);
        let read_budget = ReadBudget::default();
        (
            Self {
                stream,
                write_timeout: Box::pin(tokio::time::sleep(INTEGRATION_WRITE_TIMEOUT)),
                activity,
                read_budget: read_budget.clone(),
            },
            receiver,
            read_budget,
        )
    }

    fn reset_timeout(&mut self) {
        self.write_timeout
            .as_mut()
            .reset(tokio::time::Instant::now() + INTEGRATION_WRITE_TIMEOUT);
    }

    fn record_activity(&self) {
        self.activity.send_replace(tokio::time::Instant::now());
    }
}

impl AsyncRead for TimedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.read_budget.start();
        let previous = buffer.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(context, buffer);
        if matches!(result, Poll::Ready(Ok(()))) && buffer.filled().len() > previous {
            self.record_activity();
        }
        result
    }
}

impl AsyncWrite for TimedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.stream).poll_write(context, buffer) {
            Poll::Ready(result) => {
                if result.as_ref().is_ok_and(|written| *written > 0) {
                    self.record_activity();
                    self.read_budget.finish();
                }
                self.reset_timeout();
                Poll::Ready(result)
            }
            Poll::Pending => match self.write_timeout.as_mut().poll(context) {
                Poll::Ready(()) => Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "AgentRun response write timed out",
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

#[derive(Clone, Default)]
struct ReadBudget(Arc<Mutex<Option<tokio::time::Instant>>>);

impl ReadBudget {
    fn start(&self) {
        let mut started = lock(&self.0);
        if started.is_none() {
            *started = Some(tokio::time::Instant::now());
        }
    }

    fn deadline(&self) -> tokio::time::Instant {
        lock(&self.0).unwrap_or_else(tokio::time::Instant::now) + INTEGRATION_READ_TIMEOUT
    }

    fn finish(&self) {
        *lock(&self.0) = None;
    }
}

async fn wait_for_idle(mut activity: watch::Receiver<tokio::time::Instant>) {
    loop {
        let deadline = *activity.borrow_and_update() + INTEGRATION_IDLE_TIMEOUT;
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => {
                if *activity.borrow() <= deadline - INTEGRATION_IDLE_TIMEOUT {
                    return;
                }
            }
            changed = activity.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Route<'request> {
    Register,
    ObserveRuns,
    ObserveRun,
    ObserveInbox,
    Run {
        id: &'request str,
        action: &'request str,
    },
    LaunchedEvent,
    NotFound,
}

fn route(path: &str) -> Route<'_> {
    match path {
        "/v1/runs/register" => Route::Register,
        "/v1/events" => Route::LaunchedEvent,
        "/v1/observe/runs" => Route::ObserveRuns,
        "/v1/observe/run" => Route::ObserveRun,
        "/v1/observe/inbox" => Route::ObserveInbox,
        _ => {
            let Some(rest) = path.strip_prefix("/v1/runs/") else {
                return Route::NotFound;
            };
            let Some((id, action)) = rest.split_once('/') else {
                return Route::NotFound;
            };
            if id.is_empty() || action.contains('/') {
                return Route::NotFound;
            }
            Route::Run { id, action }
        }
    }
}

async fn handle_request(
    request: Request<Incoming>,
    state: Arc<ServerState>,
    read_deadline: tokio::time::Instant,
) -> Response<ResponseBody> {
    let path = request.uri().path().to_owned();
    let route = route(&path);
    if matches!(route, Route::NotFound) {
        return error_response(StatusCode::NOT_FOUND, "404 page not found");
    }
    match route {
        Route::Register => {
            if rejected_method_or_origin(&request) {
                error_response(StatusCode::FORBIDDEN, "AgentRun request rejected")
            } else {
                handle_register(request, &state, read_deadline).await
            }
        }
        Route::ObserveRuns | Route::ObserveRun | Route::ObserveInbox => {
            handle_observation(route, request, &state, read_deadline).await
        }
        Route::Run { id, action } => handle_run(id, action, request, &state, read_deadline).await,
        Route::LaunchedEvent => {
            if rejected_method_or_origin(&request) {
                error_response(StatusCode::FORBIDDEN, "AgentRun request rejected")
            } else {
                handle_launched_event(request, &state, read_deadline).await
            }
        }
        Route::NotFound => unreachable!(),
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ObservationRequest {
    generation: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    run_id: String,
}

async fn handle_observation(
    route: Route<'_>,
    request: Request<Incoming>,
    state: &ServerState,
    read_deadline: tokio::time::Instant,
) -> Response<ResponseBody> {
    if rejected_method_or_origin(&request) {
        return error_response(StatusCode::FORBIDDEN, "AgentRun request rejected");
    }
    if !authorized(&request, &state.registration_token) {
        return error_response(StatusCode::UNAUTHORIZED, "AgentRun request rejected");
    }
    let Some(observer) = &state.observer else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "AgentRun observation unavailable",
        );
    };
    let observation: ObservationRequest = match decode_json(request, read_deadline).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let result = match route {
        Route::ObserveRuns => observer
            .observe_runs(observation.generation)
            .map(|value| json_response(StatusCode::OK, &value)),
        Route::ObserveRun if !observation.run_id.is_empty() => observer
            .observe_run(observation.generation, &observation.run_id)
            .map(|value| json_response(StatusCode::OK, &value)),
        Route::ObserveInbox => observer
            .observe_handoffs(observation.generation)
            .map(|value| json_response(StatusCode::OK, &value)),
        Route::ObserveRun => Err(CoordinationError::RunNotFound),
        _ => unreachable!(),
    };
    result.unwrap_or_else(|error| observation_error_response(&error))
}

fn observation_error_response(error: &CoordinationError) -> Response<ResponseBody> {
    match error {
        CoordinationError::RunNotFound => {
            error_response(StatusCode::NOT_FOUND, "AgentRun not found")
        }
        CoordinationError::StaleGeneration { .. } => error_response(
            StatusCode::CONFLICT,
            "AgentRun observation generation changed",
        ),
        _ => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "AgentRun observation unavailable",
        ),
    }
}

async fn handle_run(
    id: &str,
    action: &str,
    request: Request<Incoming>,
    state: &ServerState,
    read_deadline: tokio::time::Instant,
) -> Response<ResponseBody> {
    let Some(token) = bearer_token(&request).map(str::to_owned) else {
        return error_response(StatusCode::UNAUTHORIZED, "AgentRun lease rejected");
    };
    if rejected_method_or_origin(&request) {
        return error_response(StatusCode::FORBIDDEN, "AgentRun request rejected");
    }
    match action {
        "heartbeat" => handle_heartbeat(id, &token, state),
        "exit" => handle_exit(id, &token, request, state, read_deadline).await,
        "events" => handle_external_event(id, &token, request, state, read_deadline).await,
        _ => error_response(StatusCode::NOT_FOUND, "404 page not found"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExternalRegistration {
    #[serde(default)]
    profile: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    pid: i32,
    #[serde(default)]
    cwd: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LeaseReceipt<'receipt> {
    id: &'receipt str,
    lease_token: &'receipt str,
}

async fn handle_register(
    request: Request<Incoming>,
    state: &ServerState,
    read_deadline: tokio::time::Instant,
) -> Response<ResponseBody> {
    if !authorized(&request, &state.registration_token) {
        return error_response(StatusCode::UNAUTHORIZED, "AgentRun request rejected");
    }
    let registration: ExternalRegistration = match decode_json(request, read_deadline).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(lease) = state.registry.register_external(Registration {
        profile: registration.profile,
        provider: registration.provider,
        pid: registration.pid,
        terminal_id: String::new(),
        cwd: registration.cwd,
    }) else {
        return error_response(StatusCode::BAD_REQUEST, "AgentRun registration rejected");
    };
    notify_runtime_changed(state);
    json_response(
        StatusCode::CREATED,
        &LeaseReceipt {
            id: &lease.run.id,
            lease_token: &lease.lease_token,
        },
    )
}

fn handle_heartbeat(id: &str, token: &str, state: &ServerState) -> Response<ResponseBody> {
    if state.registry.heartbeat(id, token).is_err() {
        return error_response(StatusCode::UNAUTHORIZED, "AgentRun lease rejected");
    }
    notify_runtime_changed(state);
    empty_response(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExitRequest {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    result: String,
}

async fn handle_exit(
    id: &str,
    token: &str,
    request: Request<Incoming>,
    state: &ServerState,
    read_deadline: tokio::time::Instant,
) -> Response<ResponseBody> {
    if state.registry.authenticate_event_lease(id, token).is_err() {
        return error_response(StatusCode::UNAUTHORIZED, "AgentRun lease rejected");
    }
    let exit: ExitRequest = match decode_json(request, read_deadline).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if state
        .registry
        .exit_external(id, token, exit.code, &exit.result)
        .is_err()
    {
        return error_response(StatusCode::UNAUTHORIZED, "AgentRun lease rejected");
    }
    notify_runtime_changed(state);
    empty_response(StatusCode::NO_CONTENT)
}

async fn handle_external_event(
    id: &str,
    token: &str,
    request: Request<Incoming>,
    state: &ServerState,
    read_deadline: tokio::time::Instant,
) -> Response<ResponseBody> {
    if state.registry.authenticate_event_lease(id, token).is_err() {
        return error_response(StatusCode::UNAUTHORIZED, "AgentRun lease rejected");
    }
    let provider_event: ProviderEvent = match decode_json(request, read_deadline).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state
        .registry
        .record_provider_event(id, token, provider_event)
    {
        Ok(event) => {
            notify_runtime_changed(state);
            event_receipt(&event)
        }
        Err(RegistryError::InvalidLease | RegistryError::RunNotFound) => {
            error_response(StatusCode::UNAUTHORIZED, "AgentRun lease rejected")
        }
        Err(_) => error_response(StatusCode::BAD_REQUEST, "AgentRun event rejected"),
    }
}

async fn handle_launched_event(
    request: Request<Incoming>,
    state: &ServerState,
    read_deadline: tokio::time::Instant,
) -> Response<ResponseBody> {
    let Some(token) = bearer_token(&request).map(str::to_owned) else {
        return error_response(StatusCode::UNAUTHORIZED, "AgentRun event token rejected");
    };
    if state
        .registry
        .authenticate_launched_event_token(&token)
        .is_ok()
    {
        return decode_and_record_launched_event(request, state, read_deadline, &token).await;
    }
    let Ok(wait_slot) = Arc::clone(&state.event_wait_slots).try_acquire_owned() else {
        return error_response(StatusCode::UNAUTHORIZED, "AgentRun event token rejected");
    };
    let registry = Arc::clone(&state.registry);
    let wait_token = token.clone();
    let mut wait = AbortOnDrop::new(tokio::task::spawn_blocking(move || {
        let _wait_slot = wait_slot;
        registry.await_launched_event_token(&wait_token, LAUNCHED_EVENT_BIND_WAIT)
    }));
    if !matches!(
        tokio::time::timeout(LAUNCHED_EVENT_BIND_WAIT, &mut wait).await,
        Ok(Ok(Ok(())))
    ) {
        return error_response(StatusCode::UNAUTHORIZED, "AgentRun event token rejected");
    }
    decode_and_record_launched_event(request, state, read_deadline, &token).await
}

async fn decode_and_record_launched_event(
    request: Request<Incoming>,
    state: &ServerState,
    read_deadline: tokio::time::Instant,
    token: &str,
) -> Response<ResponseBody> {
    let provider_event: ProviderEvent = match decode_json(request, read_deadline).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state
        .registry
        .record_launched_provider_event(token, provider_event)
    {
        Ok(event) => {
            notify_runtime_changed(state);
            event_receipt(&event)
        }
        Err(RegistryError::InvalidEventToken | RegistryError::RunNotFound) => {
            error_response(StatusCode::UNAUTHORIZED, "AgentRun event token rejected")
        }
        Err(_) => error_response(StatusCode::BAD_REQUEST, "AgentRun event rejected"),
    }
}

fn event_receipt(event: &Event) -> Response<ResponseBody> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Receipt<'event> {
        id: &'event str,
        host_sequence: u64,
        observed_at: Timestamp,
    }
    json_response(
        StatusCode::CREATED,
        &Receipt {
            id: &event.id,
            host_sequence: event.host_sequence,
            observed_at: event.observed_at,
        },
    )
}

async fn decode_json<T: for<'de> Deserialize<'de>>(
    request: Request<Incoming>,
    read_deadline: tokio::time::Instant,
) -> Result<T, Response<ResponseBody>> {
    if request
        .headers()
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > MAX_INTEGRATION_BODY_BYTES as u64)
    {
        return Err(error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "AgentRun request too large",
        ));
    }
    let mut body = request.into_body();
    let read = async {
        let mut contents = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| ReadBodyError::Read)?;
            if let Some(data) = frame.data_ref() {
                if contents.len().saturating_add(data.len()) > MAX_INTEGRATION_BODY_BYTES {
                    return Err(ReadBodyError::TooLarge);
                }
                contents.extend_from_slice(data);
            }
        }
        Ok::<_, ReadBodyError>(contents)
    };
    let read_remaining = read_deadline.saturating_duration_since(tokio::time::Instant::now());
    let contents = match tokio::time::timeout(read_remaining, read).await {
        Ok(Ok(contents)) => contents,
        Ok(Err(ReadBodyError::TooLarge)) => {
            return Err(error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "AgentRun request too large",
            ));
        }
        Ok(Err(ReadBodyError::Read)) | Err(_) => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid AgentRun request",
            ));
        }
    };
    let mut deserializer = serde_json::Deserializer::from_slice(&contents);
    let value = T::deserialize(&mut deserializer)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid AgentRun request"))?;
    deserializer
        .end()
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid AgentRun request"))?;
    Ok(value)
}

enum ReadBodyError {
    TooLarge,
    Read,
}

struct AbortOnDrop<T>(Option<tokio::task::JoinHandle<T>>);

impl<T> AbortOnDrop<T> {
    fn new(handle: tokio::task::JoinHandle<T>) -> Self {
        Self(Some(handle))
    }
}

impl<T> Future for AbortOnDrop<T> {
    type Output = Result<T, tokio::task::JoinError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(
            self.0
                .as_mut()
                .expect("AbortOnDrop cannot be polled after completion"),
        )
        .poll(context)
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

fn authorized(request: &Request<Incoming>, expected: &str) -> bool {
    bearer_token(request).is_some_and(|actual| constant_time_equal(actual, expected))
}

fn bearer_token(request: &Request<Incoming>) -> Option<&str> {
    request
        .headers()
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

fn constant_time_equal(left: &str, right: &str) -> bool {
    left.len() == right.len() && bool::from(left.as_bytes().ct_eq(right.as_bytes()))
}

fn notify_runtime_changed(state: &ServerState) {
    if let Some(revision) = &state.mutation_revision {
        let _ = revision.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            Some(current.saturating_add(1))
        });
    }
    if let Some(sender) = &state.runtime_changed {
        let _ = sender.try_send(());
    }
}

fn rejected_method_or_origin(request: &Request<Incoming>) -> bool {
    request.method() != Method::POST || request.headers().contains_key(ORIGIN)
}

fn default_thread_factory() -> ThreadFactory {
    Arc::new(|_, task| {
        std::thread::Builder::new()
            .name("ptrack-agent-http".to_owned())
            .spawn(task)
            .map_err(|error| IntegrationError(format!("start AgentRun HTTP server: {error}")))
    })
}

fn empty_response(status: StatusCode) -> Response<ResponseBody> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()))
        .expect("fixed AgentRun response is valid")
}

fn error_response(status: StatusCode, message: &str) -> Response<ResponseBody> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header("x-content-type-options", "nosniff")
        .body(Full::new(Bytes::from(format!("{message}\n"))))
        .expect("fixed AgentRun response is valid")
}

fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response<ResponseBody> {
    match serde_json::to_vec(value) {
        Ok(mut body) => {
            body.push(b'\n');
            Response::builder()
                .status(status)
                .header(CONTENT_TYPE, "application/json")
                .body(Full::new(Bytes::from(body)))
                .expect("fixed AgentRun response is valid")
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "AgentRun response failed",
        ),
    }
}

fn random_opaque_value() -> Result<String, IntegrationError> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| IntegrationError(format!("create AgentRun opaque value: {error}")))?;
    let mut value = String::with_capacity(43);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        value.push(char::from(ALPHABET[usize::from(first >> 2)]));
        value.push(char::from(
            ALPHABET[usize::from(((first & 3) << 4) | (second >> 4))],
        ));
        if chunk.len() > 1 {
            value.push(char::from(
                ALPHABET[usize::from(((second & 15) << 2) | (third >> 6))],
            ));
        }
        if chunk.len() > 2 {
            value.push(char::from(ALPHABET[usize::from(third & 63)]));
        }
    }
    Ok(value)
}

fn integration_error(error: impl fmt::Display) -> IntegrationError {
    IntegrationError(error.to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
