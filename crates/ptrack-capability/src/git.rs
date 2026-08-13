use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, Instant};

use ptrack_capability_policy::{
    AuditEvent, Denied, GitAuthorization, SshOperation, authorize_git, authorize_ssh, normalize,
};
use ptrack_core::{Capability, CapabilityKind, SshScope};
use ptrack_store::{Clock, ProjectStore, SystemClock};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::audit::AuditRecorder;
use crate::process::{ProcessError, ProcessResult, ProcessSpec, run_process};

const GIT_ENV: [(&str, &str); 5] = [
    ("LC_ALL", "C"),
    ("LANG", "C"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GCM_INTERACTIVE", "Never"),
    ("GIT_NO_LAZY_FETCH", "1"),
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitRequest {
    pub operation: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub branch: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub refspec: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitResult {
    pub exit_code: i32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdout: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr: String,
    pub diagnostic: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitError {
    message: String,
    class: String,
    result: Box<GitResult>,
}

impl GitError {
    fn new(message: impl Into<String>, class: impl Into<String>) -> Self {
        let class = class.into();
        Self {
            message: message.into(),
            result: Box::new(GitResult {
                exit_code: -1,
                stdout: String::new(),
                stderr: String::new(),
                diagnostic: class.clone(),
            }),
            class,
        }
    }

    fn with_result(
        message: impl Into<String>,
        class: impl Into<String>,
        result: GitResult,
    ) -> Self {
        Self {
            message: message.into(),
            class: class.into(),
            result: Box::new(result),
        }
    }

    #[cfg(windows)]
    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(message, "internal")
    }

    #[must_use]
    pub fn class(&self) -> &str {
        &self.class
    }

    #[must_use]
    pub const fn result(&self) -> &GitResult {
        &self.result
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for GitError {}

impl From<Denied> for GitError {
    fn from(error: Denied) -> Self {
        Self::new(error.to_string(), "denied")
    }
}

pub struct GitExecutor<'a> {
    pub(crate) recorder: AuditRecorder<'a>,
    runner: &'a dyn ProcessRunner,
}

type RunFuture<'a> = Pin<Box<dyn Future<Output = Result<ProcessResult, ProcessError>> + Send + 'a>>;

pub(crate) trait ProcessRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        spec: &'a ProcessSpec,
        cancellation: &'a CancellationToken,
    ) -> RunFuture<'a>;
}

struct SystemRunner;

impl ProcessRunner for SystemRunner {
    fn run<'a>(
        &'a self,
        spec: &'a ProcessSpec,
        cancellation: &'a CancellationToken,
    ) -> RunFuture<'a> {
        Box::pin(run_process(spec, cancellation))
    }
}

static SYSTEM_RUNNER: SystemRunner = SystemRunner;

impl<'a> GitExecutor<'a> {
    #[must_use]
    pub const fn new(store: Option<&'a ProjectStore>) -> Self {
        Self {
            recorder: AuditRecorder::new(store),
            runner: &SYSTEM_RUNNER,
        }
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "private executor test injection seam")
    )]
    pub(crate) const fn from_parts(
        recorder: AuditRecorder<'a>,
        runner: &'a dyn ProcessRunner,
    ) -> Self {
        Self { recorder, runner }
    }

    /// Executes a fixed Git operation against freshly verified repository and
    /// remote state. SSH transport requires a separate exact SSH grant.
    ///
    /// # Errors
    /// Returns stable policy and diagnostic errors without exposing argv,
    /// environment, raw URLs, or subprocess stderr.
    pub async fn execute(
        &self,
        cancellation: &CancellationToken,
        git_capability: &Capability,
        ssh_capability: Option<&Capability>,
        agent_profile: &str,
        project_root: &Path,
        request: &GitRequest,
    ) -> Result<GitResult, GitError> {
        let canonical_root = project_root.canonicalize().map_err(|_| {
            GitError::new(
                "capability denied: Git project root cannot be canonicalized",
                "denied",
            )
        })?;
        let preview = normalize(git_capability).map_err(|_| {
            GitError::new(
                "capability denied: stored Git capability is invalid",
                "denied",
            )
        })?;
        if preview.capability.kind != CapabilityKind::Git {
            return Err(GitError::new(
                "capability denied: stored Git capability is invalid",
                "denied",
            ));
        }
        let limits = &preview.capability.limits;
        let timeout =
            Duration::from_secs(u64::try_from(limits.timeout_seconds).unwrap_or_default());
        let maximum = u64::try_from(limits.max_output_bytes).unwrap_or_default();
        let actual_remote_url = verify_repository(
            self.runner,
            cancellation,
            &canonical_root,
            &preview.capability,
            timeout,
            maximum,
        )
        .await?;
        let scope = preview.capability.git.as_ref().ok_or_else(|| {
            GitError::new(
                "capability denied: stored Git capability is invalid",
                "denied",
            )
        })?;
        let normalized = authorize_git(
            git_capability,
            agent_profile,
            SystemClock.now_utc(),
            &GitAuthorization {
                operation: request.operation.clone(),
                remote_name: scope.remote_name.clone(),
                remote_url: actual_remote_url,
                branch: request.branch.clone(),
                refspec: request.refspec.clone(),
                force: request.force,
            },
        )?;
        let started = Instant::now();
        let outcome = self
            .execute_authorized(
                cancellation,
                &canonical_root,
                &normalized,
                ssh_capability,
                agent_profile,
                request,
            )
            .await;
        let response_bytes = outcome.as_ref().map_or_else(
            |error| {
                i64::try_from(
                    error
                        .result
                        .stdout
                        .len()
                        .saturating_add(error.result.stderr.len()),
                )
                .unwrap_or(i64::MAX)
            },
            |result| {
                i64::try_from(result.stdout.len().saturating_add(result.stderr.len()))
                    .unwrap_or(i64::MAX)
            },
        );
        let event = AuditEvent {
            operation: request.operation.clone(),
            target: scope.remote_name.clone(),
            success: outcome.is_ok(),
            error_class: outcome
                .as_ref()
                .err()
                .map_or_else(|| "none".to_owned(), |error| error.class.clone()),
            duration_millis: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
            request_bytes: 0,
            response_bytes,
            redirects: 0,
        };
        if let Err(error) = self.recorder.record(&normalized, &event)
            && outcome.is_ok()
        {
            return Err(GitError::new(error.to_string(), "internal"));
        }
        outcome
    }

    async fn execute_authorized(
        &self,
        cancellation: &CancellationToken,
        root: &Path,
        capability: &Capability,
        ssh_capability: Option<&Capability>,
        agent_profile: &str,
        request: &GitRequest,
    ) -> Result<GitResult, GitError> {
        let scope = capability.git.as_ref().expect("normalized Git capability");
        let hooks = PrivateTempDir::new("ptrack-empty-hooks")?;
        let alias = random_alias()?;
        let mut env = git_environment();
        let _ssh = if is_ssh_remote(&scope.remote_url) {
            let ssh = ssh_capability.ok_or_else(|| {
                GitError::new(
                    "capability denied: Git-over-SSH requires a separate SSH grant",
                    "denied",
                )
            })?;
            let approved_ssh = authorize_ssh(
                ssh,
                agent_profile,
                SystemClock.now_utc(),
                SshOperation::Git,
                "",
            )
            .map_err(|_| {
                GitError::new(
                    "capability denied: Git remote does not match the approved SSH host identity",
                    "denied",
                )
            })?;
            let ssh_scope = approved_ssh.ssh.as_ref().ok_or_else(|| {
                GitError::new(
                    "capability denied: Git remote does not match the approved SSH host identity",
                    "denied",
                )
            })?;
            if !git_remote_matches_ssh(&scope.remote_url, ssh_scope) {
                return Err(GitError::new(
                    "capability denied: Git remote does not match the approved SSH host identity",
                    "denied",
                ));
            }
            let pinned = PinnedKnownHosts::new(ssh_scope)?;
            env.push((OsString::from("GIT_SSH_VARIANT"), OsString::from("ssh")));
            env.push((
                OsString::from("GIT_SSH_COMMAND"),
                OsString::from(git_ssh_command(ssh_scope, &pinned.file)),
            ));
            Some(pinned)
        } else {
            None
        };
        let spec = build_operation(capability, root, &hooks.path, &alias, request, env)?;
        let process = self
            .runner
            .run(&spec, cancellation)
            .await
            .map_err(process_git_error)?;
        process_result(&process)
    }
}

async fn verify_repository(
    runner: &dyn ProcessRunner,
    cancellation: &CancellationToken,
    root: &Path,
    capability: &Capability,
    timeout: Duration,
    maximum: u64,
) -> Result<String, GitError> {
    let scope = capability.git.as_ref().expect("normalized Git capability");
    let actual = run_metadata(
        runner,
        cancellation,
        root,
        timeout,
        maximum,
        &["rev-parse", "--show-toplevel"],
    )
    .await
    .map_err(|_| {
        GitError::new(
            "capability denied: Git repository identity could not be verified",
            "denied",
        )
    })?;
    let actual = one_line(&actual).ok_or_else(|| {
        GitError::new(
            "capability denied: Git repository identity could not be verified",
            "denied",
        )
    })?;
    let actual = Path::new(actual).canonicalize().map_err(|_| {
        GitError::new(
            "capability denied: Git repository identity could not be verified",
            "denied",
        )
    })?;
    if actual != root {
        return Err(GitError::new(
            "capability denied: Git repository root does not match the project",
            "denied",
        ));
    }
    let remote_key = format!("remote.{}.url", scope.remote_name);
    let remote = run_metadata(
        runner,
        cancellation,
        root,
        timeout,
        maximum,
        &["config", "--get-all", &remote_key],
    )
    .await
    .map_err(|_| {
        GitError::new(
            "capability denied: Git remote could not be verified",
            "denied",
        )
    })?;
    let remote = one_line(&remote)
        .ok_or_else(|| GitError::new("capability denied: Git remote is invalid", "denied"))?
        .to_owned();
    let remote_pattern = format!(
        "^remote\\.{}\\.(pushurl|uploadpack|receivepack)$",
        escape_basic_regex(&scope.remote_name)
    );
    verify_no_config(
        runner,
        cancellation,
        root,
        timeout,
        maximum,
        &remote_pattern,
        "Git remote override policy could not be verified",
        "Git remote overrides make the approved operation ambiguous",
    )
    .await?;
    verify_no_config(
        runner,
        cancellation,
        root,
        timeout,
        maximum,
        "^url\\..*\\.(insteadOf|pushInsteadOf)$",
        "Git URL rewrite policy could not be verified",
        "Git URL rewrite rules make the approved remote ambiguous",
    )
    .await?;
    Ok(remote)
}

#[allow(clippy::too_many_arguments)]
async fn verify_no_config(
    runner: &dyn ProcessRunner,
    cancellation: &CancellationToken,
    root: &Path,
    timeout: Duration,
    maximum: u64,
    pattern: &str,
    verify_error: &str,
    ambiguous_error: &str,
) -> Result<(), GitError> {
    let spec = git_process(root, timeout, maximum, &["config", "--get-regexp", pattern]);
    let result = runner
        .run(&spec, cancellation)
        .await
        .map_err(|_| GitError::new(format!("capability denied: {verify_error}"), "denied"))?;
    if result.truncated || (result.exit_code != 0 && result.exit_code != 1) {
        return Err(GitError::new(
            format!("capability denied: {verify_error}"),
            "denied",
        ));
    }
    if result.exit_code == 0 || !result.stdout.is_empty() {
        return Err(GitError::new(
            format!("capability denied: {ambiguous_error}"),
            "denied",
        ));
    }
    Ok(())
}

async fn run_metadata(
    runner: &dyn ProcessRunner,
    cancellation: &CancellationToken,
    root: &Path,
    timeout: Duration,
    maximum: u64,
    args: &[&str],
) -> Result<ProcessResult, ProcessError> {
    let spec = git_process(root, timeout, maximum, args);
    let result = runner.run(&spec, cancellation).await?;
    if result.exit_code != 0 || result.truncated {
        return Err(ProcessError::Wait);
    }
    Ok(result)
}

fn git_process(root: &Path, timeout: Duration, maximum: u64, args: &[&str]) -> ProcessSpec {
    let mut full = vec![OsString::from("-c"), OsString::from("core.fsmonitor=false")];
    full.push(OsString::from("-C"));
    full.push(root.as_os_str().to_owned());
    full.extend(args.iter().map(OsString::from));
    ProcessSpec {
        name: OsString::from("git"),
        args: full,
        env: git_environment(),
        max_output_bytes: maximum,
        timeout,
    }
}

pub(crate) fn build_operation(
    capability: &Capability,
    root: &Path,
    hooks: &Path,
    alias: &str,
    request: &GitRequest,
    env: Vec<(OsString, OsString)>,
) -> Result<ProcessSpec, GitError> {
    let scope = capability.git.as_ref().expect("normalized Git capability");
    let mut args: Vec<OsString> = vec![
        "-C".into(),
        root.into(),
        "-c".into(),
        format!("core.hooksPath={}", hooks.display()).into(),
        "-c".into(),
        "core.fsmonitor=false".into(),
        "-c".into(),
        "protocol.allow=never".into(),
        "-c".into(),
        "protocol.ext.allow=never".into(),
        "-c".into(),
        "submodule.recurse=false".into(),
        "-c".into(),
        "fetch.recurseSubmodules=false".into(),
        "-c".into(),
        "push.recurseSubmodules=no".into(),
        "-c".into(),
        if is_ssh_remote(&scope.remote_url) {
            "protocol.ssh.allow=always".into()
        } else {
            "protocol.https.allow=always".into()
        },
        "-c".into(),
        format!("url.{}.insteadOf={alias}", scope.remote_url).into(),
        "-c".into(),
        format!("url.{}.pushInsteadOf={alias}", scope.remote_url).into(),
    ];
    match request.operation.as_str() {
        "status" => args.extend(os_args(&["status", "--short", "--branch"])),
        "fetch" => {
            args.extend(os_args(&["fetch", "--no-recurse-submodules"]));
            args.push(
                if scope.allow_tags {
                    "--tags"
                } else {
                    "--no-tags"
                }
                .into(),
            );
            args.extend(os_args(&["--", alias]));
            args.push(if request.refspec.is_empty() {
                format!("refs/heads/{}", request.branch).into()
            } else {
                request.refspec.clone().into()
            });
        }
        "pull" => {
            args.extend(os_args(&[
                "pull",
                "--ff-only",
                "--no-rebase",
                "--no-recurse-submodules",
            ]));
            if !scope.allow_tags {
                args.push("--no-tags".into());
            }
            args.extend(os_args(&["--", alias]));
            args.push(format!("refs/heads/{}", request.branch).into());
        }
        "push" => {
            args.push("push".into());
            if request.force {
                args.push("--force-with-lease".into());
            }
            args.extend(os_args(&["--", alias]));
            args.push(if request.refspec.is_empty() {
                format!("refs/heads/{0}:refs/heads/{0}", request.branch).into()
            } else {
                request.refspec.clone().into()
            });
        }
        "ls-remote" => {
            args.extend(os_args(&["ls-remote", "--", alias]));
            if !request.branch.is_empty() {
                args.push(format!("refs/heads/{}", request.branch).into());
            }
        }
        _ => {
            return Err(GitError::new(
                "capability denied: unsupported Git operation",
                "denied",
            ));
        }
    }
    Ok(ProcessSpec {
        name: OsString::from("git"),
        args,
        env,
        max_output_bytes: u64::try_from(capability.limits.max_output_bytes).unwrap_or_default(),
        timeout: Duration::from_secs(
            u64::try_from(capability.limits.timeout_seconds).unwrap_or_default(),
        ),
    })
}

fn process_result(result: &ProcessResult) -> Result<GitResult, GitError> {
    let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    if result.truncated {
        return Err(GitError::with_result(
            "process output exceeds its byte limit",
            "output-limit",
            GitResult {
                exit_code: result.exit_code,
                stdout,
                stderr,
                diagnostic: "output-limit".to_owned(),
            },
        ));
    }
    let class = classify_git_exit(result.exit_code, &stderr);
    if result.exit_code != 0 {
        return Err(GitError::with_result(
            format!("Git operation failed: {class}"),
            class,
            GitResult {
                exit_code: result.exit_code,
                stdout,
                stderr,
                diagnostic: class.to_owned(),
            },
        ));
    }
    Ok(GitResult {
        exit_code: result.exit_code,
        stdout,
        stderr,
        diagnostic: "none".to_owned(),
    })
}

fn process_git_error(error: ProcessError) -> GitError {
    let class = match error {
        ProcessError::Cancelled => "cancelled",
        ProcessError::Timeout => "timeout",
        ProcessError::Spawn | ProcessError::Wait => "transport",
    };
    GitError::new(format!("Git operation failed: {class}"), class)
}

#[must_use]
pub fn classify_git_exit(exit_code: i32, stderr: &str) -> &'static str {
    if exit_code == 0 {
        return "none";
    }
    let lower = stderr.to_ascii_lowercase();
    match () {
        () if lower.contains("could not resolve host")
            || lower.contains("could not resolve hostname") =>
        {
            "dns"
        }
        () if lower.contains("ssl certificate problem")
            || lower.contains("certificate verify failed") =>
        {
            "tls"
        }
        () if lower.contains("host key verification failed") => "host-key",
        () if lower.contains("authentication failed")
            || lower.contains("could not read username")
            || lower.contains("permission denied (publickey") =>
        {
            "authentication"
        }
        () if lower.contains("protected branch")
            || lower.contains("remote rejected")
            || lower.contains("repository not found")
            || lower.contains("permission to") =>
        {
            "remote-policy"
        }
        () if lower.contains("connection refused")
            || lower.contains("no route to host")
            || lower.contains("failed to connect") =>
        {
            "routing"
        }
        () => "transport",
    }
}

fn one_line(result: &ProcessResult) -> Option<&str> {
    let text = std::str::from_utf8(&result.stdout).ok()?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    if text.is_empty() || text.trim() != text || text.contains(['\n', '\r']) {
        return None;
    }
    Some(text)
}

fn git_environment() -> Vec<(OsString, OsString)> {
    GIT_ENV
        .iter()
        .map(|(name, value)| ((*name).into(), (*value).into()))
        .collect()
}

fn random_alias() -> Result<String, GitError> {
    let mut random = [0_u8; 24];
    getrandom::fill(&mut random)
        .map_err(|_| GitError::new("Git transport alias could not be generated", "internal"))?;
    Ok(format!("ptrack-approved-{}://remote", hex(&random)))
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn os_args<'a>(values: &'a [&'a str]) -> impl Iterator<Item = OsString> + 'a {
    values.iter().map(OsString::from)
}

fn escape_basic_regex(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        if matches!(character, '.' | '[' | '\\' | '*' | '^' | '$') {
            output.push('\\');
        }
        output.push(character);
    }
    output
}

fn is_ssh_remote(remote: &str) -> bool {
    remote.starts_with("ssh://") || !remote.contains("://")
}

fn git_remote_matches_ssh(remote: &str, scope: &SshScope) -> bool {
    git_ssh_identity(remote).is_some_and(|(user, host, port)| {
        user == scope.user && host == scope.host && port == scope.port
    })
}

fn git_ssh_identity(remote: &str) -> Option<(String, String, u16)> {
    if let Some(rest) = remote.strip_prefix("ssh://") {
        let authority = rest.split('/').next()?;
        let (user, host_port) = authority.split_once('@')?;
        if user.is_empty() {
            return None;
        }
        let (host, port) = if host_port.starts_with('[') {
            let end = host_port.find(']')?;
            let host = &host_port[1..end];
            let port = host_port
                .get(end + 1..)
                .and_then(|suffix| suffix.strip_prefix(':'))
                .map_or(Some(22), |value| value.parse().ok())?;
            (host, port)
        } else if let Some((host, port)) = host_port.rsplit_once(':') {
            (host, port.parse().ok()?)
        } else {
            (host_port, 22)
        };
        return Some((user.to_owned(), host.to_ascii_lowercase(), port));
    }
    let (identity, _) = remote.split_once(':')?;
    let (user, host) = identity.rsplit_once('@')?;
    if user.is_empty() || host.is_empty() {
        return None;
    }
    Some((user.to_owned(), host.to_ascii_lowercase(), 22))
}

struct PrivateTempDir {
    path: PathBuf,
}

impl PrivateTempDir {
    fn new(prefix: &str) -> Result<Self, GitError> {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random)
            .map_err(|_| GitError::new("temporary directory could not be created", "internal"))?;
        let path = std::env::temp_dir().join(format!("{prefix}-{}", hex(&random)));
        fs::create_dir(&path)
            .map_err(|_| GitError::new("temporary directory could not be created", "internal"))?;
        set_private_dir(&path)?;
        Ok(Self { path })
    }
}

impl Drop for PrivateTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct PinnedKnownHosts {
    _directory: PrivateTempDir,
    file: PathBuf,
}

impl PinnedKnownHosts {
    fn new(scope: &SshScope) -> Result<Self, GitError> {
        let directory = PrivateTempDir::new("ptrack-known-hosts")?;
        let file = directory.path.join("known_hosts");
        let host = if scope.port == 22 {
            scope.host.clone()
        } else {
            format!("[{}]:{}", scope.host, scope.port)
        };
        fs::write(&file, format!("{host} {}\n", scope.host_key))
            .map_err(|_| GitError::new("prepare Git SSH host key: internal", "internal"))?;
        set_private_file(&file)?;
        Ok(Self {
            _directory: directory,
            file,
        })
    }
}

fn git_ssh_command(scope: &SshScope, known_hosts: &Path) -> String {
    let mut args = vec![
        "-F".to_owned(),
        "/dev/null".to_owned(),
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
        "-o".to_owned(),
        "PasswordAuthentication=no".to_owned(),
        "-o".to_owned(),
        "KbdInteractiveAuthentication=no".to_owned(),
        "-o".to_owned(),
        "StrictHostKeyChecking=yes".to_owned(),
        "-o".to_owned(),
        format!("UserKnownHostsFile={}", known_hosts.display()),
        "-o".to_owned(),
        "GlobalKnownHostsFile=/dev/null".to_owned(),
        "-o".to_owned(),
        "PermitLocalCommand=no".to_owned(),
        "-o".to_owned(),
        "ClearAllForwardings=yes".to_owned(),
        "-p".to_owned(),
        scope.port.to_string(),
    ];
    args.insert(0, "ssh".to_owned());
    args.into_iter()
        .map(|arg| shell_quote(&arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(unix)]
fn set_private_dir(path: &Path) -> Result<(), GitError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| GitError::new("temporary directory could not be protected", "internal"))
}

#[cfg(windows)]
fn set_private_dir(path: &Path) -> Result<(), GitError> {
    crate::private_windows::private_windows_acl(path)
}

#[cfg(not(any(unix, windows)))]
fn set_private_dir(_path: &Path) -> Result<(), GitError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<(), GitError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| GitError::new("temporary file could not be protected", "internal"))
}

#[cfg(windows)]
fn set_private_file(path: &Path) -> Result<(), GitError> {
    crate::private_windows::private_windows_acl(path)
}

#[cfg(not(any(unix, windows)))]
fn set_private_file(_path: &Path) -> Result<(), GitError> {
    Ok(())
}
