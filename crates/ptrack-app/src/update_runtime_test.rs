use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier, Mutex};

use ptrack_store::{ActiveBinding, GlobalStore, StoreKind};
use ptrack_updater::{
    ApplyAction, ApplyResult, Asset, Candidate, Progress, StageKind, StagedUpdate, Target,
    UpdateError,
};
use tokio_util::sync::CancellationToken;

use super::update_runtime::{
    BackendFuture, DesktopUpdateService, GlobalStoreUpdatePreferences, UpdateBackend,
    UpdateEventSink, UpdatePhase, UpdatePreferences, UpdateRuntime, UpdateState,
};

#[test]
fn check_download_apply_publish_exact_secret_free_monotonic_state() {
    let root = temporary_root();
    let events = Arc::new(EventLog::default());
    let backend = Arc::new(FakeBackend::normal(&root));
    let runtime = UpdateRuntime::with_backend(
        "1.2.3".to_owned(),
        target(),
        root.clone(),
        Arc::new(MemoryPreferences::default()),
        Some(events.clone()),
        backend,
    );
    runtime.start().unwrap();
    let available = runtime.check_for_updates().unwrap();
    assert_eq!(available.phase, UpdatePhase::Available);
    let wire = serde_json::to_string(&available).unwrap();
    assert!(wire.contains("\"currentVersion\":\"1.2.3\""));
    assert!(wire.contains("\"version\":\"1.2.4\""));
    assert!(!wire.contains("browser_download_url"));
    assert!(!wire.contains(".stage-"));

    let ready = runtime.download_update("1.2.4").unwrap();
    assert_eq!(ready.phase, UpdatePhase::Ready);
    assert!(ready.checksum_verified);
    assert_eq!(ready.downloaded_bytes, 1_024);
    let installed = runtime.apply_update("1.2.4").unwrap();
    assert_eq!(installed.phase, UpdatePhase::Installed);
    assert_eq!(installed.apply_action, "installed-restart-required");
    assert!(installed.restart_required);
    assert!(events.revisions_are_strictly_increasing());
    cleanup(&root);
}

#[test]
fn cancel_retains_single_flight_until_worker_exits_and_shutdown_is_owned() {
    let root = temporary_root();
    let started = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let backend = Arc::new(FakeBackend::canceling(
        &root,
        started.clone(),
        release.clone(),
    ));
    let runtime = UpdateRuntime::with_backend(
        "1.2.3".to_owned(),
        target(),
        root.clone(),
        Arc::new(MemoryPreferences::default()),
        None,
        backend,
    );
    runtime.start().unwrap();
    runtime.check_for_updates().unwrap();
    let worker = {
        let runtime = runtime.clone();
        std::thread::spawn(move || runtime.download_update("1.2.4"))
    };
    started.wait();
    assert_eq!(runtime.cancel_operation().phase, UpdatePhase::Canceling);
    assert_eq!(
        runtime.check_for_updates().unwrap_err(),
        "another update operation is active"
    );
    release.wait();
    assert_eq!(
        worker.join().unwrap().unwrap_err(),
        "update operation was canceled"
    );
    runtime.shutdown().unwrap();
    cleanup(&root);
}

#[test]
fn startup_recovery_blocks_more_than_sixty_four_stage_directories_before_authority() {
    let root = temporary_root();
    for index in 0..65 {
        std::fs::create_dir(root.join(format!(".stage-{index:02}"))).unwrap();
    }
    let runtime = UpdateRuntime::with_backend(
        "1.2.3".to_owned(),
        target(),
        root.clone(),
        Arc::new(MemoryPreferences::default()),
        None,
        Arc::new(FakeBackend::normal(&root)),
    );
    runtime.start().unwrap();
    let state = runtime.state();
    assert_eq!(state.phase, UpdatePhase::RecoveryRequired);
    assert_eq!(
        state.error,
        "Too many saved updates require manual cleanup."
    );
    assert_eq!(
        runtime.check_for_updates().unwrap_err(),
        "update recovery requires attention"
    );
    cleanup(&root);
}

#[test]
fn startup_recovery_selects_newest_valid_stage_and_discards_other_verified_stages() {
    let root = temporary_root();
    for name in [".stage-current", ".stage-newest", ".stage-older"] {
        std::fs::create_dir(root.join(name)).unwrap();
    }
    let backend = Arc::new(FakeBackend::recovering(&root));
    let runtime = UpdateRuntime::with_backend(
        "1.2.3".to_owned(),
        target(),
        root.clone(),
        Arc::new(MemoryPreferences::default()),
        None,
        backend.clone(),
    );
    runtime.start().unwrap();
    let state = runtime.state();
    assert_eq!(state.phase, UpdatePhase::Ready);
    assert_eq!(state.release.unwrap().version, "1.3.0");
    let discarded = backend.discarded.lock().unwrap();
    assert!(
        discarded
            .iter()
            .any(|path| path.ends_with(".stage-current"))
    );
    assert!(discarded.iter().any(|path| path.ends_with(".stage-older")));
    assert!(!discarded.iter().any(|path| path.ends_with(".stage-newest")));
    drop(discarded);
    cleanup(&root);
}

#[test]
fn unresolved_linux_apply_journal_blocks_startup_authority() {
    let root = temporary_root();
    std::fs::write(root.join(".pending-apply-target.json"), b"ambiguous").unwrap();
    let runtime = UpdateRuntime::with_backend(
        "1.2.3".to_owned(),
        target(),
        root.clone(),
        Arc::new(MemoryPreferences::default()),
        None,
        Arc::new(FakeBackend::normal(&root)),
    );
    runtime.start().unwrap();
    assert_eq!(runtime.state().phase, UpdatePhase::RecoveryRequired);
    cleanup(&root);
}

#[test]
fn preference_is_literal_opt_in_and_mutation_emits_full_state() {
    let root = temporary_root();
    let preferences = Arc::new(MemoryPreferences::default());
    let events = Arc::new(EventLog::default());
    let runtime = UpdateRuntime::with_backend(
        "1.2.3".to_owned(),
        target(),
        root.clone(),
        preferences.clone(),
        Some(events.clone()),
        Arc::new(FakeBackend::normal(&root)),
    );
    runtime.start().unwrap();
    assert!(runtime.set_automatic_checks(true).unwrap().automatic_checks);
    assert!(preferences.load_automatic_checks().unwrap());
    assert_eq!(events.last().unwrap().current_version, "1.2.3");
    cleanup(&root);
}

#[test]
fn production_preference_adapter_persists_only_literal_true_or_false_in_global_store() {
    let root = temporary_root();
    let database = std::fs::canonicalize(&root).unwrap().join("global.redb");
    let binding = ActiveBinding {
        generation: 1,
        database_id: "update-preferences".to_owned(),
        kind: StoreKind::Global,
        canonical_path: database.clone(),
    };
    drop(GlobalStore::create_new(&database, binding.clone()).unwrap());
    let preferences = GlobalStoreUpdatePreferences::new(database.clone(), binding.clone());
    preferences.save_automatic_checks(true).unwrap();
    assert!(preferences.load_automatic_checks().unwrap());
    assert_eq!(
        GlobalStore::open_existing(&database, &binding)
            .unwrap()
            .config(b"updates.auto-check")
            .unwrap(),
        b"true"
    );
    preferences.save_automatic_checks(false).unwrap();
    assert!(!preferences.load_automatic_checks().unwrap());
    cleanup(&root);
}

#[derive(Default)]
struct MemoryPreferences(Mutex<bool>);

impl UpdatePreferences for MemoryPreferences {
    fn load_automatic_checks(&self) -> Result<bool, String> {
        Ok(*self.0.lock().unwrap())
    }

    fn save_automatic_checks(&self, enabled: bool) -> Result<(), String> {
        *self.0.lock().unwrap() = enabled;
        Ok(())
    }
}

#[derive(Default)]
struct EventLog(Mutex<Vec<UpdateState>>);

impl UpdateEventSink for EventLog {
    fn state_changed(&self, state: UpdateState) {
        self.0.lock().unwrap().push(state);
    }
}

impl EventLog {
    fn last(&self) -> Option<UpdateState> {
        self.0.lock().unwrap().last().cloned()
    }

    fn revisions_are_strictly_increasing(&self) -> bool {
        self.0
            .lock()
            .unwrap()
            .windows(2)
            .all(|pair| pair[0].revision < pair[1].revision)
    }
}

struct FakeBackend {
    candidate: Candidate,
    stage: StagedUpdate,
    started: Option<Arc<Barrier>>,
    release: Option<Arc<Barrier>>,
    recovery_versions: BTreeMap<String, String>,
    discarded: Arc<Mutex<Vec<PathBuf>>>,
}

impl FakeBackend {
    fn normal(root: &Path) -> Self {
        Self {
            candidate: candidate(),
            stage: stage(root),
            started: None,
            release: None,
            recovery_versions: BTreeMap::new(),
            discarded: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn canceling(root: &Path, started: Arc<Barrier>, release: Arc<Barrier>) -> Self {
        Self {
            candidate: candidate(),
            stage: stage(root),
            started: Some(started),
            release: Some(release),
            recovery_versions: BTreeMap::new(),
            discarded: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn recovering(root: &Path) -> Self {
        let mut value = Self::normal(root);
        value.recovery_versions = BTreeMap::from([
            (".stage-current".to_owned(), "1.2.3".to_owned()),
            (".stage-newest".to_owned(), "1.3.0".to_owned()),
            (".stage-older".to_owned(), "1.2.4".to_owned()),
        ]);
        value
    }
}

impl UpdateBackend for FakeBackend {
    fn check<'a>(
        &'a self,
        _cancellation: &'a CancellationToken,
        _current_version: &'a str,
        _target: &'a Target,
    ) -> BackendFuture<'a, Candidate> {
        let candidate = self.candidate.clone();
        Box::pin(async move { Ok(candidate) })
    }

    fn stage<'a>(
        &'a self,
        cancellation: &'a CancellationToken,
        _candidate: &'a Candidate,
        _target: &'a Target,
        _root: &'a Path,
        progress: Arc<dyn Fn(Progress) + Send + Sync>,
    ) -> BackendFuture<'a, StagedUpdate> {
        let stage = self.stage.clone();
        let started = self.started.clone();
        let release = self.release.clone();
        Box::pin(async move {
            if let Some(started) = started {
                started.wait();
                cancellation.cancelled().await;
                release.unwrap().wait();
                return Err(UpdateError::Cancelled);
            }
            progress(Progress {
                asset: "checksums".to_owned(),
                downloaded: 80,
                total: 80,
            });
            progress(Progress {
                asset: "package".to_owned(),
                downloaded: 128,
                total: 1_024,
            });
            progress(Progress {
                asset: "package".to_owned(),
                downloaded: 1_024,
                total: 1_024,
            });
            Ok(stage)
        })
    }

    fn apply<'a>(
        &'a self,
        _cancellation: &'a CancellationToken,
        _stage: &'a StagedUpdate,
    ) -> BackendFuture<'a, ApplyResult> {
        Box::pin(async {
            Ok(ApplyResult {
                version: "1.2.4".to_owned(),
                action: ApplyAction::InstalledRestartRequired,
                restart_required: true,
                manual_install: false,
                cleanup_pending: false,
            })
        })
    }

    fn load(
        &self,
        _cancellation: &CancellationToken,
        root: &Path,
    ) -> Result<StagedUpdate, UpdateError> {
        if self.recovery_versions.is_empty() {
            return Ok(self.stage.clone());
        }
        let name = root
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(UpdateError::InvalidStage)?;
        let version = self
            .recovery_versions
            .get(name)
            .ok_or(UpdateError::InvalidStage)?;
        let mut staged = self.stage.clone();
        staged.root = root.to_path_buf();
        staged.version.clone_from(version);
        Ok(staged)
    }

    fn recover(
        &self,
        _cancellation: &CancellationToken,
        _root: &Path,
    ) -> Result<bool, UpdateError> {
        Ok(false)
    }

    fn discard(&self, root: &Path) -> Result<(), UpdateError> {
        self.discarded.lock().unwrap().push(root.to_path_buf());
        Ok(())
    }
}

fn candidate() -> Candidate {
    Candidate {
        version: "1.2.4".to_owned(),
        tag: "v1.2.4".to_owned(),
        page_url: "https://github.com/ro-ag/ptrack/releases/tag/v1.2.4".to_owned(),
        published_at: "2026-08-11T00:00:00Z".to_owned(),
        notes: "Release notes".to_owned(),
        package: Asset {
            name: "ptrack_1.2.4_linux_amd64.tar.gz".to_owned(),
            download_url: "https://github.com/ro-ag/ptrack/releases/download/v1.2.4/ptrack_1.2.4_linux_amd64.tar.gz".to_owned(),
            size_bytes: 1_024,
        },
        checksums: Asset {
            name: "checksums.txt".to_owned(),
            download_url: "https://github.com/ro-ag/ptrack/releases/download/v1.2.4/checksums.txt".to_owned(),
            size_bytes: 80,
        },
    }
}

fn stage(root: &Path) -> StagedUpdate {
    StagedUpdate {
        root: root.join(".stage-fake"),
        asset_path: root.join(".stage-fake/package"),
        payload_path: root.join(".stage-fake/ptrack"),
        state_path: root.join(".stage-fake/state.json"),
        version: "1.2.4".to_owned(),
        asset_name: "ptrack_1.2.4_linux_amd64.tar.gz".to_owned(),
        os: "linux".to_owned(),
        arch: "amd64".to_owned(),
        sha256: "aa".repeat(32),
        size_bytes: 1_024,
        payload_sha256: "bb".repeat(32),
        payload_size_bytes: 512,
        kind: StageKind::LinuxBinary,
    }
}

fn target() -> Target {
    Target {
        os: "linux".to_owned(),
        arch: "amd64".to_owned(),
    }
}

fn temporary_root() -> PathBuf {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).unwrap();
    let name = bytes.iter().fold(String::new(), |mut output, byte| {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
        output
    });
    let root = std::env::temp_dir().join(format!("ptrack-update-runtime-{name}"));
    std::fs::create_dir(&root).unwrap();
    ptrack_store::protect_private_directory(&root).unwrap();
    root
}

fn cleanup(root: &Path) {
    std::fs::remove_dir_all(root).unwrap();
}
