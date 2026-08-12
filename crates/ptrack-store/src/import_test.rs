use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{
    Collection, ImportCollection, ImportData, ImportRecord, MAX_IMPORT_BYTES,
    MAX_IMPORT_ENVELOPE_BYTES, MAX_IMPORT_KEY_BYTES, MAX_IMPORT_PAYLOAD_BYTES, MAX_IMPORT_RECORDS,
    OwnedRecordKey, RecordEnvelope, RecordKey, Store, StoreError, StoreKind,
};
use crate::import::validated_record_size;

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let number = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ptrack-import-test-{}-{number}",
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

fn complete_import(kind: StoreKind) -> ImportData {
    let mut data = ImportData {
        kind,
        collections: Collection::for_store(kind)
            .map(|collection| ImportCollection {
                collection,
                records: Vec::new(),
                sequence: collection.is_sequenced().then_some(0),
            })
            .collect(),
    };
    if kind == StoreKind::Project {
        collection_mut(&mut data, Collection::ProjectMeta)
            .records
            .push(record(
                Collection::ProjectMeta,
                OwnedRecordKey::Singleton,
                b"gob meta",
            ));
    }
    data
}

#[test]
fn import_limits_enforce_common_bounds_without_boundary_allocations() {
    assert_eq!(MAX_IMPORT_RECORDS, 1_000_000);
    assert_eq!(MAX_IMPORT_KEY_BYTES, 1024 * 1024);
    assert_eq!(MAX_IMPORT_PAYLOAD_BYTES, 256 * 1024 * 1024);
    assert_eq!(MAX_IMPORT_ENVELOPE_BYTES, 256 * 1024 * 1024 + 20);
    assert_eq!(MAX_IMPORT_BYTES, 256 * 1024 * 1024);
    assert_eq!(validated_record_size(1, 0).unwrap(), 21);
    assert_eq!(
        validated_record_size(1, (MAX_IMPORT_BYTES - 21) as usize).unwrap(),
        MAX_IMPORT_BYTES
    );
    assert!(validated_record_size((MAX_IMPORT_KEY_BYTES + 1) as usize, 0).is_err());
    assert!(validated_record_size(1, (MAX_IMPORT_PAYLOAD_BYTES + 1) as usize).is_err());
    assert!(validated_record_size(1, MAX_IMPORT_PAYLOAD_BYTES as usize).is_err());
}

fn collection_mut(data: &mut ImportData, collection: Collection) -> &mut ImportCollection {
    data.collections
        .iter_mut()
        .find(|imported| imported.collection == collection)
        .unwrap()
}

fn record(collection: Collection, key: OwnedRecordKey, payload: &[u8]) -> ImportRecord {
    ImportRecord {
        key,
        envelope: RecordEnvelope::new(collection.legacy_codec(), 6, payload),
    }
}

#[test]
fn successful_project_import_preserves_records_gaps_and_exact_high_water() {
    let directory = TestDirectory::new();
    let path = directory.path("project.redb");
    let mut data = complete_import(StoreKind::Project);
    let tasks = collection_mut(&mut data, Collection::Tasks);
    tasks.records.push(record(
        Collection::Tasks,
        OwnedRecordKey::Id(2),
        b"gob task two",
    ));
    tasks.records.push(record(
        Collection::Tasks,
        OwnedRecordKey::Id(9),
        b"gob task nine",
    ));
    tasks.sequence = Some(14);
    collection_mut(&mut data, Collection::Notes).sequence = Some(7);

    let (store, report) = Store::import_new(&path, data).unwrap();
    assert_eq!(report.kind, StoreKind::Project);
    assert_eq!(report.record_count, 3);
    assert_eq!(
        report
            .collections
            .iter()
            .find(|item| item.collection == Collection::Tasks)
            .unwrap()
            .sequence,
        Some(14)
    );
    store
        .read(|transaction| {
            assert_eq!(
                transaction
                    .get(Collection::Tasks, RecordKey::Id(9))?
                    .unwrap()
                    .payload(),
                b"gob task nine"
            );
            assert_eq!(transaction.sequence_high_water(Collection::Tasks)?, 14);
            assert_eq!(transaction.sequence_high_water(Collection::Notes)?, 7);
            Ok(())
        })
        .unwrap();
    drop(store);

    let reopened = Store::open_existing(&path, StoreKind::Project).unwrap();
    reopened
        .read(|transaction| {
            assert_eq!(transaction.scan(Collection::Tasks)?.len(), 2);
            assert_eq!(transaction.sequence_high_water(Collection::Tasks)?, 14);
            Ok(())
        })
        .unwrap();
}

#[test]
fn successful_global_import_uses_raw_and_gob_legacy_codecs() {
    let directory = TestDirectory::new();
    let path = directory.path("registry.redb");
    let mut data = complete_import(StoreKind::Global);
    collection_mut(&mut data, Collection::GlobalConfig)
        .records
        .push(record(
            Collection::GlobalConfig,
            OwnedRecordKey::Bytes(b"theme".to_vec()),
            b"dark",
        ));
    collection_mut(&mut data, Collection::GlobalProjects)
        .records
        .push(record(
            Collection::GlobalProjects,
            OwnedRecordKey::Bytes(b"/tmp/project".to_vec()),
            b"gob project ref",
        ));
    collection_mut(&mut data, Collection::GlobalBackups)
        .records
        .push(record(
            Collection::GlobalBackups,
            OwnedRecordKey::Bytes(b"123".to_vec()),
            b"project\tbackup",
        ));

    let (store, report) = Store::import_new(&path, data).unwrap();
    assert_eq!(report.record_count, 3);
    assert!(
        report
            .collections
            .iter()
            .all(|item| item.sequence.is_none())
    );
    store
        .read(|transaction| {
            let config = transaction
                .get(Collection::GlobalConfig, RecordKey::Bytes(b"theme"))?
                .unwrap();
            assert_eq!(config.codec(), Collection::GlobalConfig.legacy_codec());
            assert_eq!(config.payload(), b"dark");
            let project = transaction
                .get(
                    Collection::GlobalProjects,
                    RecordKey::Bytes(b"/tmp/project"),
                )?
                .unwrap();
            assert_eq!(project.codec(), Collection::GlobalProjects.legacy_codec());
            Ok(())
        })
        .unwrap();
}

#[test]
fn every_invalid_import_is_rejected_before_destination_creation() {
    let directory = TestDirectory::new();
    let cases: Vec<(&str, ImportData)> = vec![
        ("missing.redb", {
            let mut data = complete_import(StoreKind::Global);
            data.collections.pop();
            data
        }),
        ("duplicate-collection.redb", {
            let mut data = complete_import(StoreKind::Global);
            data.collections.push(data.collections[0].clone());
            data
        }),
        ("wrong-family.redb", {
            let mut data = complete_import(StoreKind::Global);
            data.collections[0].collection = Collection::Tasks;
            data
        }),
        ("sequence-missing.redb", {
            let mut data = complete_import(StoreKind::Project);
            collection_mut(&mut data, Collection::Tasks).sequence = None;
            data
        }),
        ("sequence-extra.redb", {
            let mut data = complete_import(StoreKind::Global);
            collection_mut(&mut data, Collection::GlobalConfig).sequence = Some(0);
            data
        }),
        ("sequence-low.redb", {
            let mut data = complete_import(StoreKind::Project);
            let tasks = collection_mut(&mut data, Collection::Tasks);
            tasks
                .records
                .push(record(Collection::Tasks, OwnedRecordKey::Id(4), b"task"));
            tasks.sequence = Some(3);
            data
        }),
        ("meta-empty.redb", {
            let mut data = complete_import(StoreKind::Project);
            collection_mut(&mut data, Collection::ProjectMeta)
                .records
                .clear();
            data
        }),
        ("duplicate-key.redb", {
            let mut data = complete_import(StoreKind::Global);
            let config = collection_mut(&mut data, Collection::GlobalConfig);
            config.records.push(record(
                Collection::GlobalConfig,
                OwnedRecordKey::Bytes(b"same".to_vec()),
                b"one",
            ));
            config.records.push(record(
                Collection::GlobalConfig,
                OwnedRecordKey::Bytes(b"same".to_vec()),
                b"two",
            ));
            data
        }),
        ("out-of-order.redb", {
            let mut data = complete_import(StoreKind::Global);
            let config = collection_mut(&mut data, Collection::GlobalConfig);
            config.records.push(record(
                Collection::GlobalConfig,
                OwnedRecordKey::Bytes(b"z".to_vec()),
                b"last",
            ));
            config.records.push(record(
                Collection::GlobalConfig,
                OwnedRecordKey::Bytes(b"a".to_vec()),
                b"first",
            ));
            data
        }),
        ("wrong-key.redb", {
            let mut data = complete_import(StoreKind::Project);
            collection_mut(&mut data, Collection::Tasks)
                .records
                .push(record(
                    Collection::Tasks,
                    OwnedRecordKey::Bytes(b"not-an-id".to_vec()),
                    b"task",
                ));
            data
        }),
        ("zero-id.redb", {
            let mut data = complete_import(StoreKind::Project);
            collection_mut(&mut data, Collection::Tasks)
                .records
                .push(record(Collection::Tasks, OwnedRecordKey::Id(0), b"task"));
            data
        }),
        ("wrong-codec.redb", {
            let mut data = complete_import(StoreKind::Global);
            collection_mut(&mut data, Collection::GlobalConfig)
                .records
                .push(ImportRecord {
                    key: OwnedRecordKey::Bytes(b"key".to_vec()),
                    envelope: RecordEnvelope::new(
                        Collection::GlobalProjects.legacy_codec(),
                        6,
                        b"value",
                    ),
                });
            data
        }),
    ];

    for (name, data) in cases {
        let path = directory.path(name);
        assert!(Store::import_new(&path, data).is_err(), "{name}");
        assert!(!path.exists(), "{name} must not create an artifact");
    }
}

#[test]
fn error_before_ready_leaves_importing_artifact_that_normal_open_rejects() {
    let directory = TestDirectory::new();
    let path = directory.path("interrupted.redb");
    let observed_path = path.clone();
    let result =
        Store::import_new_with_before_ready(&path, complete_import(StoreKind::Global), move || {
            assert!(observed_path.exists());
            assert!(Store::open_existing(&observed_path, StoreKind::Global).is_err());
            Err(StoreError::InvalidImport("injected failure".to_owned()))
        });
    assert!(matches!(result, Err(StoreError::InvalidImport(_))));
    assert!(path.exists());
    assert!(matches!(
        Store::open_existing(&path, StoreKind::Global),
        Err(StoreError::InvalidManifest(_))
    ));
}

#[test]
fn panic_before_ready_leaves_importing_artifact_that_normal_open_rejects() {
    let directory = TestDirectory::new();
    let path = directory.path("panicked.redb");
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _ =
            Store::import_new_with_before_ready(&path, complete_import(StoreKind::Project), || {
                panic!("injected import panic")
            });
    }));
    assert!(panic.is_err());
    assert!(path.exists());
    assert!(matches!(
        Store::open_existing(&path, StoreKind::Project),
        Err(StoreError::InvalidManifest(_))
    ));
}

#[test]
fn import_obeys_create_only_and_legacy_path_protections() {
    let directory = TestDirectory::new();
    let existing = directory.path("existing.redb");
    fs::write(&existing, b"do not replace").unwrap();
    let before = fs::read(&existing).unwrap();
    assert!(matches!(
        Store::import_new(&existing, complete_import(StoreKind::Global)),
        Err(StoreError::DestinationExists { .. })
    ));
    assert_eq!(fs::read(&existing).unwrap(), before);

    for name in ["ptrack.db", "PTrAcK.Db", "global.db", "GlObAl.Db"] {
        let path = directory.path(name);
        assert!(matches!(
            Store::import_new(&path, complete_import(StoreKind::Global)),
            Err(StoreError::LegacyPathForbidden { .. })
        ));
        assert!(!path.exists());
    }
}
