use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::manifest::{Artifact, DatabaseEntry, DatabaseKind, SourceIdentity};
use super::sha256::{Sha256, hex};
use super::stage::{validate_bucket_presence, validate_stage};
use super::wire::{Key, Quarantine};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

#[test]
fn validates_an_empty_global_stage_and_exact_hashes() {
    let (root, manifest_path) = empty_global_stage();
    let report = validate_stage(&manifest_path).expect("valid stage");
    assert_eq!(report.database_count, 1);
    assert_eq!(report.record_count, 0);
    assert_eq!(report.quarantine_count, 0);

    fs::remove_dir_all(root).expect("remove temporary stage");
}

#[test]
fn sha256_matches_standard_vectors() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn quarantine_preserves_an_empty_invalid_gob() {
    let quarantine = Quarantine {
        _line_type: "quarantine".to_owned(),
        bucket: "capabilities".to_owned(),
        key: Key {
            encoding: "u64".to_owned(),
            value: "1".to_owned(),
        },
        source_value_sha256: sha256_hex(b""),
        reason: "invalid_capability".to_owned(),
        legacy_codec: "go-gob".to_owned(),
        legacy_value_hex: String::new(),
    };
    let (_, retained) = quarantine.convert().expect("empty invalid gob quarantine");
    assert!(retained.legacy_gob.is_empty());
}

#[test]
fn old_format_allows_later_known_buckets_to_be_present_or_absent() {
    let database = project_database("1");
    validate_bucket_presence(&database, "commits", false).expect("later bucket may be absent");
    validate_bucket_presence(&database, "commits", true).expect("later bucket may be present");
    assert!(validate_bucket_presence(&database, "tasks", false).is_err());
}

fn project_database(source_format: &str) -> DatabaseEntry {
    DatabaseEntry {
        id: "project".to_owned(),
        kind: DatabaseKind::Project,
        project_root: Some("/tmp/project".to_owned()),
        source_path: "/tmp/project/.ptrack/ptrack.db".to_owned(),
        source_format: source_format.to_owned(),
        source_identity: SourceIdentity {
            device: "1".to_owned(),
            inode: "2".to_owned(),
            size: "3".to_owned(),
            mtime_seconds: "4".to_owned(),
            mtime_nanos: "5".to_owned(),
            sha256: "0".repeat(64),
        },
        data: Artifact {
            path: "databases/0001-project.jsonl".to_owned(),
            sha256: "0".repeat(64),
            bytes: "1".to_owned(),
            record_count: "0".to_owned(),
            bucket_count: "10".to_owned(),
        },
    }
}

pub(super) fn empty_global_stage() -> (PathBuf, PathBuf) {
    let root = temporary_directory();
    let databases = root.join("databases");
    fs::create_dir(&databases).expect("databases directory");
    private_directory(&root);
    private_directory(&databases);

    let jsonl = concat!(
        "{\"type\":\"database\",\"schema\":\"1\",\"database_id\":\"global\",\"kind\":\"global\",",
        "\"source_format\":\"0\",\"bucket_count\":\"3\",\"record_count\":\"0\",",
        "\"quarantine_count\":\"0\"}\n",
        "{\"type\":\"bucket\",\"name\":\"config\",\"present\":true,\"sequence\":null,\"record_count\":\"0\"}\n",
        "{\"type\":\"bucket\",\"name\":\"projects\",\"present\":true,\"sequence\":null,\"record_count\":\"0\"}\n",
        "{\"type\":\"bucket\",\"name\":\"backups\",\"present\":true,\"sequence\":null,\"record_count\":\"0\"}\n"
    );
    let data_path = databases.join("0000-global.jsonl");
    fs::write(&data_path, jsonl).expect("JSONL");
    private_file(&data_path);
    let data_hash = sha256_hex(jsonl.as_bytes());
    let source_path = root.join("source-global.db");
    let manifest = format!(
        concat!(
            "{{\"format\":\"ptrack-db-stage\",\"version\":\"1\",\"database_count\":\"1\",",
            "\"quarantine_count\":\"0\",\"registry\":[],\"databases\":[{{\"id\":\"global\",\"kind\":\"global\",",
            "\"project_root\":null,\"source_path\":{},\"source_format\":\"0\",",
            "\"source_identity\":{{\"device\":\"1\",\"inode\":\"2\",\"size\":\"3\",",
            "\"mtime_seconds\":\"4\",\"mtime_nanos\":\"5\",\"sha256\":\"{}\"}},",
            "\"data\":{{\"path\":\"databases/0000-global.jsonl\",\"sha256\":\"{}\",",
            "\"bytes\":\"{}\",\"record_count\":\"0\",\"bucket_count\":\"3\"}}}}]}}\n"
        ),
        serde_json::to_string(source_path.to_str().expect("UTF-8 path")).expect("path JSON"),
        "0".repeat(64),
        data_hash,
        jsonl.len(),
    );
    let manifest_path = root.join("manifest.json");
    fs::write(&manifest_path, manifest).expect("manifest");
    private_file(&manifest_path);

    (root, manifest_path)
}

fn temporary_directory() -> PathBuf {
    let suffix = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("ptrack-db-import-{}-{suffix}", std::process::id()));
    fs::create_dir(&path).expect("temporary stage");
    path
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    hex(digest.finish())
}

#[cfg(unix)]
fn private_directory(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("private directory");
}

#[cfg(not(unix))]
fn private_directory(_: &Path) {}

#[cfg(unix)]
fn private_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("private file");
}

#[cfg(not(unix))]
fn private_file(_: &Path) {}
