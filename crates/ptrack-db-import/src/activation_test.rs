use std::fs;
use std::path::{Path, PathBuf};

use ptrack_store::{
    ActiveGeneration, CutoverLockMode, GlobalStore, ProjectStore, StoreKind, acquire_cutover_lock,
    install_active_generation, load_active_generation, protect_private_directory,
    protect_private_file, restore_active_generation, verify_private_path,
};
use sha2::{Digest, Sha256};

use super::stage_test::{empty_global_stage, empty_stage_with_project};
use crate::{activate_stage, import_stage, rollback_activation};

#[test]
fn activation_is_resumable_receipt_last_and_rollback_is_write_fenced() {
    let fixture = fixture("rollback", false);
    let receipt = activate(&fixture);
    assert!(fixture.batch.join("plan.json").is_file());
    assert!(fixture.batch.join("journal.json").is_file());
    assert!(fixture.batch.join("handoff.json").is_file());
    assert!(fixture.batch.join("receipt.json").is_file());
    assert_eq!(receipt.state, "ACTIVE");
    assert_eq!(receipt.destinations.len(), 1);
    assert_eq!(receipt.installed_destinations.len(), 1);
    assert_eq!(receipt.legacy_sources.len(), 1);
    assert_eq!(receipt.destinations[0].record_count, "0");
    assert_eq!(receipt.destinations[0].quarantine_count, "0");
    assert!(!receipt.legacy_sources[0].device.is_empty());
    assert!(!receipt.legacy_sources[0].inode.is_empty());
    assert_eq!(receipt.destinations[0].collection_state_sha256.len(), 64);
    assert_eq!(activate(&fixture), receipt);

    fs::remove_file(fixture.batch.join("receipt.json")).unwrap();
    assert_eq!(activate(&fixture), receipt);

    let lease = acquire_cutover_lock(&fixture.home, CutoverLockMode::Exclusive).unwrap();
    restore_active_generation(
        &fixture.home,
        &lease,
        receipt.previous_marker.as_ref(),
        "test",
    )
    .unwrap();
    drop(lease);
    rollback_activation(&fixture.batch, &fixture.home, "test", true).unwrap();
    rollback_activation(&fixture.batch, &fixture.home, "test", true).unwrap();
    let lease = acquire_cutover_lock(&fixture.home, CutoverLockMode::Shared).unwrap();
    assert!(
        load_active_generation(&fixture.home, &lease)
            .unwrap()
            .is_none()
    );
}

#[test]
fn activation_resumes_after_stores_are_durable_but_before_handoff_or_marker() {
    #[derive(serde::Serialize)]
    struct Journal<'a> {
        format: &'a str,
        version: &'a str,
        sequence: &'a str,
        state: &'a str,
        predecessor: &'a str,
        plan_sha256: &'a str,
    }

    let fixture = fixture("stores-installed-resume", true);
    let receipt = activate(&fixture);

    let lease = acquire_cutover_lock(&fixture.home, CutoverLockMode::Exclusive).unwrap();
    restore_active_generation(
        &fixture.home,
        &lease,
        receipt.previous_marker.as_ref(),
        "test",
    )
    .unwrap();
    drop(lease);
    fs::remove_file(fixture.batch.join("receipt.json")).unwrap();
    fs::remove_file(fixture.batch.join("handoff.json")).unwrap();
    let journal = Journal {
        format: "ptrack-cutover-journal",
        version: "1",
        sequence: "2",
        state: "stores-installed",
        predecessor: "planned",
        plan_sha256: &receipt.plan_sha256,
    };
    let mut bytes = serde_json::to_vec(&journal).unwrap();
    bytes.push(b'\n');
    let journal_path = fixture.batch.join("journal.json");
    fs::write(&journal_path, bytes).unwrap();
    private_file(&journal_path);

    let resumed = activate(&fixture);
    assert_eq!(resumed, receipt);
    let lease = acquire_cutover_lock(&fixture.home, CutoverLockMode::Shared).unwrap();
    assert_eq!(
        load_active_generation(&fixture.home, &lease).unwrap(),
        Some(receipt.marker)
    );
}

#[test]
fn candidate_digest_is_bound_before_any_destination_is_installed() {
    use std::io::Write as _;

    let fixture = fixture("candidate-tamper", false);
    let candidate = fixture.candidates.join("0000-global.redb");
    fs::OpenOptions::new()
        .append(true)
        .open(candidate)
        .unwrap()
        .write_all(b"tamper")
        .unwrap();
    assert!(
        activate_stage(
            &fixture.manifest,
            &fixture.candidates,
            &fixture.batch,
            &fixture.home,
            11,
            "test",
            true,
        )
        .unwrap_err()
        .to_string()
        .contains("candidate")
    );
    assert!(!fixture.home.join("global.redb").exists());
}

#[test]
fn stale_staged_destination_is_never_activated_as_the_planned_candidate() {
    let stale = fixture("stale-candidate-source", false);
    let fixture = fixture("stale-candidate-target", false);
    let destination = fixture.home.join("global.redb");
    fs::copy(stale.candidates.join("0000-global.redb"), &destination).unwrap();
    private_file(&destination);

    assert!(
        activate_stage(
            &fixture.manifest,
            &fixture.candidates,
            &fixture.batch,
            &fixture.home,
            11,
            "test",
            true,
        )
        .is_err()
    );
    let lease = acquire_cutover_lock(&fixture.home, CutoverLockMode::Shared).unwrap();
    assert!(
        load_active_generation(&fixture.home, &lease)
            .unwrap()
            .is_none()
    );
}

#[test]
fn resume_rejects_a_missing_previous_marker_recorded_by_the_plan() {
    let fixture = fixture("missing-previous-marker", false);
    let global_path = fixture.home.canonicalize().unwrap().join("global.redb");
    let binding = ptrack_store::ActiveBinding {
        generation: 7,
        database_id: "previous-global".to_owned(),
        kind: StoreKind::Global,
        canonical_path: global_path.clone(),
    };
    drop(GlobalStore::create_new(&global_path, binding).unwrap());
    let previous =
        ActiveGeneration::new(7, "previous-global".to_owned(), &global_path, Vec::new()).unwrap();
    let lease = acquire_cutover_lock(&fixture.home, CutoverLockMode::Exclusive).unwrap();
    install_active_generation(&fixture.home, &lease, &previous, "test").unwrap();
    drop(lease);

    // The first attempt durably publishes a plan bound to `previous`, then
    // correctly refuses to treat that active destination as the new candidate.
    assert!(
        activate_stage(
            &fixture.manifest,
            &fixture.candidates,
            &fixture.batch,
            &fixture.home,
            11,
            "test",
            true,
        )
        .is_err()
    );
    assert!(fixture.batch.join("plan.json").is_file());

    let lease = acquire_cutover_lock(&fixture.home, CutoverLockMode::Exclusive).unwrap();
    restore_active_generation(&fixture.home, &lease, None, "test").unwrap();
    drop(lease);
    let error = activate_stage(
        &fixture.manifest,
        &fixture.candidates,
        &fixture.batch,
        &fixture.home,
        11,
        "test",
        true,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("does not match the activation plan")
    );
}

#[test]
fn rollback_requires_the_exact_locked_legacy_source_and_destination_identity() {
    let source_fixture = fixture("identity-fence", false);
    let receipt = activate(&source_fixture);
    let source = PathBuf::from(&receipt.legacy_sources[0].path);
    fs::write(&source, b"changed legacy source").unwrap();
    assert!(
        rollback_activation(&source_fixture.batch, &source_fixture.home, "test", true,)
            .unwrap_err()
            .to_string()
            .contains("legacy source")
    );

    // Restore the exact source identity through a fresh fixture, then prove a
    // byte-identical destination at another inode is still rejected.
    let fixture = fixture("destination-identity-fence", false);
    let receipt = activate(&fixture);
    let destination = PathBuf::from(&receipt.destinations[0].path);
    let moved = destination.with_extension("moved");
    fs::rename(&destination, &moved).unwrap();
    fs::copy(&moved, &destination).unwrap();
    private_file(&destination);
    assert!(
        rollback_activation(&fixture.batch, &fixture.home, "test", true)
            .unwrap_err()
            .to_string()
            .contains("destination identity")
    );
}

#[cfg(unix)]
#[test]
fn ordinary_legacy_unix_project_directory_can_activate_private_store() {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let fixture = fixture("legacy-project-mode", true);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture.manifest).unwrap()).unwrap();
    let root = manifest["databases"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|database| database["project_root"].as_str())
        .map(PathBuf::from)
        .unwrap();
    fs::set_permissions(root.join(".ptrack"), fs::Permissions::from_mode(0o755)).unwrap();
    let receipt = activate(&fixture);
    let mode = fs::metadata(&receipt.marker.projects[0].path)
        .unwrap()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn any_committed_global_write_permanently_refuses_automatic_rollback() {
    let fixture = fixture("write-fence", false);
    let receipt = activate(&fixture);
    let binding = receipt.marker.global_binding().unwrap();
    let store = GlobalStore::open_existing(&receipt.marker.global.path, &binding).unwrap();
    assert!(!store.application_writes().unwrap());
    store.set_config(b"cutover-test", b"true").unwrap();
    assert!(store.application_writes().unwrap());
    drop(store);
    assert!(
        rollback_activation(&fixture.batch, &fixture.home, "test", true)
            .unwrap_err()
            .to_string()
            .contains("forbidden after an application write")
    );
}

#[test]
fn any_committed_project_write_permanently_refuses_automatic_rollback() {
    let fixture = fixture("project-write-fence", true);
    let receipt = activate(&fixture);
    let project = receipt.marker.projects.first().unwrap();
    let binding = receipt.marker.project_binding(project).unwrap();
    let store = ProjectStore::open_existing(&project.path, &binding, "test").unwrap();
    assert!(!store.application_writes().unwrap());
    store.set_goal("application write").unwrap();
    assert!(store.application_writes().unwrap());
    drop(store);
    assert!(
        rollback_activation(&fixture.batch, &fixture.home, "test", true)
            .unwrap_err()
            .to_string()
            .contains("forbidden after an application write")
    );
}

struct Fixture {
    stage: PathBuf,
    manifest: PathBuf,
    batch: PathBuf,
    candidates: PathBuf,
    home: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.stage);
        let _ = fs::remove_dir_all(&self.home);
    }
}

fn fixture(name: &str, with_project: bool) -> Fixture {
    let (stage, manifest) = if with_project {
        let (stage, manifest, _) = empty_stage_with_project();
        (stage, manifest)
    } else {
        empty_global_stage()
    };
    bind_source_identities(&manifest);

    let home = stage.with_extension(format!("{name}-home"));
    fs::create_dir(&home).unwrap();
    private_directory(&home);
    let migrations = home.join("migrations");
    fs::create_dir(&migrations).unwrap();
    private_directory(&migrations);
    let batch = migrations.join(format!("batch-{name}"));
    fs::create_dir(&batch).unwrap();
    private_directory(&batch);
    let candidates = batch.join("candidates");
    import_stage(&manifest, &candidates, true).unwrap();
    Fixture {
        stage,
        manifest,
        batch,
        candidates,
        home,
    }
}

fn bind_source_identities(manifest: &Path) {
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(manifest).unwrap()).unwrap();
    for database in value["databases"].as_array_mut().unwrap() {
        let source = PathBuf::from(database["source_path"].as_str().unwrap());
        let id = database["id"].as_str().unwrap().to_owned();
        fs::write(&source, format!("legacy source remains immutable: {id}")).unwrap();
        private_file(&source);
        let metadata = source.metadata().unwrap();
        let identity = verify_private_path(&source, false).unwrap();
        let modified = metadata
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        database["source_identity"] = serde_json::json!({
            "device": identity.device.to_string(),
            "inode": identity.inode.to_string(),
            "size": metadata.len().to_string(),
            "mtime_seconds": modified.as_secs().to_string(),
            "mtime_nanos": modified.subsec_nanos().to_string(),
            "sha256": format!("{:x}", Sha256::digest(fs::read(&source).unwrap()))
        });
    }
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    fs::write(manifest, bytes).unwrap();
    private_file(manifest);
}

fn activate(fixture: &Fixture) -> crate::ActivationReceipt {
    activate_stage(
        &fixture.manifest,
        &fixture.candidates,
        &fixture.batch,
        &fixture.home,
        11,
        "test",
        true,
    )
    .unwrap()
}

fn private_directory(path: &Path) {
    protect_private_directory(path).unwrap();
}

fn private_file(path: &Path) {
    protect_private_file(path).unwrap();
}
