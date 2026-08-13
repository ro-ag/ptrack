use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Command;
use std::sync::Mutex;

use ptrack_capability_policy::AuditEvent;
use ptrack_core::Capability;
use tokio_util::sync::CancellationToken;

use super::audit::{AuditError, AuditRecorder, AuditSink};
use super::git::{GitExecutor, GitRequest, ProcessRunner, build_operation, classify_git_exit};
use super::process::{ProcessError, ProcessResult, ProcessSpec};
use super::test_support::{TempDir, approved_git, approved_ssh};

#[tokio::test]
async fn git_fetch_uses_fresh_identity_fixed_args_and_random_alias() {
    let temp = TempDir::new("git-fixed-fetch");
    let root = temp.path().canonicalize().unwrap();
    let remote = "https://example.com/repo.git";
    let runner = FakeRunner::successful(&root, remote, b"ok\n");
    let capability = approved_git(remote, &["fetch"]);
    let result = executor(&runner)
        .execute(
            &CancellationToken::new(),
            &capability,
            None,
            "agent-codex",
            &root,
            &GitRequest {
                operation: "fetch".to_owned(),
                branch: "main".to_owned(),
                refspec: String::new(),
                force: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(result.stdout, "ok\n");
    assert_eq!(result.diagnostic, "none");
    let specs = runner.specs();
    assert_eq!(specs.len(), 5);
    assert_eq!(
        strings(&specs[0].args)[..5],
        [
            "-c",
            "core.fsmonitor=false",
            "-C",
            root.to_str().unwrap(),
            "rev-parse"
        ]
    );
    let operation = &specs[4];
    let args = strings(&operation.args);
    for required in [
        "core.fsmonitor=false",
        "protocol.allow=never",
        "protocol.ext.allow=never",
        "submodule.recurse=false",
        "fetch.recurseSubmodules=false",
        "push.recurseSubmodules=no",
        "protocol.https.allow=always",
        "fetch",
        "--no-recurse-submodules",
        "--no-tags",
        "refs/heads/main",
    ] {
        assert!(args.contains(&required), "missing {required}: {args:?}");
    }
    assert!(!args.contains(&"origin"));
    let alias = args
        .iter()
        .find(|value| value.starts_with("ptrack-approved-") && value.ends_with("://remote"))
        .unwrap();
    assert_eq!(
        alias.len(),
        "ptrack-approved-".len() + 48 + "://remote".len()
    );
    assert!(
        alias["ptrack-approved-".len()..alias.len() - "://remote".len()]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    let environment = environment(operation);
    for exact in [
        ("LC_ALL", "C"),
        ("LANG", "C"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GCM_INTERACTIVE", "Never"),
        ("GIT_NO_LAZY_FETCH", "1"),
    ] {
        assert!(environment.contains(&exact));
    }
}

#[tokio::test]
async fn git_changed_remote_overrides_and_rewrites_deny_before_operation() {
    let temp = TempDir::new("git-policy-deny");
    let root = temp.path().canonicalize().unwrap();
    let approved = "https://example.com/repo.git";
    let capability = approved_git(approved, &["fetch"]);
    let changed = FakeRunner::successful(&root, "https://evil.example/repo.git", b"never");
    let error = executor(&changed)
        .execute(
            &CancellationToken::new(),
            &capability,
            None,
            "agent-codex",
            &root,
            &fetch(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "capability denied: Git remote no longer matches the approved scope"
    );
    assert_eq!(changed.specs().len(), 4);

    let overrides = FakeRunner::with_results(vec![
        ok(format!("{}\n", root.display()).as_bytes()),
        ok(format!("{approved}\n").as_bytes()),
        ok(b"remote.origin.pushurl https://evil.example/repo.git\n"),
    ]);
    let error = executor(&overrides)
        .execute(
            &CancellationToken::new(),
            &capability,
            None,
            "agent-codex",
            &root,
            &fetch(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "capability denied: Git remote overrides make the approved operation ambiguous"
    );
    assert_eq!(overrides.specs().len(), 3);

    let rewrite = FakeRunner::with_results(vec![
        ok(format!("{}\n", root.display()).as_bytes()),
        ok(format!("{approved}\n").as_bytes()),
        exit(1, b""),
        ok(b"url.https://evil/.insteadOf https://example.com/\n"),
    ]);
    let error = executor(&rewrite)
        .execute(
            &CancellationToken::new(),
            &capability,
            None,
            "agent-codex",
            &root,
            &fetch(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "capability denied: Git URL rewrite rules make the approved remote ambiguous"
    );
    assert_eq!(rewrite.specs().len(), 4);
}

#[tokio::test]
async fn git_real_repository_identity_and_ambient_overrides_are_checked() {
    let temp = TempDir::new("git-real");
    git(temp.path(), &["init", "-q"]);
    git(
        temp.path(),
        &[
            "config",
            "remote.origin.url",
            "https://example.com/repo.git",
        ],
    );
    let capability = approved_git("https://example.com/repo.git", &["status"]);
    let result = GitExecutor::new(None)
        .execute(
            &CancellationToken::new(),
            &capability,
            None,
            "agent-codex",
            temp.path(),
            &GitRequest {
                operation: "status".to_owned(),
                branch: String::new(),
                refspec: String::new(),
                force: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.diagnostic, "none");

    git(
        temp.path(),
        &["config", "remote.origin.uploadpack", "/tmp/hostile"],
    );
    let error = GitExecutor::new(None)
        .execute(
            &CancellationToken::new(),
            &capability,
            None,
            "agent-codex",
            temp.path(),
            &GitRequest {
                operation: "status".to_owned(),
                branch: String::new(),
                refspec: String::new(),
                force: false,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "capability denied: Git remote overrides make the approved operation ambiguous"
    );
}

#[tokio::test]
async fn git_ssh_requires_exact_separate_grant_and_pinned_command() {
    let temp = TempDir::new("git-ssh");
    let root = temp.path().canonicalize().unwrap();
    let remote = "git@example.com:org/repo.git";
    let capability = approved_git(remote, &["fetch"]);
    let runner = FakeRunner::successful(&root, remote, b"ok");
    let missing = executor(&runner)
        .execute(
            &CancellationToken::new(),
            &capability,
            None,
            "agent-codex",
            &root,
            &fetch(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        missing.to_string(),
        "capability denied: Git-over-SSH requires a separate SSH grant"
    );

    let mismatched = approved_ssh("evil.example", "git");
    let runner = FakeRunner::successful(&root, remote, b"ok");
    let error = executor(&runner)
        .execute(
            &CancellationToken::new(),
            &capability,
            Some(&mismatched),
            "agent-codex",
            &root,
            &fetch(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "capability denied: Git remote does not match the approved SSH host identity"
    );

    let ssh = approved_ssh("example.com", "git");
    let runner = FakeRunner::successful(&root, remote, b"ok");
    executor(&runner)
        .execute(
            &CancellationToken::new(),
            &capability,
            Some(&ssh),
            "agent-codex",
            &root,
            &fetch(),
        )
        .await
        .unwrap();
    let operation = runner.specs().pop().unwrap();
    let environment = environment(&operation);
    assert!(environment.contains(&("GIT_SSH_VARIANT", "ssh")));
    let command = environment
        .iter()
        .find_map(|(name, value)| (*name == "GIT_SSH_COMMAND").then_some(*value))
        .unwrap();
    for required in [
        "'ssh'",
        "'-F' '/dev/null'",
        "'BatchMode=yes'",
        "'PasswordAuthentication=no'",
        "'KbdInteractiveAuthentication=no'",
        "'StrictHostKeyChecking=yes'",
        "'GlobalKnownHostsFile=/dev/null'",
        "'PermitLocalCommand=no'",
        "'ClearAllForwardings=yes'",
        "'-p' '22'",
    ] {
        assert!(command.contains(required), "missing {required}: {command}");
    }
    assert!(runner.known_hosts_observed());
}

#[test]
#[allow(clippy::too_many_lines)]
fn git_fixed_operation_argv_and_unsupported_gate_are_exact() {
    let temp = TempDir::new("git-operation-argv");
    let root = temp.path();
    let hooks = root.join("hooks");
    fs::create_dir(&hooks).unwrap();
    let capability = approved_git(
        "https://example.com/repo.git",
        &["status", "fetch", "pull", "push", "ls-remote"],
    );
    let cases = [
        (
            GitRequest {
                operation: "status".to_owned(),
                branch: String::new(),
                refspec: String::new(),
                force: false,
            },
            vec!["status", "--short", "--branch"],
        ),
        (
            fetch(),
            vec![
                "fetch",
                "--no-recurse-submodules",
                "--no-tags",
                "--",
                "alias://remote",
                "refs/heads/main",
            ],
        ),
        (
            GitRequest {
                operation: "pull".to_owned(),
                branch: "main".to_owned(),
                refspec: String::new(),
                force: false,
            },
            vec![
                "pull",
                "--ff-only",
                "--no-rebase",
                "--no-recurse-submodules",
                "--no-tags",
                "--",
                "alias://remote",
                "refs/heads/main",
            ],
        ),
        (
            GitRequest {
                operation: "push".to_owned(),
                branch: "main".to_owned(),
                refspec: String::new(),
                force: false,
            },
            vec![
                "push",
                "--",
                "alias://remote",
                "refs/heads/main:refs/heads/main",
            ],
        ),
        (
            GitRequest {
                operation: "ls-remote".to_owned(),
                branch: "main".to_owned(),
                refspec: String::new(),
                force: false,
            },
            vec!["ls-remote", "--", "alias://remote", "refs/heads/main"],
        ),
    ];
    for (request, expected_tail) in cases {
        let spec = build_operation(
            &capability,
            root,
            &hooks,
            "alias://remote",
            &request,
            Vec::new(),
        )
        .unwrap();
        assert!(strings(&spec.args).ends_with(&expected_tail));
    }
    let error = build_operation(
        &capability,
        root,
        &hooks,
        "alias://remote",
        &GitRequest {
            operation: "config".to_owned(),
            branch: String::new(),
            refspec: String::new(),
            force: false,
        },
        Vec::new(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "capability denied: unsupported Git operation"
    );
}

#[tokio::test]
async fn git_result_error_and_audit_precedence_are_exact() {
    let temp = TempDir::new("git-result");
    let root = temp.path().canonicalize().unwrap();
    let remote = "https://example.com/repo.git";
    let capability = approved_git(remote, &["fetch"]);
    let truncated = FakeRunner::with_results(vec![
        ok(format!("{}\n", root.display()).as_bytes()),
        ok(format!("{remote}\n").as_bytes()),
        exit(1, b""),
        exit(1, b""),
        ProcessResult {
            exit_code: 128,
            stdout: b"partial".to_vec(),
            stderr: b"SECRET stderr".to_vec(),
            truncated: true,
        },
    ]);
    let error = executor(&truncated)
        .execute(
            &CancellationToken::new(),
            &capability,
            None,
            "agent-codex",
            &root,
            &fetch(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "process output exceeds its byte limit");
    assert_eq!(error.class(), "output-limit");
    assert_eq!(error.result().exit_code, 128);
    assert_eq!(error.result().stdout, "partial");
    assert_eq!(error.result().stderr, "SECRET stderr");
    assert_eq!(error.result().diagnostic, "output-limit");

    let sink = FailingAudit;
    let failed = FakeRunner::with_results(vec![
        ok(format!("{}\n", root.display()).as_bytes()),
        ok(format!("{remote}\n").as_bytes()),
        exit(1, b""),
        exit(1, b""),
        exit_with_stderr(128, b"SECRET protected branch"),
    ]);
    let executor = GitExecutor::from_parts(AuditRecorder::from_sink(&sink), &failed);
    let operation_error = executor
        .execute(
            &CancellationToken::new(),
            &capability,
            None,
            "agent-codex",
            &root,
            &fetch(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        operation_error.to_string(),
        "Git operation failed: remote-policy"
    );
    assert!(!operation_error.to_string().contains("SECRET"));

    let success = FakeRunner::successful(&root, remote, b"ok");
    let executor = GitExecutor::from_parts(AuditRecorder::from_sink(&sink), &success);
    assert_eq!(
        executor
            .execute(
                &CancellationToken::new(),
                &capability,
                None,
                "agent-codex",
                &root,
                &fetch(),
            )
            .await
            .unwrap_err()
            .to_string(),
        "record capability audit: internal"
    );
}

#[test]
fn git_error_classification_is_stable_and_secret_free() {
    for (stderr, class) in [
        ("Could not resolve host", "dns"),
        ("certificate verify failed", "tls"),
        ("Host key verification failed", "host-key"),
        ("permission denied (publickey)", "authentication"),
        ("remote rejected: protected branch", "remote-policy"),
        ("Failed to connect: Connection refused", "routing"),
        ("opaque SECRET", "transport"),
    ] {
        assert_eq!(classify_git_exit(128, stderr), class);
    }
    assert_eq!(classify_git_exit(0, "SECRET"), "none");
}

pub(super) fn assert_cap_069_through_076_git_contract() {
    git_fixed_operation_argv_and_unsupported_gate_are_exact();
    git_error_classification_is_stable_and_secret_free();
}

struct FailingAudit;

impl AuditSink for FailingAudit {
    fn record(&self, _capability: &Capability, _event: &AuditEvent) -> Result<(), AuditError> {
        Err(AuditError)
    }
}

struct FakeRunner {
    results: Mutex<VecDeque<Result<ProcessResult, ProcessError>>>,
    specs: Mutex<Vec<ProcessSpec>>,
    known_hosts: Mutex<bool>,
}

impl FakeRunner {
    fn with_results(results: Vec<ProcessResult>) -> Self {
        Self {
            results: Mutex::new(results.into_iter().map(Ok).collect()),
            specs: Mutex::new(Vec::new()),
            known_hosts: Mutex::new(false),
        }
    }

    fn successful(root: &Path, remote: &str, operation_stdout: &[u8]) -> Self {
        Self::with_results(vec![
            ok(format!("{}\n", root.display()).as_bytes()),
            ok(format!("{remote}\n").as_bytes()),
            exit(1, b""),
            exit(1, b""),
            ok(operation_stdout),
        ])
    }

    fn specs(&self) -> Vec<ProcessSpec> {
        self.specs.lock().unwrap().clone()
    }

    fn known_hosts_observed(&self) -> bool {
        *self.known_hosts.lock().unwrap()
    }
}

impl ProcessRunner for FakeRunner {
    fn run<'a>(
        &'a self,
        spec: &'a ProcessSpec,
        _cancellation: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ProcessResult, ProcessError>> + Send + 'a>> {
        Box::pin(async move {
            self.specs.lock().unwrap().push(spec.clone());
            for (name, value) in &spec.env {
                if name == "GIT_SSH_COMMAND" {
                    let command = value.to_string_lossy();
                    if let Some(rest) = command.split("UserKnownHostsFile=").nth(1)
                        && let Some(path) = rest.split('\'').next()
                    {
                        let contents = fs::read_to_string(path).unwrap();
                        *self.known_hosts.lock().unwrap() =
                            contents == "example.com ssh-ed25519 QUJDREVGR0hJSktMTU5PUA==\n";
                    }
                }
            }
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(ProcessResult::default()))
        })
    }
}

fn executor(runner: &FakeRunner) -> GitExecutor<'_> {
    GitExecutor::from_parts(AuditRecorder::new(None), runner)
}

fn fetch() -> GitRequest {
    GitRequest {
        operation: "fetch".to_owned(),
        branch: "main".to_owned(),
        refspec: String::new(),
        force: false,
    }
}

fn ok(stdout: &[u8]) -> ProcessResult {
    ProcessResult {
        exit_code: 0,
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
        truncated: false,
    }
}

fn exit(code: i32, stdout: &[u8]) -> ProcessResult {
    ProcessResult {
        exit_code: code,
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
        truncated: false,
    }
}

fn exit_with_stderr(code: i32, stderr: &[u8]) -> ProcessResult {
    ProcessResult {
        exit_code: code,
        stdout: Vec::new(),
        stderr: stderr.to_vec(),
        truncated: false,
    }
}

fn strings(values: &[OsString]) -> Vec<&str> {
    values.iter().map(|value| value.to_str().unwrap()).collect()
}

fn environment(spec: &ProcessSpec) -> Vec<(&str, &str)> {
    spec.env
        .iter()
        .map(|(name, value)| (name.to_str().unwrap(), value.to_str().unwrap()))
        .collect()
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success());
}
