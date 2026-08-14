use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::sync::Mutex;

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
    drop(commands);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct RecordingRunner(Mutex<Vec<(String, Vec<String>)>>);

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
    ) -> CommandFuture<'a> {
        self.0
            .lock()
            .unwrap()
            .push((program.display().to_string(), arguments.to_vec()));
        Box::pin(async { Ok(Vec::new()) })
    }
}

#[cfg(target_os = "macos")]
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
