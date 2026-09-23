use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::sync::Mutex;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::installer::{CommandFuture, CommandRunner, Installer};

#[cfg(target_os = "macos")]
use super::staging::{StageKind, StagedUpdate, hash_regular_file, write_stage_record};

struct NoopRunner;

impl CommandRunner for NoopRunner {
    fn run<'a>(
        &'a self,
        _cancellation: &'a CancellationToken,
        _program: &'a Path,
        _arguments: &'a [String],
        _timeout: Duration,
    ) -> CommandFuture<'a> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

#[test]
fn installer_owns_explicit_executable_and_command_dependencies() {
    let expected = PathBuf::from("/fixed/ptrack");
    let captured = expected.clone();
    let installer =
        Installer::with_parts(Arc::new(move || Ok(captured.clone())), Arc::new(NoopRunner));
    assert_eq!(installer.current_executable_for_test().unwrap(), expected);
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "current_thread")]
async fn macos_handoff_runs_exact_pinned_trust_chain_before_open() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = temporary_root();
    let host = super::Target::host();
    let asset_name = format!("p-track_1.2.4_darwin_{}.dmg", host.arch);
    let asset_path = root.join(&asset_name);
    std::fs::write(&asset_path, b"synthetic dmg").unwrap();
    std::fs::set_permissions(&asset_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let cancellation = CancellationToken::new();
    let (digest, size) = hash_regular_file(&cancellation, &asset_path, 512 << 20).unwrap();
    let stage = StagedUpdate {
        root: root.clone(),
        asset_path: asset_path.clone(),
        payload_path: asset_path,
        state_path: root.join("state.json"),
        version: "1.2.4".to_owned(),
        asset_name,
        os: "darwin".to_owned(),
        arch: host.arch,
        sha256: digest.clone(),
        size_bytes: size,
        payload_sha256: digest,
        payload_size_bytes: size,
        kind: StageKind::DarwinDmg,
    };
    write_stage_record(&stage).unwrap();
    let runner = Arc::new(RecordingRunner::default());
    let installer = Installer::with_parts(
        Arc::new(|| {
            Ok(PathBuf::from(
                "/Applications/P-TRACK.app/Contents/MacOS/ptrack",
            ))
        }),
        runner.clone(),
    );
    let result = installer.apply(&cancellation, &stage).await.unwrap();
    assert!(result.manual_install);
    assert_eq!(
        runner.programs(),
        [
            "/usr/bin/hdiutil",
            "/usr/bin/codesign",
            "/usr/sbin/spctl",
            "/usr/bin/open"
        ]
    );
    let commands = runner.0.lock().unwrap();
    assert!(
        commands[1]
            .1
            .iter()
            .any(|argument| argument.contains("3CAJR4ZDMQ"))
    );
    assert_eq!(commands[3].1, [stage.asset_path.display().to_string()]);
    assert!(
        commands
            .iter()
            .all(|command| !command.2.is_zero() && command.2 <= Duration::from_secs(120)),
        "every trust and handoff command needs a hard deadline"
    );
    drop(commands);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct RecordingRunner(Mutex<Vec<(String, Vec<String>, Duration)>>);

#[cfg(target_os = "macos")]
impl RecordingRunner {
    fn programs(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|command| command.0.clone())
            .collect()
    }
}

#[cfg(target_os = "macos")]
impl CommandRunner for RecordingRunner {
    fn run<'a>(
        &'a self,
        _cancellation: &'a CancellationToken,
        program: &'a Path,
        arguments: &'a [String],
        timeout: Duration,
    ) -> CommandFuture<'a> {
        self.0
            .lock()
            .unwrap()
            .push((program.display().to_string(), arguments.to_vec(), timeout));
        Box::pin(async { Ok(Vec::new()) })
    }
}

#[cfg(unix)]
fn temporary_root() -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;

    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).unwrap();
    let suffix = random.iter().fold(String::new(), |mut output, byte| {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
        output
    });
    let root = std::env::temp_dir().join(format!("ptrack-installer-test-{suffix}"));
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    root
}

#[cfg(unix)]
mod bounded_command {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use tokio_util::sync::CancellationToken;

    use super::super::UpdateError;
    use super::super::installer::run_bounded_command;

    fn shell(script: &str) -> Vec<String> {
        vec!["-c".to_owned(), script.to_owned()]
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_hung_command_is_killed_at_its_deadline() {
        let started = Instant::now();
        let result = run_bounded_command(
            &CancellationToken::new(),
            Path::new("/bin/sh"),
            &shell("sleep 30"),
            Duration::from_millis(200),
        )
        .await;
        assert_eq!(result, Err(UpdateError::InstallRefused));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn commands_read_a_null_stdin() {
        let output = run_bounded_command(
            &CancellationToken::new(),
            Path::new("/bin/sh"),
            &shell("if read line; then echo input; else echo eof; fi"),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&output).trim(), "eof");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_stops_a_running_command() {
        let cancellation = CancellationToken::new();
        let trigger = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            trigger.cancel();
        });
        let started = Instant::now();
        let result = run_bounded_command(
            &cancellation,
            Path::new("/bin/sh"),
            &shell("sleep 30"),
            Duration::from_secs(30),
        )
        .await;
        assert_eq!(result, Err(UpdateError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_timeout_kills_helpers_the_command_started() {
        let root = super::temporary_root();
        let marker = root.join("helper-survived");
        let result = run_bounded_command(
            &CancellationToken::new(),
            Path::new("/bin/sh"),
            &shell(&format!(
                "(sleep 1; touch '{}') & sleep 30",
                marker.display()
            )),
            Duration::from_millis(300),
        )
        .await;
        assert_eq!(result, Err(UpdateError::InstallRefused));
        tokio::time::sleep(Duration::from_millis(1_300)).await;
        assert!(!marker.exists(), "a helper outlived the command's deadline");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(unix)]
mod linux_installer {
    use std::fs;
    use std::io;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use sha2::{Digest, Sha256};
    use tokio_util::sync::CancellationToken;

    use super::super::installer::linux::{self, ApplyLock};
    use super::super::installer::{CommandFuture, CommandRunner, Installer};
    use super::super::staging::{StageKind, StagedUpdate, hash_regular_file, write_stage_record};
    use super::super::{ApplyAction, Target, UpdateError};

    const OLD_SCRIPT: &[u8] = b"#!/bin/sh\necho 'ptrack 1.2.3'\n";
    const NEW_SCRIPT: &[u8] = b"#!/bin/sh\necho 'ptrack 1.2.4'\n";

    /// A private updates root, a stage inside it, and an installed `ptrack`
    /// in a separate owner-only directory.
    struct Fixture {
        base: PathBuf,
        install: PathBuf,
        target: PathBuf,
        stage: StagedUpdate,
    }

    impl Fixture {
        /// Stage whose payload is a runnable script, for apply tests.
        fn with_script_payload(payload: &[u8]) -> Self {
            Self::layout(payload)
        }

        /// Stage that passes `load_stage` (ELF payload, durable record), for
        /// recovery tests.
        fn with_loadable_stage() -> Self {
            let host = Target::host();
            let mut fixture = Self::layout(&fake_elf(&host.arch));
            write_private(&fixture.stage.asset_path, b"verified archive");
            let cancellation = CancellationToken::new();
            let stage = &mut fixture.stage;
            (stage.sha256, stage.size_bytes) =
                hash_regular_file(&cancellation, &stage.asset_path, 512 << 20).unwrap();
            write_stage_record(stage).unwrap();
            fixture
        }

        fn layout(payload: &[u8]) -> Self {
            let base = fs::canonicalize(super::temporary_root()).unwrap();
            let install = base.join("bin");
            fs::create_dir(&install).unwrap();
            fs::set_permissions(&install, fs::Permissions::from_mode(0o755)).unwrap();
            let target = install.join("ptrack");
            fs::write(&target, OLD_SCRIPT).unwrap();
            fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
            let root = base.join(".stage-0123456789abcdef");
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            let host = Target::host();
            let asset_name = format!("ptrack_1.2.4_linux_{}.tar.gz", host.arch);
            let payload_path = root.join("ptrack");
            write_private(&payload_path, payload);
            let stage = StagedUpdate {
                asset_path: root.join(&asset_name),
                payload_path,
                state_path: root.join("state.json"),
                version: "1.2.4".to_owned(),
                asset_name,
                os: "linux".to_owned(),
                arch: host.arch,
                sha256: "00".repeat(32),
                size_bytes: 1,
                payload_sha256: hex(&Sha256::digest(payload)),
                payload_size_bytes: payload.len() as u64,
                kind: StageKind::LinuxBinary,
                root,
            };
            Self {
                base,
                install,
                target,
                stage,
            }
        }

        fn installer(&self, runner: Arc<dyn CommandRunner>) -> Installer {
            let target = self.target.clone();
            Installer::with_parts(Arc::new(move || Ok(target.clone())), runner)
        }

        fn journal(&self) -> PathBuf {
            linux::journal_path(&self.stage, &self.target)
        }

        /// Files other than `ptrack` left in the install directory.
        fn install_leftovers(&self) -> Vec<String> {
            fs::read_dir(&self.install)
                .unwrap()
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name != "ptrack")
                .collect()
        }

        /// Reproduces the on-disk state `apply` leaves at each crash point:
        /// a backup hard link and a journal, optionally with the new payload
        /// already renamed over the target.
        fn simulate_crash(&self, renamed: bool, backup_kept: bool) -> PathBuf {
            let original = fs::metadata(&self.target).unwrap();
            let backup = self
                .install
                .join(".ptrack-backup-00112233445566778899aabbccddeeff");
            fs::hard_link(&self.target, &backup).unwrap();
            let journal = serde_json::json!({
                "version": self.stage.version,
                "stage_root": self.stage.root,
                "target": self.target,
                "backup": backup,
                "original_dev": original.dev(),
                "original_ino": original.ino(),
                "payload_sha256": self.stage.payload_sha256,
                "payload_size_bytes": self.stage.payload_size_bytes,
            });
            write_private(&self.journal(), format!("{journal}\n").as_bytes());
            if renamed {
                let candidate = self
                    .install
                    .join(".ptrack-update-00112233445566778899aabbccddeeff");
                fs::copy(&self.stage.payload_path, &candidate).unwrap();
                fs::set_permissions(&candidate, fs::Permissions::from_mode(0o755)).unwrap();
                fs::rename(&candidate, &self.target).unwrap();
            }
            if !backup_kept {
                fs::remove_file(&backup).unwrap();
            }
            backup
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    /// Runs the installed script for real. Spawning a file this process just
    /// wrote can fail with ETXTBSY while a concurrently forked test child
    /// still holds the write descriptor, so the spawn is retried.
    struct ScriptRunner;

    impl CommandRunner for ScriptRunner {
        fn run<'a>(
            &'a self,
            _cancellation: &'a CancellationToken,
            program: &'a Path,
            arguments: &'a [String],
            _timeout: Duration,
        ) -> CommandFuture<'a> {
            Box::pin(async move { spawn_with_retry(program, arguments) })
        }
    }

    fn spawn_with_retry(program: &Path, arguments: &[String]) -> Result<Vec<u8>, UpdateError> {
        for _ in 0..50 {
            match std::process::Command::new(program)
                .args(arguments)
                .stdin(std::process::Stdio::null())
                .output()
            {
                Ok(output) if output.status.success() => return Ok(output.stdout),
                Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(_) | Err(_) => return Err(UpdateError::InstallRefused),
            }
        }
        Err(UpdateError::InstallRefused)
    }

    struct FixedRunner(Result<Vec<u8>, UpdateError>);

    impl CommandRunner for FixedRunner {
        fn run<'a>(
            &'a self,
            _cancellation: &'a CancellationToken,
            _program: &'a Path,
            _arguments: &'a [String],
            _timeout: Duration,
        ) -> CommandFuture<'a> {
            let result = self.0.clone();
            Box::pin(async move { result })
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn apply_replaces_the_binary_after_a_passing_smoke_test() {
        let fixture = Fixture::with_script_payload(NEW_SCRIPT);
        let result = linux::apply(
            &fixture.installer(Arc::new(ScriptRunner)),
            &CancellationToken::new(),
            &fixture.stage,
        )
        .await
        .unwrap();
        assert_eq!(result.action, ApplyAction::InstalledRestartRequired);
        assert!(result.restart_required && !result.cleanup_pending);
        assert_eq!(fs::read(&fixture.target).unwrap(), NEW_SCRIPT);
        assert_eq!(
            fs::metadata(&fixture.target).unwrap().permissions().mode() & 0o7777,
            0o755,
            "the installed binary keeps the original mode"
        );
        assert!(fixture.install_leftovers().is_empty());
        assert!(!fixture.journal().exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn apply_rolls_back_when_the_new_binary_reports_another_version() {
        let fixture = Fixture::with_script_payload(b"#!/bin/sh\necho 'ptrack 9.9.9'\n");
        let original = fs::metadata(&fixture.target).unwrap().ino();
        let error = linux::apply(
            &fixture.installer(Arc::new(ScriptRunner)),
            &CancellationToken::new(),
            &fixture.stage,
        )
        .await
        .unwrap_err();
        assert_eq!(error, UpdateError::InstallRefused);
        assert_eq!(fs::read(&fixture.target).unwrap(), OLD_SCRIPT);
        assert_eq!(fs::metadata(&fixture.target).unwrap().ino(), original);
        assert!(fixture.install_leftovers().is_empty());
        assert!(!fixture.journal().exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn apply_reports_a_canceled_smoke_test_as_canceled_and_rolls_back() {
        let fixture = Fixture::with_script_payload(NEW_SCRIPT);
        let error = linux::apply(
            &fixture.installer(Arc::new(FixedRunner(Err(UpdateError::Cancelled)))),
            &CancellationToken::new(),
            &fixture.stage,
        )
        .await
        .unwrap_err();
        assert_eq!(error, UpdateError::Cancelled);
        assert_eq!(fs::read(&fixture.target).unwrap(), OLD_SCRIPT);
        assert!(fixture.install_leftovers().is_empty());
        assert!(!fixture.journal().exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn apply_refuses_while_a_journal_is_pending() {
        let fixture = Fixture::with_script_payload(NEW_SCRIPT);
        write_private(&fixture.journal(), b"{}\n");
        let error = linux::apply(
            &fixture.installer(Arc::new(ScriptRunner)),
            &CancellationToken::new(),
            &fixture.stage,
        )
        .await
        .unwrap_err();
        assert_eq!(error, UpdateError::InstallRefused);
        assert_eq!(fs::read(&fixture.target).unwrap(), OLD_SCRIPT);
        assert!(fixture.install_leftovers().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn apply_refuses_while_another_apply_holds_the_lock() {
        let fixture = Fixture::with_script_payload(NEW_SCRIPT);
        let held = ApplyLock::acquire(&fixture.stage, &fixture.target).unwrap();
        let error = linux::apply(
            &fixture.installer(Arc::new(ScriptRunner)),
            &CancellationToken::new(),
            &fixture.stage,
        )
        .await
        .unwrap_err();
        assert_eq!(error, UpdateError::InstallRefused);
        assert_eq!(fs::read(&fixture.target).unwrap(), OLD_SCRIPT);
        drop(held);
        linux::apply(
            &fixture.installer(Arc::new(ScriptRunner)),
            &CancellationToken::new(),
            &fixture.stage,
        )
        .await
        .expect("the lock is released with its holder");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn apply_refuses_unsafe_current_executables() {
        for (target_mode, directory_mode) in [
            (0o644, 0o755),
            (0o775, 0o755),
            (0o757, 0o755),
            (0o4755, 0o755),
            (0o755, 0o775),
            (0o755, 0o777),
        ] {
            let fixture = Fixture::with_script_payload(NEW_SCRIPT);
            fs::set_permissions(&fixture.target, fs::Permissions::from_mode(target_mode)).unwrap();
            fs::set_permissions(&fixture.install, fs::Permissions::from_mode(directory_mode))
                .unwrap();
            let error = linux::apply(
                &fixture.installer(Arc::new(ScriptRunner)),
                &CancellationToken::new(),
                &fixture.stage,
            )
            .await
            .unwrap_err();
            assert_eq!(
                error,
                UpdateError::InstallRefused,
                "accepted target {target_mode:o} in directory {directory_mode:o}"
            );
            assert_eq!(fs::read(&fixture.target).unwrap(), OLD_SCRIPT);
            fs::set_permissions(&fixture.install, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(fixture.install_leftovers().is_empty());
        }

        let fixture = Fixture::with_script_payload(NEW_SCRIPT);
        let unresolvable = Installer::with_parts(
            Arc::new(|| Err(io::Error::other("no current executable"))),
            Arc::new(ScriptRunner),
        );
        assert_eq!(
            linux::apply(&unresolvable, &CancellationToken::new(), &fixture.stage)
                .await
                .unwrap_err(),
            UpdateError::InstallRefused
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn apply_follows_a_symlinked_executable_to_the_real_binary() {
        let fixture = Fixture::with_script_payload(NEW_SCRIPT);
        let link = fixture.base.join("ptrack-link");
        std::os::unix::fs::symlink(&fixture.target, &link).unwrap();
        let installer =
            Installer::with_parts(Arc::new(move || Ok(link.clone())), Arc::new(ScriptRunner));
        linux::apply(&installer, &CancellationToken::new(), &fixture.stage)
            .await
            .unwrap();
        assert_eq!(fs::read(&fixture.target).unwrap(), NEW_SCRIPT);
        assert!(
            fs::symlink_metadata(fixture.base.join("ptrack-link"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    /// Records smoke-test calls and answers with a fixed result.
    struct Smoke {
        calls: AtomicUsize,
        result: Mutex<Result<Vec<u8>, UpdateError>>,
    }

    impl Smoke {
        fn answering(result: Result<Vec<u8>, UpdateError>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                result: Mutex::new(result),
            }
        }

        fn run(
            &self,
            _cancellation: &CancellationToken,
            _program: &Path,
        ) -> Result<Vec<u8>, UpdateError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.lock().unwrap().clone()
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    fn recover(fixture: &Fixture, smoke: &Smoke) -> Result<bool, UpdateError> {
        linux::recover_with(
            &CancellationToken::new(),
            &fixture.stage.root,
            &fixture.target,
            &|cancellation, program| smoke.run(cancellation, program),
        )
    }

    #[test]
    fn recover_without_a_journal_reports_nothing_to_do() {
        let fixture = Fixture::with_loadable_stage();
        let smoke = Smoke::answering(Ok(b"ptrack 1.2.4\n".to_vec()));
        assert_eq!(recover(&fixture, &smoke), Ok(false));
        assert_eq!(smoke.calls(), 0);
    }

    #[test]
    fn recover_before_the_rename_keeps_the_original_and_clears_the_journal() {
        let fixture = Fixture::with_loadable_stage();
        let backup = fixture.simulate_crash(false, true);
        let smoke = Smoke::answering(Ok(b"ptrack 1.2.4\n".to_vec()));
        assert_eq!(recover(&fixture, &smoke), Ok(true));
        assert_eq!(fs::read(&fixture.target).unwrap(), OLD_SCRIPT);
        assert!(!backup.exists());
        assert!(!fixture.journal().exists());
        assert_eq!(smoke.calls(), 0);
    }

    #[test]
    fn recover_after_the_rename_repeats_the_smoke_test_before_keeping_the_update() {
        let fixture = Fixture::with_loadable_stage();
        let backup = fixture.simulate_crash(true, true);
        let smoke = Smoke::answering(Ok(b"ptrack 1.2.4\n".to_vec()));
        assert_eq!(recover(&fixture, &smoke), Ok(true));
        assert_eq!(smoke.calls(), 1);
        assert_eq!(
            fs::read(&fixture.target).unwrap(),
            fs::read(&fixture.stage.payload_path).unwrap()
        );
        assert!(!backup.exists());
        assert!(!fixture.journal().exists());
    }

    #[test]
    fn recover_after_the_rename_rolls_back_when_the_smoke_test_fails() {
        let fixture = Fixture::with_loadable_stage();
        let backup = fixture.simulate_crash(true, true);
        let smoke = Smoke::answering(Ok(b"ptrack 1.2.3\n".to_vec()));
        assert_eq!(recover(&fixture, &smoke), Ok(true));
        assert_eq!(smoke.calls(), 1);
        assert_eq!(fs::read(&fixture.target).unwrap(), OLD_SCRIPT);
        assert!(
            !backup.exists(),
            "the backup was renamed back over the target"
        );
        assert!(!fixture.journal().exists());
        assert!(fixture.install_leftovers().is_empty());
    }

    #[test]
    fn recover_rolls_back_when_the_new_binary_cannot_run() {
        let fixture = Fixture::with_loadable_stage();
        fixture.simulate_crash(true, true);
        let smoke = Smoke::answering(Err(UpdateError::InstallRefused));
        assert_eq!(recover(&fixture, &smoke), Ok(true));
        assert_eq!(fs::read(&fixture.target).unwrap(), OLD_SCRIPT);
        assert!(!fixture.journal().exists());
    }

    #[test]
    fn recover_leaves_the_journal_when_the_smoke_test_is_canceled() {
        let fixture = Fixture::with_loadable_stage();
        let backup = fixture.simulate_crash(true, true);
        let smoke = Smoke::answering(Err(UpdateError::Cancelled));
        assert_eq!(recover(&fixture, &smoke), Err(UpdateError::Cancelled));
        assert!(backup.exists());
        assert!(fixture.journal().exists());
    }

    #[test]
    fn recover_after_cleanup_began_keeps_the_proven_update() {
        let fixture = Fixture::with_loadable_stage();
        fixture.simulate_crash(true, false);
        let smoke = Smoke::answering(Err(UpdateError::InstallRefused));
        assert_eq!(recover(&fixture, &smoke), Ok(true));
        assert_eq!(smoke.calls(), 0);
        assert_eq!(
            fs::read(&fixture.target).unwrap(),
            fs::read(&fixture.stage.payload_path).unwrap()
        );
        assert!(!fixture.journal().exists());
    }

    #[test]
    fn recover_refuses_a_target_that_is_neither_original_nor_the_payload() {
        let fixture = Fixture::with_loadable_stage();
        fixture.simulate_crash(true, true);
        let replacement = fixture
            .install
            .join(".ptrack-update-ffffffffffffffffffffffffffffffff");
        fs::write(&replacement, b"#!/bin/sh\necho 'someone else'\n").unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&replacement, &fixture.target).unwrap();
        let smoke = Smoke::answering(Ok(b"ptrack 1.2.4\n".to_vec()));
        assert_eq!(recover(&fixture, &smoke), Err(UpdateError::InstallRefused));
        assert!(fixture.journal().exists());
    }

    #[test]
    fn recover_reports_a_journal_owned_by_another_stage() {
        let fixture = Fixture::with_loadable_stage();
        fixture.simulate_crash(false, true);
        let journal = fs::read_to_string(fixture.journal()).unwrap();
        let other = journal.replace(".stage-0123456789abcdef", ".stage-fedcba9876543210");
        write_private(&fixture.journal(), other.as_bytes());
        let smoke = Smoke::answering(Ok(b"ptrack 1.2.4\n".to_vec()));
        assert_eq!(
            recover(&fixture, &smoke),
            Err(UpdateError::PendingStageMismatch)
        );
    }

    #[test]
    fn recover_refuses_a_malformed_journal() {
        let fixture = Fixture::with_loadable_stage();
        write_private(
            &fixture.journal(),
            b"{\"version\":\"1.2.4\",\"unknown\":true}\n",
        );
        let smoke = Smoke::answering(Ok(b"ptrack 1.2.4\n".to_vec()));
        assert_eq!(recover(&fixture, &smoke), Err(UpdateError::InstallRefused));
    }

    fn write_private(path: &Path, bytes: &[u8]) {
        let _ = fs::remove_file(path);
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn fake_elf(arch: &str) -> Vec<u8> {
        let machine: u16 = if arch == "arm64" { 183 } else { 62 };
        let mut bytes = vec![0_u8; 64];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[16..18].copy_from_slice(&2_u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&machine.to_le_bytes());
        bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
        bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
        bytes
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().fold(String::new(), |mut output, byte| {
            use std::fmt::Write as _;
            let _ = write!(output, "{byte:02x}");
            output
        })
    }
}
