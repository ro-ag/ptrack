use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ptrack_capability_policy::{
    AuditEvent, Denied, SshOperation, authorize_ssh, normalize_remote_path, resolve_project_path,
};
use ptrack_core::{Capability, SshScope};
use ptrack_store::{Clock, ProjectStore, SystemClock};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::audit::AuditRecorder;
use crate::git::{ProcessRunner, system_runner};
use crate::process::{ProcessError, ProcessResult, ProcessSpec};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshRequest {
    pub operation: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub local_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remote_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub forward_target: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub listen_port: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshResult {
    pub exit_code: i32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdout: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr: String,
    pub diagnostic: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshError {
    message: String,
    class: String,
    result: Box<SshResult>,
}

impl SshError {
    fn new(message: impl Into<String>, class: impl Into<String>) -> Self {
        let class = class.into();
        Self {
            message: message.into(),
            result: Box::new(SshResult {
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
        result: SshResult,
    ) -> Self {
        Self {
            message: message.into(),
            class: class.into(),
            result: Box::new(result),
        }
    }

    #[must_use]
    pub fn class(&self) -> &str {
        &self.class
    }

    #[must_use]
    pub const fn result(&self) -> &SshResult {
        &self.result
    }
}

impl fmt::Display for SshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SshError {}

impl From<Denied> for SshError {
    fn from(error: Denied) -> Self {
        Self::new(error.to_string(), "denied")
    }
}

pub struct SshExecutor<'a> {
    recorder: AuditRecorder<'a>,
    runner: &'a dyn ProcessRunner,
}

impl<'a> SshExecutor<'a> {
    #[must_use]
    pub const fn new(store: Option<&'a ProjectStore>) -> Self {
        Self {
            recorder: AuditRecorder::new(store),
            runner: system_runner(),
        }
    }

    pub(crate) const fn from_parts(
        recorder: AuditRecorder<'a>,
        runner: &'a dyn ProcessRunner,
    ) -> Self {
        Self { recorder, runner }
    }

    /// Executes one fixed, separately authorized SSH operation.
    ///
    /// # Errors
    /// Returns stable policy and diagnostic errors without exposing argv,
    /// local paths, credentials, tokens, or raw subprocess diagnostics.
    pub async fn execute(
        &self,
        cancellation: &CancellationToken,
        capability: &Capability,
        agent_profile: &str,
        project_root: &Path,
        request: &SshRequest,
    ) -> Result<SshResult, SshError> {
        let operation = parse_operation(&request.operation)?;
        let value = match operation {
            SshOperation::LocalForward | SshOperation::RemoteForward => &request.forward_target,
            _ => &request.command,
        };
        let normalized = authorize_ssh(
            capability,
            agent_profile,
            SystemClock.now_utc(),
            operation,
            value,
        )?;
        let scope = normalized
            .ssh
            .as_ref()
            .ok_or_else(|| SshError::new("capability denied: capability is not SSH", "denied"))?;
        let pinned = PinnedKnownHosts::new(scope)?;
        let mut plan = build_plan(&normalized, project_root, &pinned.file, request, operation)?;
        let started = Instant::now();
        let process = self.runner.run(&plan.spec, cancellation).await;
        let mut outcome = process
            .map_err(|error| process_error(error, operation))
            .and_then(|process| complete_process(&mut plan, &process, operation));
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
            target: audit_target(scope),
            success: outcome.is_ok(),
            error_class: outcome
                .as_ref()
                .err()
                .map_or_else(|| "none".to_owned(), |error| error.class.clone()),
            duration_millis: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
            request_bytes: plan.request_bytes,
            response_bytes: response_bytes.saturating_add(plan.transfer_bytes),
            redirects: 0,
        };
        if let Err(error) = self.recorder.record(&normalized, &event)
            && outcome.is_ok()
        {
            outcome = Err(SshError::new(error.to_string(), "internal"));
        }
        outcome
    }
}

struct SshPlan {
    spec: ProcessSpec,
    completion: Option<DownloadCompletion>,
    _upload: Option<StagedUpload>,
    request_bytes: i64,
    transfer_bytes: i64,
}

fn build_plan(
    capability: &Capability,
    project_root: &Path,
    known_hosts: &Path,
    request: &SshRequest,
    operation: SshOperation,
) -> Result<SshPlan, SshError> {
    let Some(scope) = capability.ssh.as_ref() else {
        return Err(SshError::new(
            "capability denied: capability is not SSH",
            "denied",
        ));
    };
    let mut args = ssh_base_args(scope, known_hosts, false);
    let target = format!("{}@{}", scope.user, scope.host);
    let mut name = OsString::from("ssh");
    let mut completion = None;
    let mut upload = None;
    let mut maximum = u64::try_from(capability.limits.max_output_bytes).unwrap_or_default();
    let mut request_bytes = 0;
    match operation {
        SshOperation::Git => {
            return Err(SshError::new(
                "capability denied: Git-over-SSH must be invoked through the Git capability intersection",
                "denied",
            ));
        }
        SshOperation::RemoteCommand => {
            args.extend(os_args(["-T", &target, &request.command]));
        }
        SshOperation::Upload => {
            let (staged, upload_args) = build_upload(
                scope,
                project_root,
                known_hosts,
                request,
                &target,
                capability.limits.max_request_bytes,
            )?;
            request_bytes = staged.length;
            name = OsString::from("scp");
            args = upload_args;
            upload = Some(staged);
        }
        SshOperation::Download => {
            let (download, command) = build_download(
                scope,
                project_root,
                request,
                capability.limits.max_response_bytes,
            )?;
            completion = Some(download);
            maximum = u64::try_from(capability.limits.max_response_bytes).unwrap_or_default();
            args.extend(os_args(["-T", &target, &command]));
        }
        SshOperation::InteractiveShell => {
            return Err(SshError::new(
                "capability denied: interactive SSH shells are unavailable through the capability broker transport",
                "denied",
            ));
        }
        SshOperation::LocalForward | SshOperation::RemoteForward => {
            append_forward_args(&mut args, request, operation, &target)?;
        }
    }
    Ok(SshPlan {
        spec: ProcessSpec {
            name,
            args,
            env: vec![
                (OsString::from("LC_ALL"), OsString::from("C")),
                (OsString::from("LANG"), OsString::from("C")),
            ],
            max_output_bytes: maximum,
            timeout: Duration::from_secs(
                u64::try_from(capability.limits.timeout_seconds).unwrap_or_default(),
            ),
        },
        completion,
        _upload: upload,
        request_bytes,
        transfer_bytes: 0,
    })
}

fn build_upload(
    scope: &SshScope,
    project_root: &Path,
    known_hosts: &Path,
    request: &SshRequest,
    target: &str,
    maximum: i64,
) -> Result<(StagedUpload, Vec<OsString>), SshError> {
    let remote_path = exact_remote_path(&request.remote_path, "upload")?;
    if !any_remote_within(&scope.upload_remote_roots, remote_path) {
        return Err(SshError::new(
            "capability denied: upload path is outside approved roots",
            "denied",
        ));
    }
    let staged = StagedUpload::new(
        project_root,
        &request.local_path,
        &scope.upload_roots,
        maximum,
    )?;
    let mut args = ssh_base_args(scope, known_hosts, true);
    args.push(OsString::from("--"));
    args.push(staged.path().as_os_str().to_owned());
    args.push(OsString::from(format!("{target}:{remote_path}")));
    Ok((staged, args))
}

fn build_download(
    scope: &SshScope,
    project_root: &Path,
    request: &SshRequest,
    maximum: i64,
) -> Result<(DownloadCompletion, String), SshError> {
    let remote_path = exact_remote_path(&request.remote_path, "download")?;
    if !any_remote_within(&scope.download_remote_roots, remote_path) {
        return Err(SshError::new(
            "capability denied: download path is outside approved roots",
            "denied",
        ));
    }
    let completion = DownloadCompletion::new(
        project_root,
        &request.local_path,
        &scope.download_roots,
        maximum,
    )?;
    Ok((completion, format!("cat -- {remote_path}")))
}

fn append_forward_args(
    args: &mut Vec<OsString>,
    request: &SshRequest,
    operation: SshOperation,
    target: &str,
) -> Result<(), SshError> {
    if request.listen_port == 0 {
        return Err(SshError::new(
            "capability denied: forward listen port is invalid",
            "denied",
        ));
    }
    let flag = if operation == SshOperation::LocalForward {
        "-L"
    } else {
        "-R"
    };
    let forward = format!(
        "127.0.0.1:{}:{}",
        request.listen_port, request.forward_target
    );
    args.extend(os_args([
        "-o",
        "ClearAllForwardings=no",
        "-o",
        "ExitOnForwardFailure=yes",
        "-N",
        flag,
        &forward,
        target,
    ]));
    Ok(())
}

fn complete_process(
    plan: &mut SshPlan,
    process: &ProcessResult,
    operation: SshOperation,
) -> Result<SshResult, SshError> {
    let stdout = String::from_utf8_lossy(&process.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&process.stderr).into_owned();
    let mut result = SshResult {
        exit_code: process.exit_code,
        stdout,
        stderr,
        diagnostic: "none".to_owned(),
    };
    if process.truncated {
        let class = if operation == SshOperation::Download {
            "response-limit"
        } else {
            "output-limit"
        };
        class.clone_into(&mut result.diagnostic);
        let message = if operation == SshOperation::Download {
            "HTTP response exceeds its byte limit"
        } else {
            "process output exceeds its byte limit"
        };
        return Err(SshError::with_result(message, class, result));
    }
    if process.exit_code != 0 {
        let class = classify_ssh_exit(&result.stderr);
        class.clone_into(&mut result.diagnostic);
        return Err(SshError::with_result(
            format!("SSH operation failed: {class}"),
            class,
            result,
        ));
    }
    if let Some(completion) = plan.completion.take() {
        plan.transfer_bytes = i64::try_from(process.stdout.len()).unwrap_or(i64::MAX);
        completion.install(&process.stdout)?;
        result.stdout.clear();
    }
    Ok(result)
}

fn parse_operation(raw: &str) -> Result<SshOperation, SshError> {
    match raw {
        "git" => Ok(SshOperation::Git),
        "remote-command" => Ok(SshOperation::RemoteCommand),
        "upload" => Ok(SshOperation::Upload),
        "download" => Ok(SshOperation::Download),
        "interactive-shell" => Ok(SshOperation::InteractiveShell),
        "local-forward" => Ok(SshOperation::LocalForward),
        "remote-forward" => Ok(SshOperation::RemoteForward),
        _ => Err(SshError::new(
            format!("capability denied: unknown SSH operation {raw:?}"),
            "denied",
        )),
    }
}

pub(crate) fn ssh_base_args(scope: &SshScope, known_hosts: &Path, scp: bool) -> Vec<OsString> {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    os_args([
        "-F",
        null,
        "-o",
        "BatchMode=yes",
        "-o",
        "PasswordAuthentication=no",
        "-o",
        "KbdInteractiveAuthentication=no",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        &format!("UserKnownHostsFile={}", known_hosts.display()),
        "-o",
        &format!("GlobalKnownHostsFile={null}"),
        "-o",
        "PermitLocalCommand=no",
        "-o",
        "ClearAllForwardings=yes",
        if scp { "-P" } else { "-p" },
        &scope.port.to_string(),
    ])
}

fn os_args<const N: usize>(values: [&str; N]) -> Vec<OsString> {
    values.into_iter().map(OsString::from).collect()
}

pub(crate) struct PinnedKnownHosts {
    _directory: PrivateDirectory,
    file: PathBuf,
}

impl PinnedKnownHosts {
    pub(crate) fn new(scope: &SshScope) -> Result<Self, SshError> {
        let directory = PrivateDirectory::new("ptrack-known-hosts")?;
        let file = directory.path.join("known_hosts");
        let host = if scope.port == 22 {
            scope.host.clone()
        } else {
            format!("[{}]:{}", scope.host, scope.port)
        };
        fs::write(&file, format!("{host} {}\n", scope.host_key))
            .map_err(|_| SshError::new("prepare pinned host key: internal", "internal"))?;
        protect_private_file(&file)?;
        Ok(Self {
            _directory: directory,
            file,
        })
    }

    pub(crate) fn file(&self) -> &Path {
        &self.file
    }
}

struct PrivateDirectory {
    path: PathBuf,
}

impl PrivateDirectory {
    fn new(prefix: &str) -> Result<Self, SshError> {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random)
            .map_err(|_| SshError::new("temporary directory could not be created", "internal"))?;
        let path = std::env::temp_dir().join(format!("{prefix}-{}", hex(&random)));
        fs::create_dir(&path)
            .map_err(|_| SshError::new("temporary directory could not be created", "internal"))?;
        if let Err(error) = protect_private_dir(&path) {
            let _ = fs::remove_dir_all(&path);
            return Err(error);
        }
        Ok(Self { path })
    }
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct StagedUpload {
    _directory: PrivateDirectory,
    path: PathBuf,
    length: i64,
}

impl StagedUpload {
    fn new(
        project_root: &Path,
        requested: &str,
        approved_roots: &[String],
        maximum: i64,
    ) -> Result<Self, SshError> {
        let source_path = resolve_project_path(project_root, requested, approved_roots, true)
            .map_err(|_| {
                SshError::new(
                    "capability denied: upload path is outside approved roots",
                    "denied",
                )
            })?;
        let mut source = File::open(&source_path)
            .map_err(|_| SshError::new("upload source is unavailable", "internal"))?;
        let identity = FileIdentity::capture(&source)
            .map_err(|_| SshError::new("upload source must be a regular file", "denied"))?;
        if !identity.regular {
            return Err(SshError::new(
                "capability denied: upload source must be a regular file",
                "denied",
            ));
        }
        verify_upload_identity(project_root, requested, approved_roots, &identity)?;
        let directory = PrivateDirectory::new("ptrack-upload")?;
        let path = directory.path.join("payload");
        let mut staged = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|_| SshError::new("upload staging failed", "internal"))?;
        protect_private_file(&path)?;
        let allowed = u64::try_from(maximum).unwrap_or_default();
        let mut bounded = Read::by_ref(&mut source).take(allowed.saturating_add(1));
        let length = std::io::copy(&mut bounded, &mut staged)
            .map_err(|_| SshError::new("upload staging failed", "internal"))?;
        staged
            .sync_all()
            .map_err(|_| SshError::new("upload staging failed", "internal"))?;
        if length > allowed {
            return Err(SshError::new(
                "transfer request exceeds its byte limit",
                "request-limit",
            ));
        }
        verify_upload_identity(project_root, requested, approved_roots, &identity)?;
        protect_read_only_file(&path)?;
        Ok(Self {
            _directory: directory,
            path,
            length: i64::try_from(length).unwrap_or(i64::MAX),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

struct DownloadCompletion {
    project_root: PathBuf,
    requested: String,
    approved_roots: Vec<String>,
    destination: PathBuf,
    maximum: i64,
    directory: PrivateDirectory,
}

impl DownloadCompletion {
    fn new(
        project_root: &Path,
        requested: &str,
        approved_roots: &[String],
        maximum: i64,
    ) -> Result<Self, SshError> {
        let destination = resolve_project_path(project_root, requested, approved_roots, false)
            .map_err(|_| {
                SshError::new(
                    "capability denied: download path is outside approved roots",
                    "denied",
                )
            })?;
        let project_root = project_root.canonicalize().map_err(|_| {
            SshError::new(
                "capability denied: project root cannot be canonicalized",
                "denied",
            )
        })?;
        Ok(Self {
            project_root,
            requested: requested.to_owned(),
            approved_roots: approved_roots.to_vec(),
            destination,
            maximum,
            directory: PrivateDirectory::new("ptrack-download")?,
        })
    }

    fn install(self, payload: &[u8]) -> Result<(), SshError> {
        if i64::try_from(payload.len()).unwrap_or(i64::MAX) > self.maximum {
            return Err(SshError::new(
                "HTTP response exceeds its byte limit",
                "response-limit",
            ));
        }
        let staged = self.directory.path.join("payload");
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&staged)
            .map_err(|_| SshError::new("download staging failed", "internal"))?;
        protect_private_file(&staged)?;
        file.write_all(payload)
            .and_then(|()| file.sync_all())
            .map_err(|_| SshError::new("download staging failed", "internal"))?;
        let current = resolve_project_path(
            &self.project_root,
            &self.requested,
            &self.approved_roots,
            false,
        );
        if current.as_ref().ok() != Some(&self.destination) {
            return Err(SshError::new(
                "capability denied: download destination changed during transfer",
                "denied",
            ));
        }
        install_download(
            &self.project_root,
            &self.destination,
            &staged,
            &file,
            self.maximum,
        )
    }
}

struct FileIdentity {
    regular: bool,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    length: u64,
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
}

impl FileIdentity {
    fn capture(file: &File) -> std::io::Result<Self> {
        let metadata = file.metadata()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            Ok(Self {
                regular: metadata.is_file(),
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                regular: metadata.is_file(),
                length: metadata.len(),
                modified: metadata.modified().ok(),
            })
        }
    }

    fn matches_path(&self, path: &Path) -> bool {
        let Ok(metadata) = fs::metadata(path) else {
            return false;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            metadata.is_file() && self.device == metadata.dev() && self.inode == metadata.ino()
        }
        #[cfg(not(unix))]
        {
            metadata.is_file()
                && self.length == metadata.len()
                && self.modified == metadata.modified().ok()
        }
    }
}

fn verify_upload_identity(
    project_root: &Path,
    requested: &str,
    roots: &[String],
    identity: &FileIdentity,
) -> Result<(), SshError> {
    let current = resolve_project_path(project_root, requested, roots, true);
    if !current.is_ok_and(|path| identity.matches_path(&path)) {
        return Err(SshError::new(
            "capability denied: upload source changed during verification",
            "denied",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn install_download(
    project: &Path,
    destination: &Path,
    staged: &Path,
    _staged_file: &File,
    maximum: i64,
) -> Result<(), SshError> {
    use rustix::fs::{Mode, OFlags};
    let (directory, final_name) = open_download_parent(project, destination)?;
    let mut source = open_download_source(staged)?;
    let temporary_name = format!(".ptrack-download-{}", random_hex(16)?);
    let temporary = rustix::fs::openat(
        &directory,
        temporary_name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|_| SshError::new("download install failed", "internal"))?;
    let mut temporary_file = File::from(temporary);
    let allowed = u64::try_from(maximum).unwrap_or_default();
    let install = copy_and_rename_download(
        &directory,
        final_name.as_os_str(),
        temporary_name.as_str(),
        &mut source,
        &mut temporary_file,
        allowed,
    );
    if install.is_err() {
        use std::os::fd::AsFd as _;
        unlink_owned_download_temp(
            directory.as_fd(),
            temporary_name.as_str(),
            temporary_file.as_fd(),
        );
    }
    install?;
    rustix::fs::fsync(&directory).map_err(|_| SshError::new("download install failed", "internal"))
}

#[cfg(unix)]
fn open_download_parent(
    project: &Path,
    destination: &Path,
) -> Result<(std::os::fd::OwnedFd, OsString), SshError> {
    use rustix::fs::{Mode, OFlags};
    let relative = destination.strip_prefix(project).map_err(|_| {
        SshError::new(
            "capability denied: download destination escapes the project",
            "denied",
        )
    })?;
    let mut parts = relative.components().peekable();
    let final_name = relative.file_name().ok_or_else(|| {
        SshError::new(
            "capability denied: download destination escapes the project",
            "denied",
        )
    })?;
    let mut directory = rustix::fs::open(
        project,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|_| {
        SshError::new(
            "download destination parent is not a stable project directory",
            "denied",
        )
    })?;
    while let Some(component) = parts.next() {
        if parts.peek().is_none() {
            break;
        }
        let std::path::Component::Normal(name) = component else {
            return Err(SshError::new(
                "capability denied: download destination escapes the project",
                "denied",
            ));
        };
        directory = rustix::fs::openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|_| {
            SshError::new(
                "capability denied: download destination parent is not a stable project directory",
                "denied",
            )
        })?;
    }
    Ok((directory, final_name.to_os_string()))
}

#[cfg(unix)]
fn open_download_source(staged: &Path) -> Result<File, SshError> {
    use rustix::fs::{Mode, OFlags};
    let source = rustix::fs::open(
        staged,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|_| {
        SshError::new(
            "capability denied: download staging file is invalid",
            "denied",
        )
    })?;
    let stat = rustix::fs::fstat(&source).map_err(|_| {
        SshError::new(
            "capability denied: download staging file is invalid",
            "denied",
        )
    })?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile {
        return Err(SshError::new(
            "capability denied: download staging file is invalid",
            "denied",
        ));
    }
    Ok(File::from(source))
}

#[cfg(unix)]
fn copy_and_rename_download(
    directory: &std::os::fd::OwnedFd,
    final_name: &std::ffi::OsStr,
    temporary_name: &str,
    source: &mut File,
    temporary: &mut File,
    allowed: u64,
) -> Result<(), SshError> {
    let copied = std::io::copy(
        &mut Read::by_ref(source).take(allowed.saturating_add(1)),
        temporary,
    )
    .map_err(|_| SshError::new("download install failed", "internal"))?;
    if copied > allowed {
        return Err(SshError::new(
            "HTTP response exceeds its byte limit",
            "response-limit",
        ));
    }
    temporary
        .sync_all()
        .map_err(|_| SshError::new("download install failed", "internal"))?;
    rustix::fs::renameat(directory, temporary_name, directory, final_name)
        .map_err(|_| SshError::new("download install failed", "internal"))
}

#[cfg(unix)]
pub(crate) fn unlink_owned_download_temp(
    directory: std::os::fd::BorrowedFd<'_>,
    name: &str,
    expected: std::os::fd::BorrowedFd<'_>,
) {
    use rustix::fs::AtFlags;
    let Ok(expected) = rustix::fs::fstat(expected) else {
        return;
    };
    let Ok(current) = rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) else {
        return;
    };
    if current.st_dev == expected.st_dev && current.st_ino == expected.st_ino {
        let _ = rustix::fs::unlinkat(directory, name, AtFlags::empty());
    }
}

#[cfg(windows)]
fn install_download(
    project: &Path,
    destination: &Path,
    _staged: &Path,
    staged_file: &File,
    maximum: i64,
) -> Result<(), SshError> {
    crate::private_windows::install_download(project, destination, staged_file, maximum)
        .map_err(|message| SshError::new(message, "denied"))
}

#[cfg(not(any(unix, windows)))]
fn install_download(
    _project: &Path,
    _destination: &Path,
    _staged: &Path,
    _staged_file: &File,
    _maximum: i64,
) -> Result<(), SshError> {
    Err(SshError::new(
        "capability denied: download destination parent is not a stable project directory",
        "denied",
    ))
}

#[cfg(unix)]
fn protect_private_dir(path: &Path) -> Result<(), SshError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| SshError::new("temporary directory could not be protected", "internal"))
}

#[cfg(windows)]
fn protect_private_dir(path: &Path) -> Result<(), SshError> {
    crate::private_windows::protect_private_path(path)
        .map_err(|()| SshError::new("temporary directory could not be protected", "internal"))
}

#[cfg(not(any(unix, windows)))]
fn protect_private_dir(_path: &Path) -> Result<(), SshError> {
    Ok(())
}

#[cfg(unix)]
fn protect_private_file(path: &Path) -> Result<(), SshError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| SshError::new("temporary file could not be protected", "internal"))
}

#[cfg(windows)]
fn protect_private_file(path: &Path) -> Result<(), SshError> {
    crate::private_windows::protect_private_path(path)
        .map_err(|()| SshError::new("temporary file could not be protected", "internal"))
}

#[cfg(not(any(unix, windows)))]
fn protect_private_file(_path: &Path) -> Result<(), SshError> {
    Ok(())
}

#[cfg(unix)]
fn protect_read_only_file(path: &Path) -> Result<(), SshError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o400))
        .map_err(|_| SshError::new("upload staging failed", "internal"))
}

#[cfg(not(unix))]
fn protect_read_only_file(path: &Path) -> Result<(), SshError> {
    protect_private_file(path)
}

fn process_error(error: ProcessError, operation: SshOperation) -> SshError {
    let class = match error {
        ProcessError::Cancelled => "cancelled",
        ProcessError::Timeout => "timeout",
        ProcessError::Spawn | ProcessError::Wait => "transport",
    };
    let mut result = SshResult {
        exit_code: -1,
        stdout: String::new(),
        stderr: String::new(),
        diagnostic: class.to_owned(),
    };
    if operation == SshOperation::Download && class == "output-limit" {
        "response-limit".clone_into(&mut result.diagnostic);
    }
    SshError::with_result(format!("SSH operation failed: {class}"), class, result)
}

#[must_use]
pub fn classify_ssh_error(error: &SshError) -> &str {
    error.class()
}

#[must_use]
pub fn classify_ssh_exit(stderr: &str) -> &'static str {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("could not resolve hostname") {
        "dns"
    } else if lower.contains("host key verification failed")
        || lower.contains("remote host identification has changed")
    {
        "host-key"
    } else if lower.contains("permission denied")
        || lower.contains("no more authentication methods")
    {
        "authentication"
    } else if lower.contains("network is unreachable")
        || lower.contains("no route to host")
        || lower.contains("connection refused")
    {
        "routing"
    } else if lower.contains("operation timed out") || lower.contains("connection timed out") {
        "timeout"
    } else if lower.contains("administratively prohibited") || lower.contains("not allowed") {
        "remote-policy"
    } else if lower.contains("operation not permitted") {
        "sandbox"
    } else {
        "transport"
    }
}

fn any_remote_within(roots: &[String], candidate: &str) -> bool {
    roots.iter().any(|root| {
        candidate == root
            || (root != "/"
                && candidate
                    .strip_prefix(root)
                    .is_some_and(|suffix| suffix.starts_with('/')))
    })
}

fn exact_remote_path<'a>(candidate: &'a str, operation: &str) -> Result<&'a str, SshError> {
    if normalize_remote_path(candidate).is_ok_and(|normalized| normalized == candidate) {
        return Ok(candidate);
    }
    Err(SshError::new(
        format!("capability denied: {operation} path is outside approved roots"),
        "denied",
    ))
}

fn audit_target(scope: &SshScope) -> String {
    if scope.host.contains(':') {
        format!("[{}]:{}", scope.host, scope.port)
    } else {
        format!("{}:{}", scope.host, scope.port)
    }
}

#[cfg(unix)]
fn random_hex(length: usize) -> Result<String, SshError> {
    let mut bytes = vec![0_u8; length];
    getrandom::fill(&mut bytes)
        .map_err(|_| SshError::new("temporary name could not be created", "internal"))?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_zero(value: &u16) -> bool {
    *value == 0
}
