use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use ptrack_db_import::{import_stage, validate_stage};
use ptrack_store::{
    ActiveBinding, Collection, GlobalStore, ProjectStore, RecordKey, StagedStore, Store, StoreKind,
    protect_private_directory,
};

#[test]
fn go_json_batch_imports_and_reopens_every_candidate() {
    let root = temporary_root();
    let stage = root.join("stage");
    let candidates = root.join("candidates");
    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/ptrack-db-export")
        .canonicalize()
        .expect("legacy exporter helper");
    let output = Command::new("go")
        .args(["test", "-run", "^TestCrossLanguageJSONFixture$", "-count=1"])
        .env("PTRACK_JSON_STAGE_OUTPUT", &stage)
        .current_dir(helper)
        .output()
        .expect("run Go JSON exporter fixture");
    assert!(
        output.status.success(),
        "Go fixture failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let manifest = stage.join("manifest.json");
    let validation = validate_stage(&manifest).expect("validate Go JSON stage");
    assert_eq!(validation.database_count, 2);
    assert_eq!(validation.quarantine_count, 1);
    let receipt = import_stage(&manifest, &candidates, true).expect("import complete batch");
    assert_eq!(receipt.candidate_count, 2);
    assert_eq!(receipt.report, validation);

    let expected = [
        ("0000-global.redb", StoreKind::Global),
        ("0001-project-000001.redb", StoreKind::Project),
    ];
    for (name, kind) in expected {
        let path = candidates.join(name);
        let store = Store::open_existing(&path, kind).expect("reopen candidate");
        let provenance = store
            .json_stage_provenance()
            .expect("read provenance")
            .expect("JSON-stage provenance");
        assert_eq!(provenance.batch_manifest_sha256, validation.manifest_sha256);
        if kind == StoreKind::Project {
            store
                .read(|read| {
                    assert_eq!(read.sequence_high_water(Collection::Plans)?, 1);
                    assert_eq!(read.sequence_high_water(Collection::Capabilities)?, 2);
                    assert_eq!(read.scan(Collection::Capabilities)?.len(), 1);
                    Ok(())
                })
                .expect("verify imported sequences and revoked capability row");
        } else {
            store
                .read(|read| {
                    assert_eq!(
                        read.get(Collection::GlobalConfig, RecordKey::Bytes(&[0xff, b'k']))?
                            .expect("binary config")
                            .payload(),
                        &[0x00, 0xff, b'v']
                    );
                    Ok(())
                })
                .expect("verify binary global config");
        }
        drop(store);

        let binding = ActiveBinding {
            generation: 11,
            database_id: name.trim_end_matches(".redb").to_owned(),
            kind,
            canonical_path: path.canonicalize().expect("canonical candidate"),
        };
        let staged = StagedStore::open(&path, kind).expect("open staged candidate");
        if kind == StoreKind::Project {
            let project = ProjectStore::activate(staged, binding, "fixture")
                .expect("activate project candidate");
            assert!(!project.application_writes().expect("write marker"));
            let snapshot = project.snapshot().expect("typed project snapshot");
            assert_eq!(snapshot.meta.goal, "ship safely");
            assert_eq!(snapshot.plans[0].title, "parity");
            assert_eq!(snapshot.tasks[0].title, "convert");
            let capability = project.capability(1).expect("typed capability");
            assert!(!capability.enabled);
            assert!(capability.approved_at.is_zero());
            assert!(capability.expires_at.is_zero());
        } else {
            let global = GlobalStore::activate(staged, binding).expect("activate global candidate");
            assert_eq!(
                global.config(&[0xff, b'k']).expect("typed config"),
                [0x00, 0xff, b'v']
            );
            assert_eq!(global.projects().expect("typed registry").len(), 1);
        }
    }
    assert!(candidates.join("receipt.json").is_file());
    assert!(!candidates.join("incomplete.json").exists());

    fs::remove_dir_all(root).expect("remove test-only migration directory");
}

fn temporary_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "ptrack-json-cross-language-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&root).expect("create test root");
    protect_private_directory(&root).expect("make test root private");
    root
}
