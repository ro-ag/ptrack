use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ptrack_capability_policy::AuditEvent;
use ptrack_core::Capability;
use reqwest::Url;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::audit::{AuditError, AuditRecorder, AuditSink};
use super::http::{
    ConnectionClass, HttpExecutor, HttpRequest, proxy_diagnostic_from, validate_headers,
};
use super::test_support::{approved_http, refresh_approval};

type Requests = Arc<Mutex<Vec<String>>>;

#[tokio::test]
async fn http_redirect_is_reauthorized_and_secrets_are_stripped() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (origin, task) = server(Arc::clone(&requests), |index, _| {
        if index == 0 {
            response("302 Found", &[("Location", "/api/final")], b"")
        } else {
            response("200 OK", &[], b"ok")
        }
    })
    .await;
    let capability = approved_http(&format!("{origin}/api"));
    let executor = HttpExecutor::new(None);
    let response = executor
        .execute(
            &CancellationToken::new(),
            &capability,
            "agent-codex",
            &request(
                "GET",
                &format!("{origin}/api/start"),
                &[
                    ("Authorization", "Bearer secret"),
                    ("Cookie", "session=secret"),
                ],
            ),
        )
        .await
        .unwrap();
    assert_eq!(response.body, b"ok");
    assert_eq!(response.redirects, 1);
    assert_eq!(response.diagnostics.ca_store, "system");
    let observed = requests.lock().await;
    assert!(
        observed[0]
            .to_ascii_lowercase()
            .contains("authorization: bearer secret")
    );
    assert!(!observed[1].to_ascii_lowercase().contains("authorization:"));
    assert!(!observed[1].to_ascii_lowercase().contains("cookie:"));
    task.abort();
}

#[tokio::test]
async fn http_redirect_escape_and_zero_limit_stop_before_second_hop() {
    let reached = Arc::new(AtomicBool::new(false));
    let outside_reached = Arc::clone(&reached);
    let (outside, outside_task) = server(Arc::new(Mutex::new(Vec::new())), move |_, _| {
        outside_reached.store(true, Ordering::SeqCst);
        response("200 OK", &[], b"escaped")
    })
    .await;
    let (inside, inside_task) = server(Arc::new(Mutex::new(Vec::new())), move |_, _| {
        response("302 Found", &[("Location", outside.as_str())], b"")
    })
    .await;
    let capability = approved_http(&format!("{inside}/api"));
    let error = HttpExecutor::new(None)
        .execute(
            &CancellationToken::new(),
            &capability,
            "agent-codex",
            &request("GET", &format!("{inside}/api/start"), &[]),
        )
        .await
        .unwrap_err();
    assert_eq!(error.class(), ConnectionClass::Denied);
    assert!(
        error
            .to_string()
            .starts_with("redirect rejected: capability denied:")
    );
    assert!(!reached.load(Ordering::SeqCst));

    let mut zero = capability;
    zero.limits.max_redirects = 0;
    zero = refresh_approval(zero);
    let error = HttpExecutor::new(None)
        .execute(
            &CancellationToken::new(),
            &zero,
            "agent-codex",
            &request("GET", &format!("{inside}/api/start"), &[]),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "capability denied: HTTP redirect limit exceeded"
    );
    inside_task.abort();
    outside_task.abort();
}

#[test]
fn http_header_policy_has_exact_bounds_and_denies_ambiguous_fields() {
    for forbidden in [
        "Host",
        "Proxy-Authorization",
        "Connection",
        "Keep-Alive",
        "Proxy-Connection",
        "Te",
        "Trailer",
        "Transfer-Encoding",
        "Upgrade",
    ] {
        let error = validate_headers(&headers(&[(forbidden, "value")])).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("HTTP header {forbidden:?} is not allowed")
        );
    }
    assert_eq!(
        validate_headers(&headers(&[("X-Test", "line\nsecret")]))
            .unwrap_err()
            .to_string(),
        "HTTP header \"X-Test\" contains a newline"
    );
    let boundary = "x".repeat((64 << 10) - "X".len());
    assert!(validate_headers(&headers(&[("X", &boundary)])).is_ok());
    let over = format!("{boundary}x");
    assert_eq!(
        validate_headers(&headers(&[("X", &over)]))
            .unwrap_err()
            .to_string(),
        "HTTP headers exceed their byte limit"
    );
}

#[tokio::test]
async fn http_response_stream_and_decoded_headers_are_bounded() {
    let (origin, task) = server(Arc::new(Mutex::new(Vec::new())), |index, _| {
        if index == 0 {
            response("200 OK", &[], &[b'x'; 17])
        } else {
            response("200 OK", &[("X-Large", &"x".repeat(64 << 10))], b"")
        }
    })
    .await;
    let mut capability = approved_http(&format!("{origin}/api"));
    capability.limits.max_response_bytes = 16;
    capability = refresh_approval(capability);
    let executor = HttpExecutor::new(None);
    let error = executor
        .execute(
            &CancellationToken::new(),
            &capability,
            "agent-codex",
            &request("GET", &format!("{origin}/api/body"), &[]),
        )
        .await
        .unwrap_err();
    assert_eq!(error.class(), ConnectionClass::ResponseLimit);
    assert_eq!(error.to_string(), "HTTP response exceeds its byte limit");

    let error = executor
        .execute(
            &CancellationToken::new(),
            &capability,
            "agent-codex",
            &request("GET", &format!("{origin}/api/headers"), &[]),
        )
        .await
        .unwrap_err();
    assert_eq!(error.class(), ConnectionClass::ResponseLimit);
    assert_eq!(
        error.to_string(),
        "HTTP response headers exceed their byte limit"
    );
    task.abort();
}

#[tokio::test]
async fn http_timeout_and_cancel_are_stable_and_secret_free() {
    let (origin, task) = hanging_server().await;
    let mut capability = approved_http(&format!("{origin}/api"));
    capability.limits.timeout_seconds = 1;
    capability = refresh_approval(capability);
    let request = request("GET", &format!("{origin}/api/secret?token=SECRET"), &[]);
    let error = HttpExecutor::new(None)
        .execute(
            &CancellationToken::new(),
            &capability,
            "agent-codex",
            &request,
        )
        .await
        .unwrap_err();
    assert_eq!(error.class(), ConnectionClass::Timeout);
    assert!(!error.to_string().contains("SECRET"));

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let error = HttpExecutor::new(None)
        .execute(&cancellation, &capability, "agent-codex", &request)
        .await
        .unwrap_err();
    assert_eq!(error.class(), ConnectionClass::Cancelled);
    task.abort();
}

#[tokio::test]
async fn http_audit_error_surfaces_only_after_success() {
    let (origin, task) = server(Arc::new(Mutex::new(Vec::new())), |_, _| {
        response("200 OK", &[], b"ok")
    })
    .await;
    let capability = approved_http(&format!("{origin}/api"));
    let sink = FailingAudit;
    let executor = HttpExecutor {
        recorder: AuditRecorder::from_sink(&sink),
    };
    let success_error = executor
        .execute(
            &CancellationToken::new(),
            &capability,
            "agent-codex",
            &request("GET", &format!("{origin}/api/ok"), &[]),
        )
        .await
        .unwrap_err();
    assert_eq!(
        success_error.to_string(),
        "record capability audit: internal"
    );

    let mut denied = capability;
    denied.limits.max_response_bytes = 1;
    denied = refresh_approval(denied);
    let operation_error = executor
        .execute(
            &CancellationToken::new(),
            &denied,
            "agent-codex",
            &request("GET", &format!("{origin}/api/large"), &[]),
        )
        .await
        .unwrap_err();
    assert_eq!(
        operation_error.to_string(),
        "HTTP response exceeds its byte limit"
    );
    task.abort();
}

#[test]
fn proxy_diagnostics_honor_no_proxy_and_redact_credentials() {
    let target = Url::parse("https://api.example.com:443/path").unwrap();
    let values = HashMap::from([
        (
            "HTTPS_PROXY".to_owned(),
            "http://user:secret@proxy.example:8080/path?token=secret#fragment".to_owned(),
        ),
        ("NO_PROXY".to_owned(), "internal.example,.local".to_owned()),
    ]);
    assert_eq!(
        proxy_diagnostic_from(&target, |name| values.get(name).cloned()),
        "http://proxy.example:8080/path"
    );
    let bypass = Url::parse("https://service.internal.example/path").unwrap();
    assert_eq!(
        proxy_diagnostic_from(&bypass, |name| values.get(name).cloned()),
        "direct"
    );
}

#[test]
fn http_dto_body_is_base64_and_round_trips() {
    let request = HttpRequest {
        method: "POST".to_owned(),
        url: "https://example.com/api".to_owned(),
        headers: BTreeMap::new(),
        body: vec![0, b's', b'e', b'c', b'r', b'e', b't', 255],
    };
    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains(r#""body":"AHNlY3JldP8=""#));
    assert_eq!(serde_json::from_str::<HttpRequest>(&json).unwrap(), request);
}

pub(super) fn assert_cap_063_through_068_http_contract() {
    http_header_policy_has_exact_bounds_and_denies_ambiguous_fields();
    proxy_diagnostics_honor_no_proxy_and_redact_credentials();
    http_dto_body_is_base64_and_round_trips();
}

struct FailingAudit;

impl AuditSink for FailingAudit {
    fn record(&self, _capability: &Capability, _event: &AuditEvent) -> Result<(), AuditError> {
        Err(AuditError)
    }
}

fn request(method: &str, url: &str, values: &[(&str, &str)]) -> HttpRequest {
    HttpRequest {
        method: method.to_owned(),
        url: url.to_owned(),
        headers: headers(values),
        body: Vec::new(),
    }
}

fn headers(values: &[(&str, &str)]) -> BTreeMap<String, Vec<String>> {
    values
        .iter()
        .map(|(name, value)| ((*name).to_owned(), vec![(*value).to_owned()]))
        .collect()
}

async fn server(
    requests: Requests,
    response_fn: impl Fn(usize, &str) -> Vec<u8> + Send + Sync + 'static,
) -> (String, tokio::task::JoinHandle<io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let response_fn = Arc::new(response_fn);
    let task = tokio::spawn(async move {
        let mut index = 0_usize;
        loop {
            let (mut stream, _) = listener.accept().await?;
            let request = read_request(&mut stream).await?;
            requests.lock().await.push(request.clone());
            let response = response_fn(index, &request);
            index += 1;
            stream.write_all(&response).await?;
            stream.shutdown().await?;
        }
    });
    (format!("http://{address}"), task)
}

async fn read_request(stream: &mut TcpStream) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1_024];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > 128 << 10 {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn hanging_server() -> (String, tokio::task::JoinHandle<io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await?;
            let _request = read_request(&mut stream).await?;
            std::future::pending::<()>().await;
        }
    });
    (format!("http://{address}"), task)
}

fn response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )
    .into_bytes();
    for (name, value) in headers {
        bytes.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    bytes.extend_from_slice(b"\r\n");
    bytes.extend_from_slice(body);
    bytes
}
