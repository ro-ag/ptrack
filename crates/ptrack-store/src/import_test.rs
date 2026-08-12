use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ptrack_core::{
    Meta, NativeRecord, ProjectRef, RecordKind, Task, TaskStatus, Timestamp, decode_record,
    encode_record,
};

use super::{
    Collection, IMPORT_BUNDLE_VERSION, ImportCollection, ImportData, ImportProvenance,
    ImportRecord, MAX_IMPORT_BYTES, MAX_IMPORT_ENVELOPE_BYTES, MAX_IMPORT_KEY_BYTES,
    MAX_IMPORT_PAYLOAD_BYTES, MAX_IMPORT_RECORDS, OwnedRecordKey, RecordEnvelope, RecordKey, Store,
    StoreError, StoreKind,
};
use crate::import::validated_record_size;
use crate::schema::{
    MANIFEST_KEY_FAMILY, MANIFEST_KEY_IMPORT_BUNDLE_SHA256, MANIFEST_KEY_IMPORT_BUNDLE_VERSION,
    MANIFEST_KEY_IMPORT_SOURCE_FORMAT, MANIFEST_KEY_ORIGIN, MANIFEST_KEY_OWNER,
    MANIFEST_KEY_SCHEMA_VERSION, MANIFEST_KEY_STATE, MANIFEST_KEY_STORE_KIND, MANIFEST_TABLE,
    STORE_ORIGIN_IMPORTED, STORE_STATE_IMPORTING,
};
use redb::{ReadableDatabase, ReadableTable};

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
        provenance: ImportProvenance {
            bundle_version: IMPORT_BUNDLE_VERSION,
            source_format: if kind == StoreKind::Project { 5 } else { 0 },
            bundle_sha256: [0x5a; 32],
        },
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
    let payload = match collection {
        Collection::ProjectMeta => encode_record(&NativeRecord::Meta(Meta {
            goal: String::from_utf8_lossy(payload).into_owned(),
            summary: String::new(),
            active_plan: 0,
            created_at: Timestamp::Zero,
            updated_at: Timestamp::Zero,
            format_version: 5,
            last_write_version: "v0.21.0".to_owned(),
        }))
        .unwrap(),
        Collection::Tasks => {
            let id = match &key {
                OwnedRecordKey::Id(id) if *id != 0 => *id,
                OwnedRecordKey::Singleton | OwnedRecordKey::Id(_) | OwnedRecordKey::Bytes(_) => 1,
            };
            encode_record(&NativeRecord::Task(Task {
                id,
                plan_id: 1,
                title: String::from_utf8_lossy(payload).into_owned(),
                status: TaskStatus::Todo,
                order: 0,
                created_at: Timestamp::Zero,
                updated_at: Timestamp::Zero,
            }))
            .unwrap()
        }
        Collection::GlobalProjects => {
            let OwnedRecordKey::Bytes(ref path) = key else {
                panic!("project-ref test key must be bytes")
            };
            encode_record(&NativeRecord::ProjectRef(ProjectRef {
                name: String::from_utf8_lossy(payload).into_owned(),
                path: String::from_utf8(path.clone()).unwrap(),
                last_seen: Timestamp::Zero,
            }))
            .unwrap()
        }
        Collection::GlobalConfig | Collection::GlobalBackups => payload.to_vec(),
        _ => panic!("test record helper does not support {collection:?}"),
    };
    ImportRecord {
        key,
        envelope: RecordEnvelope::new(
            collection.import_codec(),
            collection.import_payload_schema(),
            payload,
        ),
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
            let task = transaction
                .get(Collection::Tasks, RecordKey::Id(9))?
                .unwrap();
            let NativeRecord::Task(task) = decode_record(RecordKind::Task, task.payload()).unwrap()
            else {
                panic!("task payload")
            };
            assert_eq!(task.title, "gob task nine");
            assert_eq!(transaction.sequence_high_water(Collection::Tasks)?, 14);
            assert_eq!(transaction.sequence_high_water(Collection::Notes)?, 7);
            Ok(())
        })
        .unwrap();
    drop(store);

    let manifest = manifest_entries(&path);
    assert_eq!(manifest.len(), 9);
    assert!(manifest.contains_key(MANIFEST_KEY_FAMILY));
    assert!(manifest.contains_key(MANIFEST_KEY_OWNER));
    assert!(manifest.contains_key(MANIFEST_KEY_SCHEMA_VERSION));
    assert!(manifest.contains_key(MANIFEST_KEY_STATE));
    assert!(manifest.contains_key(MANIFEST_KEY_STORE_KIND));
    assert_eq!(manifest[MANIFEST_KEY_ORIGIN], STORE_ORIGIN_IMPORTED);
    assert_eq!(
        manifest[MANIFEST_KEY_IMPORT_BUNDLE_VERSION],
        IMPORT_BUNDLE_VERSION.to_be_bytes()
    );
    assert_eq!(
        manifest[MANIFEST_KEY_IMPORT_SOURCE_FORMAT],
        5_u64.to_be_bytes()
    );
    assert_eq!(manifest[MANIFEST_KEY_IMPORT_BUNDLE_SHA256], [0x5a; 32]);

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
fn successful_global_import_uses_raw_and_native_codecs() {
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
            assert_eq!(config.codec(), Collection::GlobalConfig.import_codec());
            assert_eq!(config.payload(), b"dark");
            let project = transaction
                .get(
                    Collection::GlobalProjects,
                    RecordKey::Bytes(b"/tmp/project"),
                )?
                .unwrap();
            assert_eq!(project.codec(), Collection::GlobalProjects.import_codec());
            Ok(())
        })
        .unwrap();
}

#[test]
fn imported_provenance_is_strictly_validated_without_open_mutation() {
    let directory = TestDirectory::new();
    for case in 0..5 {
        let path = directory.path(&format!("malformed-provenance-{case}.redb"));
        let (store, _) = Store::import_new(&path, complete_import(StoreKind::Global)).unwrap();
        drop(store);

        let database = redb::Database::open(&path).unwrap();
        let transaction = database.begin_write().unwrap();
        {
            let mut manifest = transaction.open_table(MANIFEST_TABLE).unwrap();
            match case {
                0 => manifest
                    .insert(MANIFEST_KEY_IMPORT_BUNDLE_VERSION, [2_u8].as_slice())
                    .unwrap(),
                1 => manifest
                    .insert(MANIFEST_KEY_IMPORT_SOURCE_FORMAT, [0_u8; 7].as_slice())
                    .unwrap(),
                2 => manifest
                    .insert(MANIFEST_KEY_IMPORT_BUNDLE_SHA256, [0_u8; 31].as_slice())
                    .unwrap(),
                3 => manifest
                    .insert(b"unexpected".as_slice(), b"value".as_slice())
                    .unwrap(),
                4 => manifest
                    .insert(
                        MANIFEST_KEY_IMPORT_SOURCE_FORMAT,
                        1_u64.to_be_bytes().as_slice(),
                    )
                    .unwrap(),
                _ => unreachable!(),
            };
        }
        transaction.commit().unwrap();
        drop(database);

        let before = fs::read(&path).unwrap();
        assert!(matches!(
            Store::open_existing(&path, StoreKind::Global),
            Err(StoreError::InvalidManifest(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
    }
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
                        Collection::GlobalProjects.import_codec(),
                        Collection::GlobalProjects.import_payload_schema(),
                        b"value",
                    ),
                });
            data
        }),
        ("wrong-payload-schema.redb", {
            let mut data = complete_import(StoreKind::Global);
            collection_mut(&mut data, Collection::GlobalProjects)
                .records
                .push(ImportRecord {
                    key: OwnedRecordKey::Bytes(b"/project".to_vec()),
                    envelope: RecordEnvelope::new(
                        Collection::GlobalProjects.import_codec(),
                        0,
                        b"value",
                    ),
                });
            data
        }),
        ("wrong-bundle-version.redb", {
            let mut data = complete_import(StoreKind::Global);
            data.provenance.bundle_version = 1;
            data
        }),
        ("unsupported-global-source-format.redb", {
            let mut data = complete_import(StoreKind::Global);
            data.provenance.source_format = 1;
            data
        }),
        ("unsupported-project-source-format.redb", {
            let mut data = complete_import(StoreKind::Project);
            data.provenance.source_format = 6;
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
    let manifest = manifest_entries(&path);
    assert_eq!(manifest[MANIFEST_KEY_ORIGIN], STORE_ORIGIN_IMPORTED);
    assert_eq!(manifest[MANIFEST_KEY_STATE], STORE_STATE_IMPORTING);
    assert!(!manifest.contains_key(MANIFEST_KEY_IMPORT_BUNDLE_VERSION));
    assert!(!manifest.contains_key(MANIFEST_KEY_IMPORT_SOURCE_FORMAT));
    assert!(!manifest.contains_key(MANIFEST_KEY_IMPORT_BUNDLE_SHA256));
}

#[cfg(unix)]
#[test]
fn import_rejects_parent_swap_before_creation_without_artifact() {
    let directory = TestDirectory::new();
    let parent = directory.path("destination-parent");
    let held_parent = directory.path("held-parent");
    fs::create_dir(&parent).unwrap();
    let path = parent.join("project.redb");
    fs::write(parent.join("marker"), b"original").unwrap();

    let swap_parent = parent.clone();
    let move_to = held_parent.clone();
    let result = Store::import_new_with_parent_hooks(
        &path,
        complete_import(StoreKind::Project),
        move || {
            fs::rename(&swap_parent, &move_to)?;
            fs::create_dir(&swap_parent)?;
            fs::write(swap_parent.join("replacement-marker"), b"replacement")?;
            Ok(())
        },
        || Ok(()),
        || Ok(()),
        || Ok(()),
    );

    assert!(matches!(
        result,
        Err(StoreError::DestinationParentChanged { .. })
    ));
    assert!(!path.exists());
    assert!(!held_parent.join("project.redb").exists());
    assert_eq!(fs::read(held_parent.join("marker")).unwrap(), b"original");
    assert_eq!(
        fs::read(parent.join("replacement-marker")).unwrap(),
        b"replacement"
    );
}

#[cfg(unix)]
#[test]
fn import_rejects_parent_swap_after_creation_with_only_private_empty_artifact() {
    use std::os::unix::fs::MetadataExt;

    let directory = TestDirectory::new();
    let parent = directory.path("destination-parent");
    let held_parent = directory.path("held-parent");
    fs::create_dir(&parent).unwrap();
    let path = parent.join("project.redb");
    fs::write(parent.join("marker"), b"original").unwrap();

    let swap_parent = parent.clone();
    let move_to = held_parent.clone();
    let result = Store::import_new_with_parent_hooks(
        &path,
        complete_import(StoreKind::Project),
        || Ok(()),
        move || {
            fs::rename(&swap_parent, &move_to)?;
            fs::create_dir(&swap_parent)?;
            fs::write(swap_parent.join("replacement-marker"), b"replacement")?;
            Ok(())
        },
        || Ok(()),
        || Ok(()),
    );

    assert!(matches!(
        result,
        Err(StoreError::DestinationParentChanged { .. })
    ));
    assert!(!path.exists());
    let retained = held_parent.join("project.redb");
    let metadata = fs::metadata(&retained).unwrap();
    assert_eq!(metadata.len(), 0);
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(fs::read(held_parent.join("marker")).unwrap(), b"original");
    assert_eq!(
        fs::read(parent.join("replacement-marker")).unwrap(),
        b"replacement"
    );
}

#[cfg(unix)]
#[test]
fn import_parent_swap_before_ready_leaves_only_importing_artifact() {
    let directory = TestDirectory::new();
    let parent = directory.path("destination-parent");
    let held_parent = directory.path("held-parent");
    fs::create_dir(&parent).unwrap();
    let path = parent.join("project.redb");

    let swap_parent = parent.clone();
    let move_to = held_parent.clone();
    let result = Store::import_new_with_parent_hooks(
        &path,
        complete_import(StoreKind::Project),
        || Ok(()),
        || Ok(()),
        move || {
            fs::rename(&swap_parent, &move_to)?;
            fs::create_dir(&swap_parent)?;
            fs::write(swap_parent.join("replacement-marker"), b"replacement")?;
            Ok(())
        },
        || Ok(()),
    );

    assert!(matches!(
        result,
        Err(StoreError::DestinationParentChanged { .. })
    ));
    assert!(!path.exists());
    let retained = held_parent.join("project.redb");
    let manifest = manifest_entries(&retained);
    assert_eq!(manifest[MANIFEST_KEY_STATE], STORE_STATE_IMPORTING);
    assert!(!manifest.contains_key(MANIFEST_KEY_IMPORT_BUNDLE_VERSION));
    assert!(!manifest.contains_key(MANIFEST_KEY_IMPORT_SOURCE_FORMAT));
    assert!(!manifest.contains_key(MANIFEST_KEY_IMPORT_BUNDLE_SHA256));
    assert_eq!(
        fs::read(parent.join("replacement-marker")).unwrap(),
        b"replacement"
    );
}

#[cfg(unix)]
#[test]
fn import_parent_swap_after_ready_reports_committed_path_change() {
    let directory = TestDirectory::new();
    let parent = directory.path("destination-parent");
    let held_parent = directory.path("held-parent");
    fs::create_dir(&parent).unwrap();
    let path = parent.join("project.redb");

    let swap_parent = parent.clone();
    let move_to = held_parent.clone();
    let result = Store::import_new_with_parent_hooks(
        &path,
        complete_import(StoreKind::Project),
        || Ok(()),
        || Ok(()),
        || Ok(()),
        move || {
            fs::rename(&swap_parent, &move_to)?;
            fs::create_dir(&swap_parent)?;
            fs::write(swap_parent.join("replacement-marker"), b"replacement")?;
            Ok(())
        },
    );

    assert!(matches!(
        result,
        Err(StoreError::ImportCommittedPathChanged { .. })
    ));
    assert!(!path.exists());
    let retained = held_parent.join("project.redb");
    let manifest = manifest_entries(&retained);
    assert_eq!(
        manifest[MANIFEST_KEY_STATE],
        crate::schema::STORE_STATE_READY
    );
    assert_eq!(
        manifest[MANIFEST_KEY_IMPORT_BUNDLE_VERSION],
        IMPORT_BUNDLE_VERSION.to_be_bytes()
    );
    assert!(manifest.contains_key(MANIFEST_KEY_IMPORT_SOURCE_FORMAT));
    assert!(manifest.contains_key(MANIFEST_KEY_IMPORT_BUNDLE_SHA256));
    assert_eq!(
        fs::read(parent.join("replacement-marker")).unwrap(),
        b"replacement"
    );
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
    let manifest = manifest_entries(&path);
    assert_eq!(manifest[MANIFEST_KEY_STATE], STORE_STATE_IMPORTING);
    assert!(!manifest.contains_key(MANIFEST_KEY_IMPORT_BUNDLE_VERSION));
}

fn manifest_entries(path: &std::path::Path) -> std::collections::BTreeMap<Vec<u8>, Vec<u8>> {
    let database = redb::Database::open(path).unwrap();
    let transaction = database.begin_read().unwrap();
    let manifest = transaction.open_table(MANIFEST_TABLE).unwrap();
    manifest
        .iter()
        .unwrap()
        .map(|entry| {
            let (key, value) = entry.unwrap();
            (key.value().to_vec(), value.value().to_vec())
        })
        .collect()
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
