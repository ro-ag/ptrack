use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ptrack_store::{Collection, OwnedRecordKey, RecordKey, Store, StoreKind};

use super::adapter::{bundle_into_import_data, import_path};
use super::bundle::validate_path;
use super::sha256::Sha256;

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);
type TestBucket<'a> = (&'a str, u64, Vec<(&'a [u8], &'a [u8])>);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let number = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ptrack-migrate-adapter-{}-{number}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn sparse_historical_project_materializes_every_current_collection() {
    let directory = TestDirectory::new();
    let bundle_path = directory.path("project-v1.bundle");
    fs::write(
        &bundle_path,
        encode(
            1,
            1,
            &[
                ("meta", 0, vec![(b"meta", b"gob meta")]),
                ("notes", 4, vec![]),
                ("plans", 7, vec![(b"\0\0\0\0\0\0\0\x03", b"gob plan")]),
                ("tasks", 0, vec![]),
            ],
        ),
    )
    .unwrap();

    let data = bundle_into_import_data(validate_path(&bundle_path).unwrap()).unwrap();
    assert_eq!(data.kind, StoreKind::Project);
    assert_eq!(data.collections.len(), 10);
    for imported in &data.collections {
        assert_eq!(
            imported.sequence,
            imported
                .collection
                .is_sequenced()
                .then_some(match imported.collection {
                    Collection::Plans => 7,
                    Collection::Notes => 4,
                    _ => 0,
                })
        );
    }
    let plans = collection(&data, Collection::Plans);
    assert_eq!(plans.records[0].key, OwnedRecordKey::Id(3));
    assert_eq!(plans.records[0].envelope.payload(), b"gob plan");
    assert_eq!(plans.records[0].envelope.payload_schema(), 1);
    assert_eq!(
        plans.records[0].envelope.codec(),
        Collection::Plans.legacy_codec()
    );

    let destination = directory.path("project-v1.redb");
    let (store, report) = import_path(&bundle_path, &destination).unwrap();
    assert_eq!(report.record_count, 2);
    drop(store);
    let reopened = Store::open_existing(&destination, StoreKind::Project).unwrap();
    reopened
        .read(|transaction| {
            assert_eq!(
                transaction
                    .get(Collection::Plans, RecordKey::Id(3))?
                    .unwrap()
                    .payload(),
                b"gob plan"
            );
            assert_eq!(
                transaction.sequence_high_water(Collection::MemoryWritebacks)?,
                0
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn current_project_import_preserves_typed_keys_payloads_and_sequences() {
    let directory = TestDirectory::new();
    let bundle_path = directory.path("project-v5.bundle");
    fs::write(
        &bundle_path,
        encode(
            1,
            5,
            &[
                ("capabilities", 12, vec![]),
                ("capability_audits", 13, vec![]),
                ("commits", 11, vec![]),
                ("issues", 10, vec![]),
                (
                    "memory_writebacks",
                    17,
                    vec![(b"agent/run/receipt", b"gob receipt")],
                ),
                ("meta", 0, vec![(b"meta", b"gob meta")]),
                ("milestones", 9, vec![]),
                ("notes", 8, vec![]),
                ("plans", 7, vec![]),
                ("tasks", 15, vec![(b"\0\0\0\0\0\0\0\x0f", b"gob task")]),
            ],
        ),
    )
    .unwrap();
    let destination = directory.path("project-v5.ReDb");

    let (store, report) = import_path(&bundle_path, &destination).unwrap();
    assert_eq!(report.kind, StoreKind::Project);
    assert_eq!(report.collections.len(), 10);
    assert_eq!(report.record_count, 3);
    drop(store);

    let reopened = Store::open_existing(&destination, StoreKind::Project).unwrap();
    reopened
        .read(|transaction| {
            let task = transaction
                .get(Collection::Tasks, RecordKey::Id(15))?
                .unwrap();
            assert_eq!(task.payload(), b"gob task");
            assert_eq!(task.payload_schema(), 5);
            let receipt = transaction
                .get(
                    Collection::MemoryWritebacks,
                    RecordKey::Bytes(b"agent/run/receipt"),
                )?
                .unwrap();
            assert_eq!(receipt.payload(), b"gob receipt");
            assert_eq!(
                transaction.sequence_high_water(Collection::MemoryWritebacks)?,
                17
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn global_import_preserves_byte_keys_raw_and_gob_payloads_and_reopens() {
    let directory = TestDirectory::new();
    let bundle_path = directory.path("global.bundle");
    fs::write(
        &bundle_path,
        encode(
            2,
            0,
            &[
                ("backups", 0, vec![(b"backup-key", b"raw backup")]),
                ("config", 0, vec![(b"theme", b"dark")]),
                (
                    "projects",
                    0,
                    vec![(b"/tmp/project", b"gob project reference")],
                ),
            ],
        ),
    )
    .unwrap();
    let destination = directory.path("global.redb");

    let (store, report) = import_path(&bundle_path, &destination).unwrap();
    assert_eq!(report.kind, StoreKind::Global);
    assert_eq!(report.record_count, 3);
    assert!(
        report
            .collections
            .iter()
            .all(|collection| collection.sequence.is_none())
    );
    drop(store);

    let reopened = Store::open_existing(&destination, StoreKind::Global).unwrap();
    reopened
        .read(|transaction| {
            let config = transaction
                .get(Collection::GlobalConfig, RecordKey::Bytes(b"theme"))?
                .unwrap();
            assert_eq!(config.payload(), b"dark");
            assert_eq!(config.payload_schema(), 0);
            assert_eq!(config.codec(), Collection::GlobalConfig.legacy_codec());
            let project = transaction
                .get(
                    Collection::GlobalProjects,
                    RecordKey::Bytes(b"/tmp/project"),
                )?
                .unwrap();
            assert_eq!(project.payload(), b"gob project reference");
            assert_eq!(project.codec(), Collection::GlobalProjects.legacy_codec());
            Ok(())
        })
        .unwrap();
}

#[test]
fn corrupt_bundle_is_rejected_before_destination_creation() {
    let directory = TestDirectory::new();
    let bundle_path = directory.path("corrupt.bundle");
    let mut bytes = empty_global_bundle();
    bytes[50] ^= 1;
    fs::write(&bundle_path, bytes).unwrap();
    let destination = directory.path("must-not-exist.redb");

    assert!(import_path(&bundle_path, &destination).is_err());
    assert!(!destination.exists());
}

#[test]
fn unsafe_destination_shapes_are_rejected_without_creation() {
    let directory = TestDirectory::new();
    let bundle_path = directory.path("valid.bundle");
    fs::write(&bundle_path, empty_global_bundle()).unwrap();

    let existing = directory.path("existing.redb");
    fs::write(&existing, b"do not replace").unwrap();
    assert!(import_path(&bundle_path, &existing).is_err());
    assert_eq!(fs::read(&existing).unwrap(), b"do not replace");

    for rejected in [
        directory.path("global.db"),
        directory.path("PtRaCk.Db"),
        directory.path("wrong.sqlite"),
        directory.path("no-extension"),
    ] {
        assert!(import_path(&bundle_path, &rejected).is_err());
        assert!(!rejected.exists());
    }

    assert!(import_path(&bundle_path, Path::new("relative.redb")).is_err());

    let same_path = directory.path("bundle-is-destination.redb");
    fs::write(&same_path, empty_global_bundle()).unwrap();
    assert!(import_path(&same_path, &same_path).is_err());
    assert_eq!(fs::read(&same_path).unwrap(), empty_global_bundle());
}

#[cfg(unix)]
#[test]
fn symbolic_link_parent_is_rejected() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new();
    let bundle_path = directory.path("valid.bundle");
    fs::write(&bundle_path, empty_global_bundle()).unwrap();
    let actual_parent = directory.path("actual");
    fs::create_dir(&actual_parent).unwrap();
    let linked_parent = directory.path("linked");
    symlink(&actual_parent, &linked_parent).unwrap();
    let destination = linked_parent.join("global.redb");

    assert!(import_path(&bundle_path, &destination).is_err());
    assert!(!actual_parent.join("global.redb").exists());
}

#[cfg(unix)]
#[test]
fn non_utf8_and_missing_parent_destinations_are_rejected() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let directory = TestDirectory::new();
    let bundle_path = directory.path("valid.bundle");
    fs::write(&bundle_path, empty_global_bundle()).unwrap();
    let non_utf8 = directory.0.join(OsString::from_vec(vec![0xff, b'.', b'r']));
    assert!(import_path(&bundle_path, &non_utf8).is_err());
    let missing_parent = directory.path("missing").join("global.redb");
    assert!(import_path(&bundle_path, &missing_parent).is_err());
}

fn collection(
    data: &ptrack_store::ImportData,
    collection: Collection,
) -> &ptrack_store::ImportCollection {
    data.collections
        .iter()
        .find(|imported| imported.collection == collection)
        .unwrap()
}

fn empty_global_bundle() -> Vec<u8> {
    encode(
        2,
        0,
        &[
            ("backups", 0, vec![]),
            ("config", 0, vec![]),
            ("projects", 0, vec![]),
        ],
    )
}

fn encode(kind: u8, source: u64, buckets: &[TestBucket<'_>]) -> Vec<u8> {
    let mut output = Vec::new();
    let record_count: u64 = buckets.iter().map(|bucket| bucket.2.len() as u64).sum();
    output.extend_from_slice(b"PTRKMIG1");
    output.extend_from_slice(&1_u16.to_be_bytes());
    output.extend_from_slice(&40_u16.to_be_bytes());
    output.push(kind);
    output.push(0);
    output.extend_from_slice(&0_u16.to_be_bytes());
    output.extend_from_slice(&source.to_be_bytes());
    output.extend_from_slice(
        &u32::try_from(buckets.len())
            .expect("test bucket count fits")
            .to_be_bytes(),
    );
    output.extend_from_slice(&0_u32.to_be_bytes());
    output.extend_from_slice(&record_count.to_be_bytes());
    for (name, sequence, records) in buckets {
        output.extend_from_slice(b"BUKT");
        output.extend_from_slice(
            &u16::try_from(name.len())
                .expect("test bucket name length fits")
                .to_be_bytes(),
        );
        output.extend_from_slice(&0_u16.to_be_bytes());
        output.extend_from_slice(&sequence.to_be_bytes());
        output.extend_from_slice(&(records.len() as u64).to_be_bytes());
        output.extend_from_slice(name.as_bytes());
        for (key, value) in records {
            output.extend_from_slice(&(key.len() as u64).to_be_bytes());
            output.extend_from_slice(&(value.len() as u64).to_be_bytes());
            output.extend_from_slice(key);
            output.extend_from_slice(value);
        }
    }
    let mut hash = Sha256::new();
    hash.update(&output);
    let digest = hash.finish();
    output.extend_from_slice(b"HASH");
    output.extend_from_slice(&1_u16.to_be_bytes());
    output.extend_from_slice(&32_u16.to_be_bytes());
    output.extend_from_slice(&digest);
    output
}
