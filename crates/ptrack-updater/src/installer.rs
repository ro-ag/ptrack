use std::future::Future;
#[cfg(any(target_os = "linux", test))]
use std::io;
use std::path::Path;
#[cfg(any(target_os = "linux", windows, test))]
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::discovery::{Target, UpdateError};
use crate::staging::{StageKind, StagedUpdate, validate_stage};

/// Upper bound for a trust check over a staged package (`hdiutil verify`,
/// `codesign`, `spctl`), which reads the whole image and may consult the
/// notarization service.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) const VERIFY_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
/// Upper bound for handing a verified package to the platform installer.
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
pub(crate) const HANDOFF_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// Upper bound for the `ptrack version` smoke test of a replaced binary.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(crate) const SMOKE_TEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long pipe readers may run on after the command itself has exited.
const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);
const MAX_COMMAND_OUTPUT: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApplyAction {
    InstalledRestartRequired,
    OpenedNativeInstaller,
    RevealedVerifiedArchive,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    pub version: String,
    pub action: ApplyAction,
    pub restart_required: bool,
    pub manual_install: bool,
    pub cleanup_pending: bool,
}

pub(crate) type CommandFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<u8>, UpdateError>> + Send + 'a>>;

pub(crate) trait CommandRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        cancellation: &'a CancellationToken,
        program: &'a Path,
        arguments: &'a [String],
        timeout: Duration,
    ) -> CommandFuture<'a>;
}

struct ProductionCommandRunner;

impl CommandRunner for ProductionCommandRunner {
    fn run<'a>(
        &'a self,
        cancellation: &'a CancellationToken,
        program: &'a Path,
        arguments: &'a [String],
        timeout: Duration,
    ) -> CommandFuture<'a> {
        Box::pin(run_bounded_command(
            cancellation,
            program,
            arguments,
            timeout,
        ))
    }
}

pub struct Installer {
    #[cfg(any(target_os = "linux", test))]
    current_executable: Arc<dyn Fn() -> io::Result<PathBuf> + Send + Sync>,
    runner: Arc<dyn CommandRunner>,
}

impl Default for Installer {
    fn default() -> Self {
        Self::new()
    }
}

impl Installer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            #[cfg(any(target_os = "linux", test))]
            current_executable: Arc::new(std::env::current_exe),
            runner: Arc::new(ProductionCommandRunner),
        }
    }

    /// Revalidates and applies or hands off a stage for the running host only.
    ///
    /// # Errors
    /// Returns a validation, target, command, replacement, or cancellation error.
    pub async fn apply(
        &self,
        cancellation: &CancellationToken,
        stage: &StagedUpdate,
    ) -> Result<ApplyResult, UpdateError> {
        let host = Target::host();
        if stage.os != host.os || stage.arch != host.arch {
            return Err(UpdateError::InstallRefused);
        }
        validate_stage(cancellation, stage)?;
        platform::apply(self, cancellation, stage).await
    }

    #[cfg(test)]
    pub(crate) fn with_parts(
        executable: Arc<dyn Fn() -> io::Result<PathBuf> + Send + Sync>,
        runner: Arc<dyn CommandRunner>,
    ) -> Self {
        Self {
            current_executable: executable,
            runner,
        }
    }

    #[cfg(test)]
    pub(crate) fn current_executable_for_test(&self) -> io::Result<PathBuf> {
        (self.current_executable)()
    }
}

/// Runs one fixed trust or handoff command with a null stdin, bounded
/// output, and a hard deadline.
///
/// On Unix the command gets its own process group, and a timeout or
/// cancellation kills the whole group before the leader is reaped, so no
/// helper it started can outlive the deadline.
pub(crate) async fn run_bounded_command(
    cancellation: &CancellationToken,
    program: &Path,
    arguments: &[String],
    timeout: Duration,
) -> Result<Vec<u8>, UpdateError> {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|_| UpdateError::InstallRefused)?;
    let stdout = child.stdout.take().ok_or(UpdateError::InstallRefused)?;
    let stderr = child.stderr.take().ok_or(UpdateError::InstallRefused)?;
    let mut stdout_task = tokio::spawn(read_command_pipe(stdout));
    let mut stderr_task = tokio::spawn(read_command_pipe(stderr));
    let deadline = tokio::time::Instant::now() + timeout;
    let status = tokio::select! {
        () = cancellation.cancelled() => {
            terminate(&mut child).await;
            stdout_task.abort();
            stderr_task.abort();
            return Err(UpdateError::Cancelled);
        }
        () = tokio::time::sleep_until(deadline) => {
            terminate(&mut child).await;
            stdout_task.abort();
            stderr_task.abort();
            return Err(UpdateError::InstallRefused);
        }
        status = child.wait() => status.map_err(|_| UpdateError::InstallRefused)?,
    };
    // A descendant that inherited the pipes can keep them open after the
    // command exits; the output is not trusted to be complete in that case.
    let drained = tokio::time::timeout(PIPE_DRAIN_GRACE, async {
        let stdout = (&mut stdout_task).await;
        let stderr = (&mut stderr_task).await;
        (stdout, stderr)
    })
    .await;
    let Ok((stdout, stderr)) = drained else {
        stdout_task.abort();
        stderr_task.abort();
        return Err(UpdateError::InstallRefused);
    };
    let mut output = stdout.map_err(|_| UpdateError::InstallRefused)??;
    let stderr = stderr.map_err(|_| UpdateError::InstallRefused)??;
    let remaining = MAX_COMMAND_OUTPUT.saturating_sub(output.len());
    output.extend_from_slice(&stderr[..stderr.len().min(remaining)]);
    if !status.success() {
        return Err(UpdateError::InstallRefused);
    }
    Ok(output)
}

/// Kills a still-running command (its whole process group on Unix) and
/// reaps it. The leader is unreaped while the group is signalled, so its
/// process ID cannot have been reused.
async fn terminate(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn read_command_pipe(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
) -> Result<Vec<u8>, UpdateError> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let count = pipe
            .read(&mut buffer)
            .await
            .map_err(|_| UpdateError::InstallRefused)?;
        if count == 0 {
            break;
        }
        let remaining = MAX_COMMAND_OUTPUT.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
    }
    Ok(output)
}

/// Resolves a crash-interrupted platform replacement for one stage root.
///
/// # Errors
/// Returns validation, ownership, identity, or ambiguous-recovery failures.
pub fn recover_pending_apply(
    cancellation: &CancellationToken,
    stage_root: &Path,
) -> Result<bool, UpdateError> {
    platform::recover(cancellation, stage_root)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{
        ApplyAction, ApplyResult, CancellationToken, HANDOFF_COMMAND_TIMEOUT, Installer, Path,
        StageKind, StagedUpdate, UpdateError, VERIFY_COMMAND_TIMEOUT,
    };

    const REQUIREMENT: &str = "anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"3CAJR4ZDMQ\"";

    pub(super) async fn apply(
        installer: &Installer,
        cancellation: &CancellationToken,
        stage: &StagedUpdate,
    ) -> Result<ApplyResult, UpdateError> {
        if stage.kind != StageKind::DarwinDmg {
            return Err(UpdateError::InstallRefused);
        }
        let commands = [
            (
                "/usr/bin/hdiutil",
                vec!["verify".to_owned(), stage.asset_path.display().to_string()],
                VERIFY_COMMAND_TIMEOUT,
            ),
            (
                "/usr/bin/codesign",
                vec![
                    "--verify".to_owned(),
                    "--strict".to_owned(),
                    "--verbose=2".to_owned(),
                    format!("-R={REQUIREMENT}"),
                    stage.asset_path.display().to_string(),
                ],
                VERIFY_COMMAND_TIMEOUT,
            ),
            (
                "/usr/sbin/spctl",
                vec![
                    "--assess".to_owned(),
                    "--type".to_owned(),
                    "open".to_owned(),
                    "--context".to_owned(),
                    "context:primary-signature".to_owned(),
                    stage.asset_path.display().to_string(),
                ],
                VERIFY_COMMAND_TIMEOUT,
            ),
            (
                "/usr/bin/open",
                vec![stage.asset_path.display().to_string()],
                HANDOFF_COMMAND_TIMEOUT,
            ),
        ];
        for (program, arguments, timeout) in commands {
            installer
                .runner
                .run(cancellation, Path::new(program), &arguments, timeout)
                .await?;
        }
        Ok(ApplyResult {
            version: stage.version.clone(),
            action: ApplyAction::OpenedNativeInstaller,
            restart_required: false,
            manual_install: true,
            cleanup_pending: false,
        })
    }

    #[allow(clippy::unnecessary_wraps)]
    pub(super) fn recover(
        _cancellation: &CancellationToken,
        _stage_root: &Path,
    ) -> Result<bool, UpdateError> {
        Ok(false)
    }
}

#[cfg(windows)]
mod platform {
    #![allow(unsafe_code)]

    use std::os::windows::ffi::OsStringExt;

    use windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW;

    use super::{
        ApplyAction, ApplyResult, CancellationToken, HANDOFF_COMMAND_TIMEOUT, Installer, Path,
        PathBuf, StageKind, StagedUpdate, UpdateError,
    };

    pub(super) async fn apply(
        installer: &Installer,
        cancellation: &CancellationToken,
        stage: &StagedUpdate,
    ) -> Result<ApplyResult, UpdateError> {
        if stage.kind != StageKind::WindowsZip {
            return Err(UpdateError::InstallRefused);
        }
        let mut buffer = [0_u16; 32_768];
        let length = unsafe { GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
        if length == 0 || usize::try_from(length).unwrap_or(usize::MAX) >= buffer.len() {
            return Err(UpdateError::InstallRefused);
        }
        let directory = std::ffi::OsString::from_wide(&buffer[..length as usize]);
        let explorer = PathBuf::from(directory).join("explorer.exe");
        let arguments = vec![format!("/select,{}", stage.asset_path.display())];
        installer
            .runner
            .run(cancellation, &explorer, &arguments, HANDOFF_COMMAND_TIMEOUT)
            .await?;
        Ok(ApplyResult {
            version: stage.version.clone(),
            action: ApplyAction::RevealedVerifiedArchive,
            restart_required: false,
            manual_install: true,
            cleanup_pending: false,
        })
    }

    pub(super) fn recover(
        _cancellation: &CancellationToken,
        _stage_root: &Path,
    ) -> Result<bool, UpdateError> {
        Ok(false)
    }
}

/// Linux in-place binary replacement.
///
/// Built on Linux, and in every Unix test build so the journal, rollback and
/// recovery paths run on the macOS development host too.
#[cfg(any(target_os = "linux", all(test, unix)))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) mod linux {
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    use rustix::fs::{FlockOperation, Mode, OFlags};
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};

    use crate::permissions::{open_private_regular, secure_private_path, validate_private_path};
    use crate::staging::load_stage;

    use super::{
        ApplyAction, ApplyResult, CancellationToken, Installer, Path, PathBuf, SMOKE_TEST_TIMEOUT,
        StageKind, StagedUpdate, Target, UpdateError,
    };

    /// Runs the replaced binary's `version` command. Tests substitute a fake.
    pub(crate) type SmokeTest<'a> =
        &'a dyn Fn(&CancellationToken, &Path) -> Result<Vec<u8>, UpdateError>;

    #[derive(Clone)]
    struct LinuxTarget {
        path: PathBuf,
        mode: u32,
        dev: u64,
        ino: u64,
    }

    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Journal {
        version: String,
        stage_root: PathBuf,
        target: PathBuf,
        backup: PathBuf,
        original_dev: u64,
        original_ino: u64,
        payload_sha256: String,
        payload_size_bytes: u64,
    }

    pub(crate) async fn apply(
        installer: &Installer,
        cancellation: &CancellationToken,
        stage: &StagedUpdate,
    ) -> Result<ApplyResult, UpdateError> {
        if stage.kind != StageKind::LinuxBinary {
            return Err(UpdateError::InstallRefused);
        }
        let executable =
            (installer.current_executable)().map_err(|_| UpdateError::InstallRefused)?;
        let target = canonical_target(&executable)?;
        let directory = target.path.parent().ok_or(UpdateError::InstallRefused)?;
        let _lock = ApplyLock::acquire(stage, &target.path)?;
        let journal_path = journal_path(stage, &target.path);
        match fs::symlink_metadata(&journal_path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) | Err(_) => return Err(UpdateError::InstallRefused),
        }
        let (candidate_path, mut candidate) = create_sibling(directory, ".ptrack-update-")?;
        let result = async {
            copy_verified(cancellation, stage, &mut candidate)?;
            candidate
                .set_permissions(fs::Permissions::from_mode(target.mode))
                .map_err(|_| UpdateError::InstallRefused)?;
            candidate
                .sync_all()
                .map_err(|_| UpdateError::InstallRefused)?;
            drop(candidate);
            target.unchanged()?;
            let backup = reserve_sibling(directory, ".ptrack-backup-")?;
            fs::hard_link(&target.path, &backup).map_err(|_| UpdateError::InstallRefused)?;
            if let Err(error) = target.verify_backup(&backup) {
                let _ = fs::remove_file(&backup);
                return Err(error);
            }
            let journal = Journal {
                version: stage.version.clone(),
                stage_root: stage.root.clone(),
                target: target.path.clone(),
                backup: backup.clone(),
                original_dev: target.dev,
                original_ino: target.ino,
                payload_sha256: stage.payload_sha256.clone(),
                payload_size_bytes: stage.payload_size_bytes,
            };
            if let Err(error) = write_journal(&journal_path, &journal) {
                let _ = fs::remove_file(&backup);
                return Err(error);
            }
            if fs::rename(&candidate_path, &target.path).is_err() {
                let _ = fs::remove_file(&backup);
                let _ = remove_journal(&journal_path);
                return Err(UpdateError::InstallRefused);
            }
            if sync_directory(directory).is_err() {
                if rollback(&target.path, &backup, directory).is_err() {
                    return Err(UpdateError::InstallRefused);
                }
                let _ = remove_journal(&journal_path);
                return Err(UpdateError::InstallRefused);
            }
            let output = installer
                .runner
                .run(
                    cancellation,
                    &target.path,
                    &["version".to_owned()],
                    SMOKE_TEST_TIMEOUT,
                )
                .await;
            // A canceled smoke test proves nothing about the new binary, so
            // it is rolled back like a failed one but reported as canceled.
            if let Err(error) = smoke_test_passed(output, &stage.version) {
                rollback(&target.path, &backup, directory)?;
                let _ = remove_journal(&journal_path);
                return Err(error);
            }
            let cleanup_pending = fs::remove_file(&backup).is_err()
                || sync_directory(directory).is_err()
                || remove_journal(&journal_path).is_err();
            Ok(ApplyResult {
                version: stage.version.clone(),
                action: ApplyAction::InstalledRestartRequired,
                restart_required: true,
                manual_install: false,
                cleanup_pending,
            })
        }
        .await;
        if result.is_err() {
            let _ = fs::remove_file(&candidate_path);
        }
        result
    }

    pub(crate) fn recover(
        cancellation: &CancellationToken,
        stage_root: &Path,
    ) -> Result<bool, UpdateError> {
        let executable = std::env::current_exe().map_err(|_| UpdateError::InstallRefused)?;
        recover_with(
            cancellation,
            stage_root,
            &executable,
            &|cancellation, program| {
                run_blocking_command(cancellation, program, &["version"], SMOKE_TEST_TIMEOUT)
            },
        )
    }

    /// Resolves a pending journal for `executable`.
    ///
    /// The journal is written before the rename and removed only after the
    /// smoke test passed and the backup was deleted, so a surviving journal
    /// with the new payload installed and the backup still present means the
    /// process stopped before (or during) the smoke test. Recovery repeats the
    /// smoke test and rolls back to the backup when it fails, instead of
    /// keeping an unproven binary.
    pub(crate) fn recover_with(
        cancellation: &CancellationToken,
        stage_root: &Path,
        executable: &Path,
        smoke_test: SmokeTest<'_>,
    ) -> Result<bool, UpdateError> {
        let stage = load_stage(cancellation, stage_root)?;
        if stage.kind != StageKind::LinuxBinary
            || stage.os != "linux"
            || stage.arch != Target::host().arch
        {
            return Err(UpdateError::InstallRefused);
        }
        let target = canonical_target(executable)?;
        let _lock = ApplyLock::acquire(&stage, &target.path)?;
        let path = journal_path(&stage, &target.path);
        let data = match read_private_json(&path, 4096) {
            Ok(data) => data,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            _ => return Err(UpdateError::InstallRefused),
        };
        let journal: Journal =
            serde_json::from_slice(&data).map_err(|_| UpdateError::InstallRefused)?;
        if journal.stage_root != stage.root {
            return Err(UpdateError::PendingStageMismatch);
        }
        if journal.version != stage.version
            || journal.target != target.path
            || journal.payload_sha256 != stage.payload_sha256
            || journal.payload_size_bytes != stage.payload_size_bytes
            || !valid_journal_paths(&journal)
        {
            return Err(UpdateError::InstallRefused);
        }
        let metadata =
            fs::symlink_metadata(&journal.target).map_err(|_| UpdateError::InstallRefused)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(UpdateError::InstallRefused);
        }
        let original =
            metadata.dev() == journal.original_dev && metadata.ino() == journal.original_ino;
        if !original {
            let (digest, size) =
                hash_target(cancellation, &journal.target, stage.payload_size_bytes)?;
            if digest != stage.payload_sha256 || size != stage.payload_size_bytes {
                return Err(UpdateError::InstallRefused);
            }
            // Without a backup the apply had already passed its smoke test
            // and was cleaning up; there is nothing left to roll back to.
            if verified_backup_present(&journal)? {
                let output = smoke_test(cancellation, &journal.target);
                match smoke_test_passed(output, &stage.version) {
                    Ok(()) => {}
                    Err(UpdateError::Cancelled) => return Err(UpdateError::Cancelled),
                    Err(_) => {
                        let directory =
                            journal.target.parent().ok_or(UpdateError::InstallRefused)?;
                        rollback(&journal.target, &journal.backup, directory)?;
                        remove_journal(&path)?;
                        return Ok(true);
                    }
                }
            }
        }
        remove_verified_backup(&journal)?;
        remove_journal(&path)?;
        Ok(true)
    }

    /// Accepts only `ptrack <version>` from a successful run; a cancellation
    /// stays a cancellation.
    fn smoke_test_passed(
        output: Result<Vec<u8>, UpdateError>,
        version: &str,
    ) -> Result<(), UpdateError> {
        match output {
            Ok(bytes) if String::from_utf8_lossy(&bytes).trim() == format!("ptrack {version}") => {
                Ok(())
            }
            Err(UpdateError::Cancelled) => Err(UpdateError::Cancelled),
            Ok(_) | Err(_) => Err(UpdateError::InstallRefused),
        }
    }

    /// Runs a command synchronously for startup recovery, with a null stdin,
    /// bounded stdout, a hard deadline, and cancellation.
    fn run_blocking_command(
        cancellation: &CancellationToken,
        program: &Path,
        arguments: &[&str],
        timeout: Duration,
    ) -> Result<Vec<u8>, UpdateError> {
        const POLL: Duration = Duration::from_millis(10);
        let mut child = std::process::Command::new(program)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| UpdateError::InstallRefused)?;
        let mut stdout = child.stdout.take().ok_or(UpdateError::InstallRefused)?;
        let reader = std::thread::Builder::new()
            .name("ptrack-update-smoke".to_owned())
            .spawn(move || {
                let mut output = Vec::new();
                let _ = Read::by_ref(&mut stdout)
                    .take(u64::try_from(super::MAX_COMMAND_OUTPUT).unwrap_or(u64::MAX))
                    .read_to_end(&mut output);
                output
            })
            .map_err(|_| {
                let _ = child.kill();
                let _ = child.wait();
                UpdateError::InstallRefused
            })?;
        let deadline = Instant::now() + timeout;
        let status = loop {
            if cancellation.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(UpdateError::Cancelled);
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(UpdateError::InstallRefused);
                }
            }
        };
        let drain_deadline = Instant::now() + super::PIPE_DRAIN_GRACE;
        while !reader.is_finished() && Instant::now() < drain_deadline {
            std::thread::sleep(POLL);
        }
        if !reader.is_finished() || !status.success() {
            return Err(UpdateError::InstallRefused);
        }
        reader.join().map_err(|_| UpdateError::InstallRefused)
    }

    pub(crate) struct ApplyLock(File);

    impl ApplyLock {
        pub(crate) fn acquire(stage: &StagedUpdate, target: &Path) -> Result<Self, UpdateError> {
            let base = stage.root.parent().ok_or(UpdateError::InstallRefused)?;
            validate_private_path(base, true).map_err(|_| UpdateError::InstallRefused)?;
            let path = base.join(format!(".apply-lock-{}", target_key(target)));
            let descriptor = rustix::fs::open(
                &path,
                OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|_| UpdateError::InstallRefused)?;
            rustix::fs::fchmod(&descriptor, Mode::RUSR | Mode::WUSR)
                .map_err(|_| UpdateError::InstallRefused)?;
            let file = File::from(descriptor);
            rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive)
                .map_err(|_| UpdateError::InstallRefused)?;
            Ok(Self(file))
        }
    }

    impl Drop for ApplyLock {
        fn drop(&mut self) {
            let _ = rustix::fs::flock(&self.0, FlockOperation::Unlock);
        }
    }

    fn canonical_target(path: &Path) -> Result<LinuxTarget, UpdateError> {
        let canonical = fs::canonicalize(path).map_err(|_| UpdateError::InstallRefused)?;
        if !canonical.is_absolute() {
            return Err(UpdateError::InstallRefused);
        }
        let descriptor = rustix::fs::open(
            &canonical,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|_| UpdateError::InstallRefused)?;
        let file = File::from(descriptor);
        let metadata = file.metadata().map_err(|_| UpdateError::InstallRefused)?;
        let parent = fs::metadata(canonical.parent().ok_or(UpdateError::InstallRefused)?)
            .map_err(|_| UpdateError::InstallRefused)?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o6000 != 0
            || metadata.mode() & 0o111 == 0
            || metadata.mode() & 0o022 != 0
            || parent.uid() != rustix::process::geteuid().as_raw()
            || parent.mode() & 0o022 != 0
        {
            return Err(UpdateError::InstallRefused);
        }
        Ok(LinuxTarget {
            path: canonical,
            mode: metadata.mode() & 0o7777,
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }

    impl LinuxTarget {
        fn unchanged(&self) -> Result<(), UpdateError> {
            let metadata =
                fs::symlink_metadata(&self.path).map_err(|_| UpdateError::InstallRefused)?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.dev() != self.dev
                || metadata.ino() != self.ino
            {
                return Err(UpdateError::InstallRefused);
            }
            Ok(())
        }

        fn verify_backup(&self, path: &Path) -> Result<(), UpdateError> {
            let metadata = fs::symlink_metadata(path).map_err(|_| UpdateError::InstallRefused)?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.dev() != self.dev
                || metadata.ino() != self.ino
            {
                return Err(UpdateError::InstallRefused);
            }
            Ok(())
        }
    }

    fn create_sibling(directory: &Path, prefix: &str) -> Result<(PathBuf, File), UpdateError> {
        for _ in 0..32 {
            let path = directory.join(format!("{prefix}{}", random_hex()?));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(file) => return Ok((path, file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(UpdateError::InstallRefused),
            }
        }
        Err(UpdateError::InstallRefused)
    }

    fn reserve_sibling(directory: &Path, prefix: &str) -> Result<PathBuf, UpdateError> {
        let (path, file) = create_sibling(directory, prefix)?;
        drop(file);
        fs::remove_file(&path).map_err(|_| UpdateError::InstallRefused)?;
        Ok(path)
    }

    fn copy_verified(
        cancellation: &CancellationToken,
        stage: &StagedUpdate,
        output: &mut File,
    ) -> Result<(), UpdateError> {
        let mut input =
            open_private_regular(&stage.payload_path).map_err(|_| UpdateError::InstallRefused)?;
        let mut hash = Sha256::new();
        let mut size = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
        loop {
            if cancellation.is_cancelled() {
                return Err(UpdateError::Cancelled);
            }
            let count = input
                .read(&mut buffer)
                .map_err(|_| UpdateError::InstallRefused)?;
            if count == 0 {
                break;
            }
            size += count as u64;
            if size > stage.payload_size_bytes {
                return Err(UpdateError::InstallRefused);
            }
            hash.update(&buffer[..count]);
            output
                .write_all(&buffer[..count])
                .map_err(|_| UpdateError::InstallRefused)?;
        }
        if size != stage.payload_size_bytes || hex_lower(&hash.finalize()) != stage.payload_sha256 {
            return Err(UpdateError::InstallRefused);
        }
        Ok(())
    }

    fn write_journal(path: &Path, journal: &Journal) -> Result<(), UpdateError> {
        let mut data = serde_json::to_vec(journal).map_err(|_| UpdateError::InstallRefused)?;
        data.push(b'\n');
        let parent = path.parent().ok_or(UpdateError::InstallRefused)?;
        let (temporary, mut file) = create_sibling(parent, ".pending-apply-")?;
        file.write_all(&data)
            .map_err(|_| UpdateError::InstallRefused)?;
        file.sync_all().map_err(|_| UpdateError::InstallRefused)?;
        drop(file);
        secure_private_path(&temporary, false).map_err(|_| UpdateError::InstallRefused)?;
        fs::rename(&temporary, path).map_err(|_| UpdateError::InstallRefused)?;
        sync_directory(parent).map_err(|_| UpdateError::InstallRefused)
    }

    fn remove_journal(path: &Path) -> Result<(), UpdateError> {
        match fs::remove_file(path) {
            Ok(()) => sync_directory(path.parent().ok_or(UpdateError::InstallRefused)?)
                .map_err(|_| UpdateError::InstallRefused),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(UpdateError::InstallRefused),
        }
    }

    pub(crate) fn journal_path(stage: &StagedUpdate, target: &Path) -> PathBuf {
        stage
            .root
            .parent()
            .unwrap_or(Path::new(""))
            .join(format!(".pending-apply-{}.json", target_key(target)))
    }

    fn target_key(path: &Path) -> String {
        let digest = Sha256::digest(path.as_os_str().as_encoded_bytes());
        hex_lower(&digest[..16])
    }

    fn valid_journal_paths(journal: &Journal) -> bool {
        journal.stage_root.is_absolute()
            && journal.target.is_absolute()
            && journal.backup.is_absolute()
            && journal.target != journal.backup
            && journal.target.parent() == journal.backup.parent()
            && journal
                .backup
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| {
                    name.starts_with(".ptrack-backup-") && name != ".ptrack-backup-"
                })
    }

    fn rollback(target: &Path, backup: &Path, directory: &Path) -> Result<(), UpdateError> {
        fs::rename(backup, target).map_err(|_| UpdateError::InstallRefused)?;
        sync_directory(directory).map_err(|_| UpdateError::InstallRefused)
    }

    /// Reports whether the journal's backup still exists, refusing a backup
    /// that is not the original binary.
    fn verified_backup_present(journal: &Journal) -> Result<bool, UpdateError> {
        let metadata = match fs::symlink_metadata(&journal.backup) {
            Ok(value) => value,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(UpdateError::InstallRefused),
        };
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.dev() != journal.original_dev
            || metadata.ino() != journal.original_ino
        {
            return Err(UpdateError::InstallRefused);
        }
        Ok(true)
    }

    fn remove_verified_backup(journal: &Journal) -> Result<(), UpdateError> {
        if !verified_backup_present(journal)? {
            return Ok(());
        }
        fs::remove_file(&journal.backup).map_err(|_| UpdateError::InstallRefused)?;
        sync_directory(journal.backup.parent().ok_or(UpdateError::InstallRefused)?)
            .map_err(|_| UpdateError::InstallRefused)
    }

    fn hash_target(
        cancellation: &CancellationToken,
        path: &Path,
        limit: u64,
    ) -> Result<(String, u64), UpdateError> {
        let descriptor = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|_| UpdateError::InstallRefused)?;
        let mut file = File::from(descriptor);
        let mut hash = Sha256::new();
        let mut size = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
        loop {
            if cancellation.is_cancelled() {
                return Err(UpdateError::Cancelled);
            }
            let count = file
                .read(&mut buffer)
                .map_err(|_| UpdateError::InstallRefused)?;
            if count == 0 {
                break;
            }
            size += count as u64;
            if size > limit {
                return Err(UpdateError::InstallRefused);
            }
            hash.update(&buffer[..count]);
        }
        Ok((hex_lower(&hash.finalize()), size))
    }

    fn read_private_json(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
        let mut file = open_private_regular(path)?;
        let mut data = Vec::new();
        Read::by_ref(&mut file)
            .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
            .read_to_end(&mut data)?;
        if data.len() > limit {
            return Err(io::Error::other(
                "private update record exceeds its byte limit",
            ));
        }
        Ok(data)
    }

    fn sync_directory(path: &Path) -> io::Result<()> {
        File::open(path)?.sync_all()
    }

    fn random_hex() -> Result<String, UpdateError> {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| UpdateError::InstallRefused)?;
        Ok(hex_lower(&random))
    }

    fn hex_lower(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        output
    }
}

#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod platform {
    use super::{ApplyResult, CancellationToken, Installer, Path, StagedUpdate, UpdateError};

    pub(super) async fn apply(
        _installer: &Installer,
        _cancellation: &CancellationToken,
        _stage: &StagedUpdate,
    ) -> Result<ApplyResult, UpdateError> {
        Err(UpdateError::UnsupportedTarget)
    }

    pub(super) fn recover(
        _cancellation: &CancellationToken,
        _stage_root: &Path,
    ) -> Result<bool, UpdateError> {
        Ok(false)
    }
}
