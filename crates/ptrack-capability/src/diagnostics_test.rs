use std::collections::VecDeque;
use std::ffi::OsString;
use std::sync::Mutex;

use ptrack_core::CapabilityKind;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

use super::diagnostics::{ConnectionTester, VpnState, diagnostic, is_tunnel_name};
use super::git::{ProcessRunner, RunFuture};
use super::process::{ProcessError, ProcessResult, ProcessSpec};
use super::test_support::{TempDir, approved_git, approved_http, approved_ssh};

pub(super) fn assert_cap_089_090_and_092_diagnostic_contract() {
    let cases = [
        ("none", "complete", "Connection test succeeded."),
        (
            "denied",
            "policy",
            "The capability policy rejected the test.",
        ),
        ("dns", "dns", "The host name could not be resolved."),
        (
            "routing",
            "routing",
            "No usable route to the host was available.",
        ),
        (
            "vpn",
            "vpn",
            "A required VPN route or policy was unavailable.",
        ),
        (
            "proxy",
            "proxy",
            "The current proxy rejected or could not authenticate the request.",
        ),
        (
            "tls",
            "tls",
            "TLS certificate or handshake validation failed with the system CA store.",
        ),
        (
            "host-key",
            "host-key",
            "The SSH host key did not match the pinned key.",
        ),
        (
            "authentication",
            "authentication",
            "Host authentication failed using current credential helpers or ssh-agent.",
        ),
        (
            "sandbox",
            "sandbox",
            "The host sandbox or local permissions blocked the operation.",
        ),
        (
            "remote-policy",
            "remote-policy",
            "The remote service was reached but rejected the operation.",
        ),
        ("timeout", "connect", "The connection test timed out."),
        (
            "request-limit",
            "request",
            "The connection failed for an unclassified transport reason.",
        ),
        (
            "response-limit",
            "response",
            "The connection failed for an unclassified transport reason.",
        ),
        (
            "output-limit",
            "response",
            "The connection test exceeded its output limit.",
        ),
        (
            "cancelled",
            "cancelled",
            "The connection test was cancelled.",
        ),
        (
            "transport",
            "transport",
            "The connection failed for an unclassified transport reason.",
        ),
        (
            "internal",
            "internal",
            "The connection test failed internally.",
        ),
    ];
    for (class, stage, message) in cases {
        let value = diagnostic(CapabilityKind::Http, class, 0, VpnState::Inactive);
        assert_eq!(value.class, class);
        assert_eq!(value.stage, stage);
        assert_eq!(value.message, message);
        assert_eq!(value.success, class == "none");
    }
    assert!(is_tunnel_name("utun4"));
    assert!(is_tunnel_name("WG0"));
    assert!(!is_tunnel_name("ethernet"));
}

#[test]
fn diagnostic_unknown_class_falls_back_without_leaking_source_error() {
    let value = diagnostic(
        CapabilityKind::Ssh,
        "credential=secret host=/private/path",
        0,
        VpnState::Unknown,
    );
    assert_eq!(value.class, "transport");
    assert_eq!(value.stage, "transport");
    assert!(!value.message.contains("secret"));
}

#[tokio::test]
async fn http_diagnostic_executes_probe_and_preserves_sanitized_407_metadata() {
    let requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let observed = std::sync::Arc::clone(&requests);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            stream.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        observed.lock().await.push(request);
        stream
            .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    let diagnostic = ConnectionTester
        .test_http(
            &CancellationToken::new(),
            &approved_http(&format!("http://{address}/api")),
        )
        .await;
    assert_eq!(diagnostic.class, "proxy");
    assert_eq!(diagnostic.stage, "proxy");
    assert_eq!(diagnostic.status_code, 407);
    assert_eq!(diagnostic.ca_store, "system");
    assert!(!diagnostic.proxy.is_empty());
    assert!(!diagnostic.proxy.contains(['@', '?', '#']));
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert!(String::from_utf8_lossy(&requests[0]).starts_with("GET /api HTTP/1.1"));
    server.await.unwrap();
}

#[tokio::test]
async fn git_and_ssh_diagnostics_execute_only_read_only_fixed_probes() {
    let temp = TempDir::new("diagnostic-execution");
    let git = ScriptedRunner::new(vec![
        success(format!("{}\n", temp.path().display()).into_bytes()),
        success(b"https://example.com/repo.git\n".to_vec()),
        exit(1, Vec::new(), Vec::new()),
        exit(1, Vec::new(), Vec::new()),
        success(b"0123456789abcdef\trefs/heads/main\n".to_vec()),
    ]);
    let diagnostic = ConnectionTester
        .test_git_with_runner(
            &CancellationToken::new(),
            &approved_git("https://example.com/repo.git", &["fetch"]),
            None,
            temp.path(),
            &git,
        )
        .await;
    assert_eq!(diagnostic.class, "none");
    let git_specs = git.specs();
    assert_eq!(git_specs.len(), 5);
    let operation = strings(&git_specs[4].args);
    assert!(operation.iter().any(|value| value == "ls-remote"));
    assert!(
        !operation
            .iter()
            .any(|value| { matches!(value.as_str(), "fetch" | "pull" | "push" | "status") })
    );

    let ssh = ScriptedRunner::new(vec![exit(
        255,
        Vec::new(),
        b"Permission denied (publickey)".to_vec(),
    )]);
    let diagnostic = ConnectionTester
        .test_ssh_with_runner(
            &CancellationToken::new(),
            &approved_ssh("example.com", "deploy"),
            &ssh,
        )
        .await;
    assert_eq!(diagnostic.class, "authentication");
    let ssh_specs = ssh.specs();
    assert_eq!(ssh_specs.len(), 1);
    let args = strings(&ssh_specs[0].args);
    assert!(
        args.iter()
            .any(|value| value == "StrictHostKeyChecking=yes")
    );
    assert!(args.iter().any(|value| value == "deploy@example.com"));
    assert_eq!(args.last().map(String::as_str), Some("true"));
}

struct ScriptedRunner {
    results: Mutex<VecDeque<Result<ProcessResult, ProcessError>>>,
    specs: Mutex<Vec<ProcessSpec>>,
}

impl ScriptedRunner {
    fn new(results: Vec<ProcessResult>) -> Self {
        Self {
            results: Mutex::new(results.into_iter().map(Ok).collect()),
            specs: Mutex::new(Vec::new()),
        }
    }

    fn specs(&self) -> Vec<ProcessSpec> {
        self.specs.lock().unwrap().clone()
    }
}

impl ProcessRunner for ScriptedRunner {
    fn run<'a>(
        &'a self,
        spec: &'a ProcessSpec,
        _cancellation: &'a CancellationToken,
    ) -> RunFuture<'a> {
        Box::pin(async move {
            self.specs.lock().unwrap().push(spec.clone());
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(ProcessResult::default()))
        })
    }
}

fn success(stdout: Vec<u8>) -> ProcessResult {
    exit(0, stdout, Vec::new())
}

fn exit(exit_code: i32, stdout: Vec<u8>, stderr: Vec<u8>) -> ProcessResult {
    ProcessResult {
        exit_code,
        stdout,
        stderr,
        truncated: false,
    }
}

fn strings(values: &[OsString]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect()
}
