use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

use super::*;
use crate::test_support::TempDirectory;

struct Fixture {
    _home: TempDirectory,
    root: TempDirectory,
    registry: Arc<Registry>,
    server: IntegrationServer,
    descriptor: IntegrationDescriptor,
}

impl Fixture {
    fn new(generation: u64, runtime_changed: Option<SyncSender<()>>) -> Self {
        let home = TempDirectory::new("ptrack-agent-http-home");
        let root = TempDirectory::new("ptrack-agent-http-root");
        let registry = Arc::new(Registry::new(RegistryConfig {
            project_root: root.path().to_path_buf(),
            ..RegistryConfig::default()
        }));
        let server = start_integration_server(
            Arc::clone(&registry),
            IntegrationConfig {
                global_home: home.path().to_path_buf(),
                project_root: root.path().to_path_buf(),
                generation,
                observer: None,
                mutation_revision: None,
                runtime_changed,
                thread_factory: None,
            },
        )
        .unwrap();
        let descriptor =
            serde_json::from_slice(&fs::read(server.descriptor_path()).unwrap()).unwrap();
        Self {
            _home: home,
            root,
            registry,
            server,
            descriptor,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.server.shutdown();
        let _ = self.registry.shutdown();
    }
}

#[derive(Debug)]
struct WireResponse {
    status: u16,
    headers: String,
    body: Vec<u8>,
}

#[test]
#[allow(clippy::too_many_lines)]
fn integration_server_lifecycle_descriptor_callbacks_and_cleanup_are_exact() {
    let (invalidations, received) = sync_channel(8);
    let fixture = Fixture::new(4, Some(invalidations));
    assert_eq!(fixture.descriptor.generation, 4);
    assert_eq!(
        fixture.descriptor.project_root,
        fixture.root.path().to_string_lossy()
    );
    assert!(fixture.descriptor.url.starts_with("http://127.0.0.1:"));
    assert_eq!(fixture.descriptor.registration_token.len(), 43);
    assert_eq!(
        fixture.server.event_endpoint(),
        format!("{}/v1/events", fixture.descriptor.url)
    );
    let descriptor_json = fs::read_to_string(fixture.server.descriptor_path()).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&descriptor_json)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        [
            "generation",
            "pid",
            "projectRoot",
            "registrationToken",
            "url"
        ]
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(fixture.server.descriptor_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(fixture.server.descriptor_path().parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    let unauthorized = request(
        &fixture.descriptor,
        "POST",
        "/v1/runs/register",
        "wrong",
        br#"{"profile":"wrapper","provider":"codex"}"#,
        &[],
    );
    assert_error(unauthorized, 401, "AgentRun request rejected\n");
    assert_error(
        request(
            &fixture.descriptor,
            "POST",
            "/v1/runs/register",
            "wrong",
            br#"{"profile":"wrapper""#,
            &[],
        ),
        401,
        "AgentRun request rejected\n",
    );

    let registration = serde_json::json!({
        "profile": "wrapper",
        "provider": "codex",
        "pid": 8123,
        "cwd": fixture.root.path()
    });
    let registered = json_request(
        &fixture.descriptor,
        "/v1/runs/register",
        &fixture.descriptor.registration_token,
        &registration,
    );
    assert_eq!(registered.status, 201);
    assert!(
        registered
            .headers
            .contains("content-type: application/json")
    );
    let lease: serde_json::Value = decode(&registered.body);
    let id = lease["id"].as_str().unwrap();
    let lease_token = lease["leaseToken"].as_str().unwrap();
    assert_eq!(id.len(), 43);
    assert_eq!(lease_token.len(), 43);
    assert!(fixture.registry.run(id).unwrap().association.is_none());

    let heartbeat = request(
        &fixture.descriptor,
        "POST",
        &format!("/v1/runs/{id}/heartbeat"),
        lease_token,
        &[],
        &[],
    );
    assert_eq!(heartbeat.status, 204);
    assert!(heartbeat.body.is_empty());

    let event = serde_json::json!({
        "modelVersion": 1,
        "id": "lifecycle-1",
        "sequence": 1,
        "type": "lifecycle.progress",
        "subject": "working"
    });
    let recorded = json_request(
        &fixture.descriptor,
        &format!("/v1/runs/{id}/events"),
        lease_token,
        &event,
    );
    assert_eq!(recorded.status, 201);
    let receipt: serde_json::Value = decode(&recorded.body);
    assert_eq!(receipt["hostSequence"], 1);
    assert_eq!(receipt["id"].as_str().unwrap().len(), 43);
    assert!(receipt["observedAt"].as_str().unwrap().contains('T'));

    let exited = json_request(
        &fixture.descriptor,
        &format!("/v1/runs/{id}/exit"),
        lease_token,
        &serde_json::json!({"code": 0, "result": "done"}),
    );
    assert_eq!(exited.status, 204);
    assert_eq!(drain_invalidation_count(&received), 4);
    let run = fixture.registry.run(id).unwrap();
    assert_eq!(run.state, RunState::Exited);
    assert_eq!(run.exit.unwrap().result, "done");

    let descriptor_path = fixture.server.descriptor_path().to_path_buf();
    fixture.server.shutdown().unwrap();
    fixture.server.shutdown().unwrap();
    assert!(!descriptor_path.exists());
    assert!(TcpStream::connect(url_address(&fixture.descriptor.url)).is_err());
}

#[test]
#[allow(clippy::too_many_lines)]
fn integration_server_strict_gates_routes_json_and_body_limit_are_exact() {
    let fixture = Fixture::new(1, None);
    let register = "/v1/runs/register";
    let token = &fixture.descriptor.registration_token;
    assert_error(
        request(&fixture.descriptor, "GET", register, token, b"{}", &[]),
        403,
        "AgentRun request rejected\n",
    );
    assert_error(
        request(
            &fixture.descriptor,
            "POST",
            register,
            token,
            b"{}",
            &[("Origin", ""), ("Origin", "wails://wails")],
        ),
        403,
        "AgentRun request rejected\n",
    );
    assert_error(
        request(
            &fixture.descriptor,
            "POST",
            register,
            token,
            b"{}",
            &[("Origin", "wails://wails")],
        ),
        403,
        "AgentRun request rejected\n",
    );
    assert_error(
        request(
            &fixture.descriptor,
            "POST",
            register,
            token,
            b"{}",
            &[("Origin", "")],
        ),
        403,
        "AgentRun request rejected\n",
    );
    for (method, path, token, status) in [
        ("GET", "/v1/runs/id/heartbeat", "", 401),
        ("GET", "/v1/runs/id/heartbeat", "present", 403),
        ("POST", "/v1/runs/id/unknown", "", 401),
        ("POST", "/v1/runs/id/unknown", "present", 404),
        ("POST", "/v1/runs/id", "", 404),
    ] {
        assert_eq!(
            request(&fixture.descriptor, method, path, token, b"{}", &[]).status,
            status
        );
    }
    assert_error(
        request(&fixture.descriptor, "POST", "/unknown", token, b"{}", &[]),
        404,
        "404 page not found\n",
    );
    for path in [
        "/v1/runs/",
        "/v1/runs/id",
        "/v1/runs//exit",
        "/v1/runs/id/events/extra",
    ] {
        assert_eq!(
            request(&fixture.descriptor, "POST", path, token, b"{}", &[]).status,
            404
        );
    }
    assert_error(
        request(
            &fixture.descriptor,
            "POST",
            register,
            token,
            br#"{"profile":"wrapper","provider":"codex"} {}"#,
            &[],
        ),
        400,
        "invalid AgentRun request\n",
    );
    assert_error(
        request(
            &fixture.descriptor,
            "POST",
            register,
            token,
            br#"{"profile":"wrapper","provider":"codex","terminalId":"forged"}"#,
            &[],
        ),
        400,
        "invalid AgentRun request\n",
    );
    let oversized = format!(r#"{{"profile":"{}"}}"#, "x".repeat(16 * 1_024));
    assert_error(
        request(
            &fixture.descriptor,
            "POST",
            register,
            token,
            oversized.as_bytes(),
            &[],
        ),
        413,
        "AgentRun request too large\n",
    );
    let exact_prefix = r#"{"profile":"wrapper","provider":"codex","cwd":""#;
    let exact_suffix = r#""}"#;
    let exact = format!(
        "{exact_prefix}{}{exact_suffix}",
        "x".repeat(16 * 1_024 - exact_prefix.len() - exact_suffix.len())
    );
    assert_eq!(exact.len(), 16 * 1_024);
    assert_eq!(
        request(
            &fixture.descriptor,
            "POST",
            register,
            token,
            exact.as_bytes(),
            &[],
        )
        .status,
        400
    );

    let registered = json_request(
        &fixture.descriptor,
        register,
        token,
        &serde_json::json!({"profile":"wrapper","provider":"codex","cwd":fixture.root.path()}),
    );
    let lease: serde_json::Value = decode(&registered.body);
    let id = lease["id"].as_str().unwrap();
    let lease_token = lease["leaseToken"].as_str().unwrap();
    let event_path = format!("/v1/runs/{id}/events");
    assert_error(
        request(
            &fixture.descriptor,
            "POST",
            &event_path,
            "wrong",
            br#"{"modelVersion":1"#,
            &[],
        ),
        401,
        "AgentRun lease rejected\n",
    );
    let exit_path = format!("/v1/runs/{id}/exit");
    let began = Instant::now();
    let mut exit_stream = connect_retry(url_address(&fixture.descriptor.url));
    exit_stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    write!(
        exit_stream,
        "POST {exit_path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer wrong\r\nContent-Length: 100\r\n\r\n",
        url_address(&fixture.descriptor.url)
    )
    .unwrap();
    assert!(read_response_headers(&mut exit_stream).starts_with("HTTP/1.1 401"));
    assert!(began.elapsed() < Duration::from_secs(1));
    assert_error(
        request(
            &fixture.descriptor,
            "POST",
            &event_path,
            lease_token,
            br#"{"modelVersion":1,"id":"event","sequence":1,"type":"lifecycle.progress","prompt":"private"}"#,
            &[],
        ),
        400,
        "invalid AgentRun request\n",
    );
    assert_error(
        request(
            &fixture.descriptor,
            "POST",
            &event_path,
            lease_token,
            br#"{"modelVersion":1,"id":"event","sequence":1,"type":"unknown"}"#,
            &[],
        ),
        400,
        "AgentRun event rejected\n",
    );
}

#[test]
fn launched_event_waits_for_binding_then_revocation_fails_closed() {
    let fixture = Fixture::new(2, None);
    let token = fixture.registry.issue_launched_event_token().unwrap();
    let address = fixture.descriptor.clone();
    let token_for_request = token.clone();
    let barrier = Arc::new(Barrier::new(2));
    let started = Arc::clone(&barrier);
    let pending = std::thread::spawn(move || {
        started.wait();
        json_request(
            &address,
            "/v1/events",
            &token_for_request,
            &serde_json::json!({
                "modelVersion":1,"id":"item-1","sequence":1,
                "type":"item.completed","category":"file","paths":["src/lib.rs"]
            }),
        )
    });
    barrier.wait();
    let run = fixture
        .registry
        .register_launched(Registration {
            profile: "agent-codex".to_owned(),
            provider: "codex".to_owned(),
            pid: i32::try_from(std::process::id()).unwrap(),
            terminal_id: "terminal-1".to_owned(),
            cwd: fixture.root.path().to_string_lossy().into_owned(),
        })
        .unwrap();
    fixture
        .registry
        .bind_launched_event_token(&token, &run.id)
        .unwrap();
    assert_eq!(pending.join().unwrap().status, 201);
    assert_eq!(fixture.registry.event_snapshot(&run.id, 10).unwrap().1, 1);
    assert!(
        fixture
            .registry
            .record_terminal_exit("terminal-1", 0, "done")
    );
    assert_error(
        json_request(
            &fixture.descriptor,
            "/v1/events",
            &token,
            &serde_json::json!({
                "modelVersion":1,"id":"item-2","sequence":2,
                "type":"item.completed","category":"file"
            }),
        ),
        401,
        "AgentRun event token rejected\n",
    );

    let pending_token = fixture.registry.issue_launched_event_token().unwrap();
    let began = Instant::now();
    let timed_out = json_request(
        &fixture.descriptor,
        "/v1/events",
        &pending_token,
        &serde_json::json!({
            "modelVersion":1,"id":"wait","sequence":1,"type":"lifecycle.progress"
        }),
    );
    assert_error(timed_out, 401, "AgentRun event token rejected\n");
    assert!(began.elapsed() >= Duration::from_millis(1_900));
    assert!(began.elapsed() < Duration::from_secs(3));
}

#[test]
fn descriptor_generation_and_token_fence_prevents_old_shutdown_cleanup() {
    let home = TempDirectory::new("ptrack-agent-http-generation-home");
    let root = TempDirectory::new("ptrack-agent-http-generation-root");
    let first_registry = Arc::new(Registry::new(RegistryConfig {
        project_root: root.path().to_path_buf(),
        ..RegistryConfig::default()
    }));
    let same_generation_registry = Arc::new(Registry::new(RegistryConfig {
        project_root: root.path().to_path_buf(),
        ..RegistryConfig::default()
    }));
    let newer_registry = Arc::new(Registry::new(RegistryConfig {
        project_root: root.path().to_path_buf(),
        ..RegistryConfig::default()
    }));
    let first = start_integration_server(
        Arc::clone(&first_registry),
        IntegrationConfig {
            global_home: home.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            generation: 1,
            observer: None,
            mutation_revision: None,
            runtime_changed: None,
            thread_factory: None,
        },
    )
    .unwrap();
    let same_generation = start_integration_server(
        Arc::clone(&same_generation_registry),
        IntegrationConfig {
            global_home: home.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            generation: 1,
            observer: None,
            mutation_revision: None,
            runtime_changed: None,
            thread_factory: None,
        },
    )
    .unwrap();
    first.shutdown().unwrap();
    let same_generation_replacement: IntegrationDescriptor =
        serde_json::from_slice(&fs::read(same_generation.descriptor_path()).unwrap()).unwrap();
    assert_eq!(same_generation_replacement.generation, 1);
    let newer = start_integration_server(
        Arc::clone(&newer_registry),
        IntegrationConfig {
            global_home: home.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            generation: 2,
            observer: None,
            mutation_revision: None,
            runtime_changed: None,
            thread_factory: None,
        },
    )
    .unwrap();
    same_generation.shutdown().unwrap();
    let replacement: IntegrationDescriptor =
        serde_json::from_slice(&fs::read(newer.descriptor_path()).unwrap()).unwrap();
    assert_eq!(replacement.generation, 2);
    newer.shutdown().unwrap();
    assert!(!newer.descriptor_path().exists());
    first_registry.shutdown().unwrap();
    same_generation_registry.shutdown().unwrap();
    newer_registry.shutdown().unwrap();
}

#[test]
fn integration_transport_enforces_header_body_and_idle_timeouts() {
    let fixture = Fixture::new(7, None);
    let registered = json_request(
        &fixture.descriptor,
        "/v1/runs/register",
        &fixture.descriptor.registration_token,
        &serde_json::json!({"profile":"wrapper","provider":"codex","cwd":fixture.root.path()}),
    );
    let lease: serde_json::Value = decode(&registered.body);
    let id = lease["id"].as_str().unwrap();
    let lease_token = lease["leaseToken"].as_str().unwrap();
    let address = url_address(&fixture.descriptor.url);

    let mut slow_header = connect_retry(address);
    slow_header
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    slow_header
        .write_all(b"POST /v1/runs/register HTTP/1.1\r\nHost: 127.0.0.1")
        .unwrap();

    let mut slow_body = connect_retry(address);
    slow_body
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    write!(
        slow_body,
        "POST /v1/runs/register HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {}\r\nContent-Length: 10\r\n\r\n{{",
        fixture.descriptor.registration_token
    )
    .unwrap();

    let mut idle = connect_retry(address);
    idle.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    write!(
        idle,
        "POST /v1/runs/{id}/heartbeat HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {lease_token}\r\nContent-Length: 0\r\n\r\n"
    )
    .unwrap();
    let idle_headers = read_response_headers(&mut idle);
    assert!(idle_headers.starts_with("HTTP/1.1 204"));

    let started = Instant::now();
    std::thread::sleep(Duration::from_millis(5_500));
    let mut header_bytes = Vec::new();
    let header_read = slow_header.read_to_end(&mut header_bytes);
    assert!(
        header_read.is_ok() || header_read.unwrap_err().kind() != std::io::ErrorKind::WouldBlock
    );
    assert!(header_bytes.is_empty());
    let mut body_bytes = Vec::new();
    let _ = slow_body.read_to_end(&mut body_bytes);
    assert_eq!(parse_response(&body_bytes).status, 400);

    let remaining = Duration::from_millis(15_500).saturating_sub(started.elapsed());
    std::thread::sleep(remaining);
    let mut byte = [0_u8; 1];
    assert_eq!(idle.read(&mut byte).unwrap(), 0);
}

#[test]
fn active_keepalive_resets_idle_timeout_beyond_fifteen_seconds() {
    let fixture = Fixture::new(8, None);
    let registered = json_request(
        &fixture.descriptor,
        "/v1/runs/register",
        &fixture.descriptor.registration_token,
        &serde_json::json!({"profile":"wrapper","provider":"codex","cwd":fixture.root.path()}),
    );
    let lease: serde_json::Value = decode(&registered.body);
    let id = lease["id"].as_str().unwrap();
    let token = lease["leaseToken"].as_str().unwrap();
    let address = url_address(&fixture.descriptor.url);
    let mut stream = connect_retry(address);
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    for _ in 0..5 {
        write!(
            stream,
            "POST /v1/runs/{id}/heartbeat HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\n\r\n"
        )
        .unwrap();
        assert!(read_response_headers(&mut stream).starts_with("HTTP/1.1 204"));
        std::thread::sleep(Duration::from_secs(4));
    }
}

#[test]
fn header_and_body_share_one_five_second_read_budget() {
    let fixture = Fixture::new(9, None);
    let address = url_address(&fixture.descriptor.url);
    let mut stream = connect_retry(address);
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let started = Instant::now();
    write!(
        stream,
        "POST /v1/runs/register HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {}\r\n",
        fixture.descriptor.registration_token
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(4));
    stream
        .write_all(b"Content-Length: 20\r\n\r\n{\"profile\":")
        .unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
    assert!(started.elapsed() < Duration::from_secs(7));
    assert!(response.is_empty() || parse_response(&response).status == 400);
}

#[test]
fn pending_event_wait_flood_is_bounded_and_shutdown_stays_bounded() {
    let fixture = Fixture::new(10, None);
    let bound_token = fixture.registry.issue_launched_event_token().unwrap();
    let bound_run = fixture
        .registry
        .register_launched(Registration {
            profile: "agent-codex".to_owned(),
            provider: "codex".to_owned(),
            pid: i32::try_from(std::process::id()).unwrap(),
            terminal_id: "terminal-bound".to_owned(),
            cwd: fixture.root.path().to_string_lossy().into_owned(),
        })
        .unwrap();
    fixture
        .registry
        .bind_launched_event_token(&bound_token, &bound_run.id)
        .unwrap();
    let tokens: Vec<String> = (0..32)
        .map(|_| fixture.registry.issue_launched_event_token().unwrap())
        .collect();
    let barrier = Arc::new(Barrier::new(tokens.len() + 1));
    let mut threads = Vec::new();
    for token in tokens {
        let descriptor = fixture.descriptor.clone();
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            let mut stream = connect_retry(url_address(&descriptor.url));
            stream.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
            let body = br#"{"modelVersion":1,"id":"wait","sequence":1,"type":"lifecycle.progress"}"#;
            write!(
                stream,
                "POST /v1/events HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n",
                url_address(&descriptor.url),
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
            let mut response = Vec::new();
            let _ = stream.read_to_end(&mut response);
            response
        }));
    }
    barrier.wait();
    std::thread::sleep(Duration::from_millis(200));
    let began = Instant::now();
    let bound = json_request(
        &fixture.descriptor,
        "/v1/events",
        &bound_token,
        &serde_json::json!({
            "modelVersion":1,"id":"bound","sequence":1,"type":"lifecycle.progress"
        }),
    );
    assert_eq!(bound.status, 201);
    assert!(began.elapsed() < Duration::from_secs(1));
    let started = Instant::now();
    fixture.server.shutdown().unwrap();
    assert!(started.elapsed() <= Duration::from_millis(2_300));
    let immediate = threads
        .into_iter()
        .filter_map(|thread| thread.join().ok())
        .filter(|response| !response.is_empty())
        .count();
    assert!(immediate >= 24);
}

#[test]
fn bounded_invalidation_sender_has_no_payload_or_host_execution_surface() {
    let (sender, receiver) = sync_channel(1);
    let fixture = Fixture::new(11, Some(sender));
    let began = Instant::now();
    let response = json_request(
        &fixture.descriptor,
        "/v1/runs/register",
        &fixture.descriptor.registration_token,
        &serde_json::json!({"profile":"wrapper","provider":"codex","cwd":fixture.root.path()}),
    );
    assert_eq!(response.status, 201);
    assert!(began.elapsed() < Duration::from_secs(1));
    assert_eq!(receiver.try_recv(), Ok(()));
    let lease: serde_json::Value = decode(&response.body);
    drop(receiver);
    assert_eq!(
        request(
            &fixture.descriptor,
            "POST",
            &format!("/v1/runs/{}/heartbeat", lease["id"].as_str().unwrap()),
            lease["leaseToken"].as_str().unwrap(),
            &[],
            &[],
        )
        .status,
        204
    );
    let began = Instant::now();
    fixture.server.shutdown().unwrap();
    assert!(began.elapsed() < Duration::from_secs(2));
}

#[test]
fn read_integration_descriptor_reports_missing_and_stale() {
    let home = TempDirectory::new("ptrack-agent-http-liveness-home");
    let root = TempDirectory::new("ptrack-agent-http-liveness-root");
    assert_eq!(
        read_integration_descriptor(home.path(), root.path()).unwrap_err(),
        PersistenceError::DescriptorNotFound
    );
    let stale = IntegrationDescriptor {
        project_root: root.path().to_string_lossy().into_owned(),
        url: "http://127.0.0.1:1".to_owned(),
        generation: 1,
        registration_token: "secret".to_owned(),
        pid: 0,
    };
    publish_runtime_json(home.path(), root.path(), "agent-registry.json", &stale).unwrap();
    assert_eq!(
        read_integration_descriptor(home.path(), root.path()).unwrap_err(),
        PersistenceError::DescriptorStale { pid: 0 }
    );
}

struct FixedObserver;

impl AgentObservation for FixedObserver {
    fn observe_runs(&self, generation: u64) -> Result<AgentRunsV2, CoordinationError> {
        if generation != 7 {
            return Err(CoordinationError::StaleGeneration {
                expected: generation,
                active: 7,
            });
        }
        Ok(AgentRunsV2 {
            generation,
            runs: vec![observed_run()],
            bounds: BoundedSnapshot::new(1, 1),
        })
    }

    fn observe_run(
        &self,
        generation: u64,
        run_id: &str,
    ) -> Result<AgentRunObservationV1, CoordinationError> {
        if generation != 7 {
            return Err(CoordinationError::StaleGeneration {
                expected: generation,
                active: 7,
            });
        }
        if run_id != "run-1" {
            return Err(CoordinationError::RunNotFound);
        }
        Ok(AgentRunObservationV1 {
            generation,
            run: observed_run(),
            intelligence: AgentIntelligenceDetail {
                state: IntelligenceState::Waiting,
                confidence: IntelligenceConfidence::High,
                evidence: Vec::new(),
                event_count: 2,
                last_event_at: Some(Timestamp::from_unix_seconds(2)),
            },
            event_bounds: BoundedSnapshot::new(2, 2),
        })
    }

    fn observe_handoffs(&self, generation: u64) -> Result<AgentHandoffInbox, CoordinationError> {
        if generation != 7 {
            return Err(CoordinationError::StaleGeneration {
                expected: generation,
                active: 7,
            });
        }
        Ok(AgentHandoffInbox {
            items: Vec::new(),
            bounds: BoundedSnapshot::new(0, 0),
            incomplete: false,
        })
    }
}

fn observed_run() -> AgentRuntimeSummary {
    AgentRuntimeSummary {
        run_id: "run-1".to_owned(),
        registration_kind: RegistrationKind::External,
        terminal_id: String::new(),
        terminal_backed: false,
        terminal_present: false,
        corresponding_terminal: false,
        state: RunState::Running,
        process_state: ProcessState::Unknown,
        lease_state: LeaseState::Active,
        live: true,
        activity_state: ActivityState::Waiting,
        association: Some(RuntimeAssociation {
            plan_id: 26,
            task_id: 209,
            revision: 3,
        }),
        intelligence: None,
    }
}

#[test]
fn observation_client_is_authenticated_generation_fenced_and_sanitized() {
    let home = TempDirectory::new("ptrack-agent-observe-home");
    let root = TempDirectory::new("ptrack-agent-observe-root");
    let registry = Arc::new(Registry::new(RegistryConfig {
        project_root: root.path().to_path_buf(),
        ..RegistryConfig::default()
    }));
    let server = start_integration_server(
        Arc::clone(&registry),
        IntegrationConfig {
            global_home: home.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            generation: 7,
            observer: Some(Arc::new(FixedObserver)),
            mutation_revision: None,
            runtime_changed: None,
            thread_factory: None,
        },
    )
    .unwrap();
    let descriptor: IntegrationDescriptor =
        serde_json::from_slice(&fs::read(server.descriptor_path()).unwrap()).unwrap();
    // The caller does not own the Desktop runtime generation. The authenticated
    // live descriptor does, and every request reuses that exact value.
    let client = AgentObservationClient::for_project(home.path(), root.path()).unwrap();
    assert_eq!(client.runs().unwrap().runs, vec![observed_run()]);
    assert_eq!(client.run("run-1").unwrap().intelligence.event_count, 2);
    assert!(client.inbox().unwrap().items.is_empty());
    assert_error(
        json_request(
            &descriptor,
            "/v1/observe/runs",
            "wrong",
            &serde_json::json!({"generation": 7}),
        ),
        401,
        "AgentRun request rejected\n",
    );
    assert_error(
        json_request(
            &descriptor,
            "/v1/observe/runs",
            &descriptor.registration_token,
            &serde_json::json!({"generation": 6}),
        ),
        409,
        "AgentRun observation generation changed\n",
    );
    let response = json_request(
        &descriptor,
        "/v1/observe/runs",
        &descriptor.registration_token,
        &serde_json::json!({"generation": 7}),
    );
    let body = String::from_utf8(response.body).unwrap();
    assert!(!body.contains("pid"));
    assert!(!body.contains("cwd"));
    assert!(!body.contains("provider"));
    server.shutdown().unwrap();
    registry.shutdown().unwrap();
}

#[test]
fn thread_start_failure_leaves_no_descriptor_or_live_listener() {
    let home = TempDirectory::new("ptrack-agent-http-thread-failure-home");
    let root = TempDirectory::new("ptrack-agent-http-thread-failure-root");
    let registry = Arc::new(Registry::new(RegistryConfig {
        project_root: root.path().to_path_buf(),
        ..RegistryConfig::default()
    }));
    let captured_address = Arc::new(std::sync::Mutex::new(None));
    let address_slot = Arc::clone(&captured_address);
    let Err(error) = start_integration_server(
        Arc::clone(&registry),
        IntegrationConfig {
            global_home: home.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            generation: 13,
            observer: None,
            mutation_revision: None,
            runtime_changed: None,
            thread_factory: Some(Arc::new(move |address, _task| {
                *address_slot.lock().unwrap() = Some(address);
                Err(IntegrationError("injected thread failure".to_owned()))
            })),
        },
    ) else {
        panic!("injected thread failure unexpectedly started server");
    };
    assert_eq!(error.to_string(), "injected thread failure");
    assert_eq!(
        read_integration_descriptor(home.path(), root.path()).unwrap_err(),
        PersistenceError::DescriptorNotFound
    );
    assert!(TcpStream::connect(captured_address.lock().unwrap().unwrap()).is_err());
    registry.shutdown().unwrap();
}

fn json_request<T: serde::Serialize>(
    descriptor: &IntegrationDescriptor,
    path: &str,
    token: &str,
    body: &T,
) -> WireResponse {
    request(
        descriptor,
        "POST",
        path,
        token,
        &serde_json::to_vec(body).unwrap(),
        &[("Content-Type", "application/json")],
    )
}

fn request(
    descriptor: &IntegrationDescriptor,
    method: &str,
    path: &str,
    token: &str,
    body: &[u8],
    extra_headers: &[(&str, &str)],
) -> WireResponse {
    let address = url_address(&descriptor.url);
    let mut stream = connect_retry(address);
    stream
        .set_read_timeout(Some(Duration::from_secs(6)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(6)))
        .unwrap();
    // One write keeps the body in the same segment as the headers. Split writes let the
    // server reject and close before it drains the body, and that close arrives as a reset
    // that discards the response the client already buffered.
    let mut wire = Vec::new();
    write!(
        wire,
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )
    .unwrap();
    for (name, value) in extra_headers {
        write!(wire, "{name}: {value}\r\n").unwrap();
    }
    wire.extend_from_slice(b"\r\n");
    wire.extend_from_slice(body);
    stream.write_all(&wire).unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    parse_response(&bytes)
}

fn connect_retry(address: &str) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match TcpStream::connect(address) {
            Ok(stream) => return stream,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                std::thread::yield_now();
            }
            Err(error) => panic!("connect to integration server: {error}"),
        }
    }
}

fn url_address(url: &str) -> &str {
    url.strip_prefix("http://").unwrap()
}

fn parse_response(bytes: &[u8]) -> WireResponse {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let headers = String::from_utf8(bytes[..split].to_vec()).unwrap();
    let status = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    WireResponse {
        status,
        headers: headers.to_ascii_lowercase(),
        body: bytes[split + 4..].to_vec(),
    }
}

fn read_response_headers(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
    }
    String::from_utf8(bytes).unwrap()
}

fn decode<T: DeserializeOwned>(body: &[u8]) -> T {
    serde_json::from_slice(body).unwrap()
}

#[allow(clippy::needless_pass_by_value)]
fn assert_error(response: WireResponse, status: u16, body: &str) {
    assert_eq!(response.status, status);
    assert_eq!(response.body, body.as_bytes());
}

fn drain_invalidation_count(receiver: &Receiver<()>) -> usize {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut count = 0;
    while count < 4 && Instant::now() < deadline {
        if receiver.recv_timeout(Duration::from_millis(10)).is_ok() {
            count += 1;
        }
    }
    count
}
