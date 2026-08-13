use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use ptrack_core::{CapabilityKind, SshScope};
use tokio_util::sync::CancellationToken;

use super::AuditRecorder;
use super::git::{ProcessRunner, RunFuture};
use super::process::{ProcessResult, ProcessSpec};
#[cfg(unix)]
use super::ssh::unlink_owned_download_temp;
use super::ssh::{SshExecutor, SshRequest, classify_ssh_exit};
use super::test_support::{TempDir, draft, refresh_approval};

pub(super) fn assert_cap_077_through_084_ssh_contract() {
    assert_eq!(
        classify_ssh_exit("Host key verification failed"),
        "host-key"
    );
    assert_eq!(
        classify_ssh_exit("Permission denied (publickey)"),
        "authentication"
    );
    assert_eq!(classify_ssh_exit("No route to host"), "routing");
    assert_eq!(
        classify_ssh_exit("Administratively prohibited"),
        "remote-policy"
    );
}

#[tokio::test]
async fn ssh_exact_argv_and_noncomposable_grants_deny_before_spawn() {
    let temp = TempDir::new("ssh-command");
    let capability = ssh_capability(|scope| {
        scope.remote_commands = vec!["printf safe".to_owned()];
    });
    let runner = RecordingRunner::success(Vec::new());
    let executor = SshExecutor::from_parts(AuditRecorder::new(None), &runner);
    let denied = executor
        .execute(
            &CancellationToken::new(),
            &capability,
            "agent-codex",
            temp.path(),
            &SshRequest {
                operation: "remote-command".to_owned(),
                command: "printf safe; touch escaped".to_owned(),
                local_path: String::new(),
                remote_path: String::new(),
                forward_target: String::new(),
                listen_port: 0,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(denied.class(), "denied");
    assert!(runner.calls.lock().unwrap().is_empty());

    executor
        .execute(
            &CancellationToken::new(),
            &capability,
            "agent-codex",
            temp.path(),
            &SshRequest {
                operation: "remote-command".to_owned(),
                command: "printf safe".to_owned(),
                local_path: String::new(),
                remote_path: String::new(),
                forward_target: String::new(),
                listen_port: 0,
            },
        )
        .await
        .unwrap();
    let calls = runner.calls.lock().unwrap();
    let args = strings(&calls[0].args);
    assert_eq!(calls[0].name, OsString::from("ssh"));
    let null_config = if cfg!(windows) { "NUL" } else { "/dev/null" };
    assert_eq!(&args[..4], ["-F", null_config, "-o", "BatchMode=yes"]);
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-o", "PasswordAuthentication=no"])
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-o", "StrictHostKeyChecking=yes"])
    );
    assert_eq!(
        &args[args.len() - 3..],
        ["-T", "deploy@example.test", "printf safe"]
    );
}

#[tokio::test]
async fn upload_is_immutable_bounded_staged_copy_and_scp_is_direct() {
    let temp = TempDir::new("ssh-upload");
    fs::create_dir(temp.path().join("uploads")).unwrap();
    fs::write(temp.path().join("uploads/source.txt"), b"payload").unwrap();
    let capability = ssh_capability(|scope| {
        scope.allow_upload = true;
        scope.upload_roots = vec!["uploads".to_owned()];
        scope.upload_remote_roots = vec!["/srv/uploads".to_owned()];
    });
    let runner = RecordingRunner::success(Vec::new());
    let result = SshExecutor::from_parts(AuditRecorder::new(None), &runner)
        .execute(
            &CancellationToken::new(),
            &capability,
            "agent-codex",
            temp.path(),
            &SshRequest {
                operation: "upload".to_owned(),
                command: String::new(),
                local_path: "uploads/source.txt".to_owned(),
                remote_path: "/srv/uploads/file.txt".to_owned(),
                forward_target: String::new(),
                listen_port: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(result.diagnostic, "none");
    let calls = runner.calls.lock().unwrap();
    assert_eq!(calls[0].name, OsString::from("scp"));
    let args = strings(&calls[0].args);
    assert!(args.contains(&"-P".to_owned()));
    assert_eq!(args[args.len() - 3], "--");
    assert_eq!(
        args.last().unwrap(),
        "deploy@example.test:/srv/uploads/file.txt"
    );
    assert_eq!(
        runner.staged.lock().unwrap().as_deref(),
        Some(b"payload".as_slice())
    );
}

#[tokio::test]
async fn download_payload_is_not_returned_and_parent_swap_fails_closed() {
    let temp = TempDir::new("ssh-download");
    fs::create_dir(temp.path().join("downloads")).unwrap();
    let capability = ssh_capability(|scope| {
        scope.allow_download = true;
        scope.download_roots = vec!["downloads".to_owned()];
        scope.download_remote_roots = vec!["/srv/downloads".to_owned()];
    });
    let runner = RecordingRunner::success(b"downloaded".to_vec());
    let result = SshExecutor::from_parts(AuditRecorder::new(None), &runner)
        .execute(
            &CancellationToken::new(),
            &capability,
            "agent-codex",
            temp.path(),
            &download_request(),
        )
        .await
        .unwrap();
    assert!(result.stdout.is_empty());
    assert_eq!(
        fs::read(temp.path().join("downloads/result.bin")).unwrap(),
        b"downloaded"
    );

    #[cfg(unix)]
    {
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let runner =
            RecordingRunner::with_swap(b"hostile".to_vec(), temp.path().join("downloads"), outside);
        let error = SshExecutor::from_parts(AuditRecorder::new(None), &runner)
            .execute(
                &CancellationToken::new(),
                &capability,
                "agent-codex",
                temp.path(),
                &download_request(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.class(), "denied");
    }
}

#[tokio::test]
async fn transfer_remote_paths_are_exact_normalized_shell_safe_and_deny_before_spawn() {
    let temp = TempDir::new("ssh-remote-paths");
    fs::create_dir(temp.path().join("uploads")).unwrap();
    fs::create_dir(temp.path().join("downloads")).unwrap();
    fs::write(temp.path().join("uploads/source.txt"), b"payload").unwrap();
    let capability = ssh_capability(|scope| {
        scope.allow_upload = true;
        scope.allow_download = true;
        scope.upload_roots = vec!["uploads".to_owned()];
        scope.download_roots = vec!["downloads".to_owned()];
        scope.upload_remote_roots = vec!["/srv/uploads".to_owned()];
        scope.download_remote_roots = vec!["/srv/downloads".to_owned()];
    });
    let hostile = [
        "/srv/uploads/file;touch-pwned",
        "/srv/uploads/two words",
        "/srv/uploads/$(touch-pwned)",
        "/srv/uploads/`touch-pwned`",
        "/srv/uploads/'quoted'",
        "/srv/uploads/\"quoted\"",
        "/srv/uploads/control\nname",
        "/srv/uploads/../outside",
        "/srv/uploads//file",
        "/srv/uploads/./file",
    ];
    for &remote_path in &hostile {
        let runner = RecordingRunner::success(Vec::new());
        let error = SshExecutor::from_parts(AuditRecorder::new(None), &runner)
            .execute(
                &CancellationToken::new(),
                &capability,
                "agent-codex",
                temp.path(),
                &SshRequest {
                    operation: "upload".to_owned(),
                    command: String::new(),
                    local_path: "uploads/source.txt".to_owned(),
                    remote_path: remote_path.to_owned(),
                    forward_target: String::new(),
                    listen_port: 0,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.class(), "denied");
        assert!(runner.calls.lock().unwrap().is_empty(), "{remote_path:?}");
    }
    for remote_path in hostile.map(|path| path.replacen("uploads", "downloads", 1)) {
        let runner = RecordingRunner::success(Vec::new());
        let error = SshExecutor::from_parts(AuditRecorder::new(None), &runner)
            .execute(
                &CancellationToken::new(),
                &capability,
                "agent-codex",
                temp.path(),
                &SshRequest {
                    operation: "download".to_owned(),
                    command: String::new(),
                    local_path: "downloads/result.bin".to_owned(),
                    remote_path: remote_path.clone(),
                    forward_target: String::new(),
                    listen_port: 0,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.class(), "denied");
        assert!(runner.calls.lock().unwrap().is_empty(), "{remote_path:?}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn download_install_cleans_owned_temps_without_deleting_replacements() {
    use std::fs::OpenOptions;
    use std::os::fd::AsFd as _;

    let temp = TempDir::new("ssh-download-cleanup");
    let downloads = temp.path().join("downloads");
    fs::create_dir(&downloads).unwrap();
    let mut capability = ssh_capability(|scope| {
        scope.allow_download = true;
        scope.download_roots = vec!["downloads".to_owned()];
        scope.download_remote_roots = vec!["/srv/downloads".to_owned()];
    });
    capability.limits.max_response_bytes = 4;
    capability = refresh_approval(capability);
    let error = SshExecutor::from_parts(
        AuditRecorder::new(None),
        &RecordingRunner::success(b"too-large".to_vec()),
    )
    .execute(
        &CancellationToken::new(),
        &capability,
        "agent-codex",
        temp.path(),
        &download_request(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.class(), "response-limit");
    assert_no_download_temps(&downloads);

    fs::create_dir(downloads.join("result.bin")).unwrap();
    capability.limits.max_response_bytes = 1024;
    capability = refresh_approval(capability);
    let error = SshExecutor::from_parts(
        AuditRecorder::new(None),
        &RecordingRunner::success(b"payload".to_vec()),
    )
    .execute(
        &CancellationToken::new(),
        &capability,
        "agent-codex",
        temp.path(),
        &download_request(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.class(), "internal");
    assert_no_download_temps(&downloads);

    let directory = OpenOptions::new().read(true).open(&downloads).unwrap();
    let owned_path = downloads.join(".ptrack-download-owned");
    fs::write(&owned_path, b"owned").unwrap();
    let owned = OpenOptions::new().read(true).open(&owned_path).unwrap();
    fs::rename(&owned_path, downloads.join("moved-owned")).unwrap();
    fs::write(&owned_path, b"canary").unwrap();
    unlink_owned_download_temp(directory.as_fd(), ".ptrack-download-owned", owned.as_fd());
    assert_eq!(fs::read(&owned_path).unwrap(), b"canary");
}

#[cfg(unix)]
fn assert_no_download_temps(directory: &std::path::Path) {
    assert!(fs::read_dir(directory).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".ptrack-download-")
    }));
}

fn ssh_capability(edit: impl FnOnce(&mut SshScope)) -> ptrack_core::Capability {
    let mut capability = draft(CapabilityKind::Ssh);
    let mut scope = SshScope {
        alias: "deploy".to_owned(),
        host: "example.test".to_owned(),
        port: 22,
        user: "deploy".to_owned(),
        host_key: "ssh-ed25519 QUJDREVGR0hJSktMTU5PUA==".to_owned(),
        allow_git: false,
        remote_commands: Vec::new(),
        allow_upload: false,
        allow_download: false,
        upload_roots: Vec::new(),
        download_roots: Vec::new(),
        upload_remote_roots: Vec::new(),
        download_remote_roots: Vec::new(),
        allow_interactive_shell: false,
        local_forward_targets: Vec::new(),
        remote_forward_targets: Vec::new(),
    };
    edit(&mut scope);
    capability.ssh = Some(scope);
    refresh_approval(capability)
}

fn download_request() -> SshRequest {
    SshRequest {
        operation: "download".to_owned(),
        command: String::new(),
        local_path: "downloads/result.bin".to_owned(),
        remote_path: "/srv/downloads/result.bin".to_owned(),
        forward_target: String::new(),
        listen_port: 0,
    }
}

#[derive(Clone, Debug)]
struct CapturedSpec {
    name: OsString,
    args: Vec<OsString>,
}

struct RecordingRunner {
    calls: Mutex<Vec<CapturedSpec>>,
    stdout: Vec<u8>,
    staged: Mutex<Option<Vec<u8>>>,
    #[cfg(unix)]
    swap: Option<(PathBuf, PathBuf)>,
}

impl RecordingRunner {
    fn success(stdout: Vec<u8>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            stdout,
            staged: Mutex::new(None),
            #[cfg(unix)]
            swap: None,
        }
    }

    #[cfg(unix)]
    fn with_swap(stdout: Vec<u8>, path: PathBuf, target: PathBuf) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            stdout,
            staged: Mutex::new(None),
            swap: Some((path, target)),
        }
    }
}

impl ProcessRunner for RecordingRunner {
    fn run<'a>(
        &'a self,
        spec: &'a ProcessSpec,
        _cancellation: &'a CancellationToken,
    ) -> RunFuture<'a> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(CapturedSpec {
                name: spec.name.clone(),
                args: spec.args.clone(),
            });
            if spec.name == "scp" {
                let path = PathBuf::from(&spec.args[spec.args.len() - 2]);
                *self.staged.lock().unwrap() = fs::read(path).ok();
            }
            #[cfg(unix)]
            if let Some((path, target)) = &self.swap {
                let old = path.with_extension("old");
                fs::rename(path, &old).unwrap();
                std::os::unix::fs::symlink(target, path).unwrap();
            }
            Ok(ProcessResult {
                exit_code: 0,
                stdout: self.stdout.clone(),
                stderr: Vec::new(),
                truncated: false,
            })
        })
    }
}

fn strings(values: &[OsString]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect()
}
