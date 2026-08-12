#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use ptrack_db_import::{import_stage, validate_stage};
use ptrack_store::{Store, StoreKind};

#[test]
fn go_json_batch_imports_and_reopens_every_candidate() {
    let root = temporary_root();
    let stage = root.join("stage");
    let candidates = root.join("candidates");
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root");
    let output = Command::new("go")
        .args([
            "test",
            "./internal/store",
            "-run",
            "^TestCrossLanguageJSONFixture$",
            "-count=1",
        ])
        .env("PTRACK_JSON_STAGE_OUTPUT", &stage)
        .current_dir(repository)
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
        let store = Store::open_existing(candidates.join(name), kind).expect("reopen candidate");
        let provenance = store
            .json_stage_provenance()
            .expect("read provenance")
            .expect("JSON-stage provenance");
        assert_eq!(provenance.batch_manifest_sha256, validation.manifest_sha256);
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("make test root private");
    }
    root
}
