use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ptrack_core::{Meta, NativeRecord, Timestamp, encode_record};
use redb::{ReadableDatabase, ReadableTable, ReadableTableMetadata};

use super::{
    ActiveBinding, Collection, ImportCollection, ImportRecord, JSON_STAGE_VERSION,
    JsonStageImportData, NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA, OwnedRecordKey, ProjectStore,
    QuarantineReason, QuarantinedLegacyRecord, RecordEnvelope, StagedStore, Store, StoreError,
    StoreKind,
};
use crate::schema::{
    MANIFEST_KEY_BATCH_MANIFEST_SHA256, MANIFEST_KEY_DATABASE_JSON_SHA256,
    MANIFEST_KEY_QUARANTINE_COUNT, MANIFEST_KEY_SOURCE_FORMAT, MANIFEST_KEY_STAGE_VERSION,
    MANIFEST_TABLE, QUARANTINE_TABLE, STORE_ORIGIN_JSON_STAGE,
};
use crate::sha256;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let number = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ptrack-json-stage-store-test-{}-{number}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn json_stage(quarantine: Vec<QuarantinedLegacyRecord>) -> JsonStageImportData {
    json_stage_with_format(quarantine, 5)
}

fn json_stage_with_format(
    quarantine: Vec<QuarantinedLegacyRecord>,
    format_version: u64,
) -> JsonStageImportData {
    let mut collections = Collection::for_store(StoreKind::Project)
        .map(|collection| ImportCollection {
            collection,
            records: Vec::new(),
            sequence: collection.is_sequenced().then_some(0),
        })
        .collect::<Vec<_>>();
    let meta = NativeRecord::Meta(Meta {
        goal: "migration".to_owned(),
        summary: String::new(),
        active_plan: 0,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        format_version,
        last_write_version: "v0.21.0".to_owned(),
    });
    collections[0].records.push(ImportRecord {
        key: OwnedRecordKey::Singleton,
        envelope: RecordEnvelope::new(
            NATIVE_CODEC,
            NATIVE_PAYLOAD_SCHEMA,
            encode_record(&meta).unwrap(),
        ),
    });
    JsonStageImportData {
        kind: StoreKind::Project,
        source_format: 5,
        batch_manifest_sha256: [0x11; 32],
        database_json_sha256: [0x22; 32],
        collections,
        quarantine,
    }
}

fn quarantined_capability() -> QuarantinedLegacyRecord {
    let gob = b"exact invalid legacy gob".to_vec();
    QuarantinedLegacyRecord {
        source_bucket: b"capabilities".to_vec(),
        source_key: 7_u64.to_be_bytes().to_vec(),
        source_value_sha256: sha256::digest(&gob),
        legacy_gob: gob,
        reason: QuarantineReason::InvalidCapability,
    }
}

fn with_key(mut record: QuarantinedLegacyRecord, key: u64) -> QuarantinedLegacyRecord {
    record.source_key = key.to_be_bytes().to_vec();
    record
}

#[test]
fn json_stage_import_attests_private_quarantine_and_survives_reopen() {
    let directory = TestDirectory::new();
    let path = directory.0.join("candidate.redb");
    let (store, report) =
        Store::import_json_stage_new(&path, json_stage(vec![quarantined_capability()])).unwrap();
    assert_eq!(report.record_count, 1);
    assert_eq!(report.quarantine_count, 1);
    assert!(
        store
            .read(|read| read.scan(Collection::Capabilities))
            .unwrap()
            .is_empty()
    );
    let provenance = store.json_stage_provenance().unwrap().unwrap();
    assert_eq!(provenance.stage_version, JSON_STAGE_VERSION);
    assert_eq!(provenance.source_format, 5);
    assert_eq!(provenance.batch_manifest_sha256, [0x11; 32]);
    assert_eq!(provenance.database_json_sha256, [0x22; 32]);
    assert_eq!(provenance.quarantine_count, 1);
    drop(store);

    let database = redb::Database::open(&path).unwrap();
    let transaction = database.begin_read().unwrap();
    let manifest = transaction.open_table(MANIFEST_TABLE).unwrap();
    assert_eq!(manifest.len().unwrap(), 11);
    assert_eq!(
        manifest.get(b"origin".as_slice()).unwrap().unwrap().value(),
        STORE_ORIGIN_JSON_STAGE
    );
    assert_eq!(
        manifest
            .get(MANIFEST_KEY_STAGE_VERSION)
            .unwrap()
            .unwrap()
            .value(),
        JSON_STAGE_VERSION.to_be_bytes()
    );
    assert_eq!(
        manifest
            .get(MANIFEST_KEY_BATCH_MANIFEST_SHA256)
            .unwrap()
            .unwrap()
            .value(),
        [0x11; 32]
    );
    assert_eq!(
        manifest
            .get(MANIFEST_KEY_DATABASE_JSON_SHA256)
            .unwrap()
            .unwrap()
            .value(),
        [0x22; 32]
    );
    assert_eq!(
        manifest
            .get(MANIFEST_KEY_SOURCE_FORMAT)
            .unwrap()
            .unwrap()
            .value(),
        5_u64.to_be_bytes()
    );
    drop(manifest);
    drop(transaction);
    drop(database);

    let reopened = Store::open_existing(&path, StoreKind::Project).unwrap();
    assert_eq!(
        reopened.json_stage_provenance().unwrap().unwrap(),
        provenance
    );
}

#[test]
fn quarantine_hash_is_checked_before_creation_and_on_reopen() {
    let directory = TestDirectory::new();
    let rejected = directory.0.join("rejected.redb");
    let mut quarantine = quarantined_capability();
    quarantine.source_value_sha256 = [0; 32];
    assert!(matches!(
        Store::import_json_stage_new(&rejected, json_stage(vec![quarantine])),
        Err(StoreError::InvalidImport(_))
    ));
    assert!(!rejected.exists());

    let path = directory.0.join("tampered.redb");
    drop(
        Store::import_json_stage_new(&path, json_stage(vec![quarantined_capability()]))
            .unwrap()
            .0,
    );
    let database = redb::Database::open(&path).unwrap();
    let transaction = database.begin_write().unwrap();
    {
        let mut table = transaction.open_table(QUARANTINE_TABLE).unwrap();
        let (key, mut value) = {
            let mut entries = table.iter().unwrap();
            let (key, value) = entries.next().unwrap().unwrap();
            (key.value().to_vec(), value.value().to_vec())
        };
        *value.last_mut().unwrap() ^= 1;
        table.insert(key.as_slice(), value.as_slice()).unwrap();
    }
    transaction.commit().unwrap();
    drop(database);
    let before = fs::read(&path).unwrap();
    assert!(matches!(
        Store::open_existing(&path, StoreKind::Project),
        Err(StoreError::InvalidManifest(_))
    ));
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn failed_ready_fence_leaves_an_inert_unopenable_artifact() {
    let directory = TestDirectory::new();
    let path = directory.0.join("incomplete.redb");
    assert!(matches!(
        Store::import_json_stage_new_with_before_ready(&path, json_stage(vec![]), || Err(
            StoreError::InvalidImport("fence".to_owned())
        ),),
        Err(StoreError::InvalidImport(_))
    ));
    assert!(path.exists());
    assert!(Store::open_existing(&path, StoreKind::Project).is_err());
}

#[test]
fn quarantine_input_is_closed_ordered_and_project_only() {
    let directory = TestDirectory::new();
    let cases = [
        {
            let mut record = quarantined_capability();
            record.source_bucket = b"capability_audits".to_vec();
            vec![record]
        },
        vec![
            with_key(quarantined_capability(), 2),
            with_key(quarantined_capability(), 1),
        ],
        vec![
            with_key(quarantined_capability(), 1),
            with_key(quarantined_capability(), 1),
        ],
    ];
    for (index, quarantine) in cases.into_iter().enumerate() {
        let path = directory.0.join(format!("invalid-{index}.redb"));
        assert!(Store::import_json_stage_new(&path, json_stage(quarantine)).is_err());
        assert!(!path.exists());
    }

    let path = directory.0.join("global-quarantine.redb");
    let data = JsonStageImportData {
        kind: StoreKind::Global,
        source_format: 0,
        batch_manifest_sha256: [1; 32],
        database_json_sha256: [2; 32],
        collections: Collection::for_store(StoreKind::Global)
            .map(|collection| ImportCollection {
                collection,
                records: Vec::new(),
                sequence: None,
            })
            .collect(),
        quarantine: vec![quarantined_capability()],
    };
    assert!(Store::import_json_stage_new(&path, data).is_err());
    assert!(!path.exists());
}

#[test]
fn manifest_quarantine_count_is_exact_and_created_stores_have_no_stage_provenance() {
    let directory = TestDirectory::new();
    let created_path = directory.0.join("created.redb");
    let created = Store::create_new(&created_path, StoreKind::Project).unwrap();
    assert_eq!(created.json_stage_provenance().unwrap(), None);
    drop(created);

    let path = directory.0.join("count.redb");
    drop(
        Store::import_json_stage_new(&path, json_stage(vec![quarantined_capability()]))
            .unwrap()
            .0,
    );
    let database = redb::Database::open(&path).unwrap();
    let transaction = database.begin_write().unwrap();
    transaction
        .open_table(MANIFEST_TABLE)
        .unwrap()
        .insert(
            MANIFEST_KEY_QUARANTINE_COUNT,
            2_u64.to_be_bytes().as_slice(),
        )
        .unwrap();
    transaction.commit().unwrap();
    drop(database);
    let before = fs::read(&path).unwrap();
    assert!(matches!(
        Store::open_existing(&path, StoreKind::Project),
        Err(StoreError::InvalidManifest(_))
    ));
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn ordinary_put_revalidates_records_in_a_json_stage_candidate() {
    let directory = TestDirectory::new();
    let path = directory.0.join("put.redb");
    let (store, _) = Store::import_json_stage_new(&path, json_stage(vec![])).unwrap();
    assert!(matches!(
        store.write(|write| write
            .put(
                Collection::Plans,
                crate::RecordKey::Id(1),
                &RecordEnvelope::new(crate::LEGACY_CODEC_RAW, 0, b"untrusted".to_vec()),
            )
            .map(|_| ())),
        Err(StoreError::InvalidImport(_))
    ));
    assert!(
        store
            .read(|read| read.scan(Collection::Plans))
            .unwrap()
            .is_empty()
    );
    drop(store);
    Store::open_existing(path, StoreKind::Project).unwrap();
}

#[test]
fn json_stage_candidates_are_immutable_through_the_public_store_api() {
    let directory = TestDirectory::new();
    let path = directory.0.join("immutable.redb");
    let (store, _) = Store::import_json_stage_new(&path, json_stage(vec![])).unwrap();
    assert!(matches!(
        store.write(|_| Ok(())),
        Err(StoreError::InvalidImport(detail)) if detail.contains("immutable")
    ));
}

#[test]
fn explicit_activation_retains_stage_provenance_and_quarantine_permanently() {
    let directory = TestDirectory::new();
    let path = directory.0.join("activated.redb");
    drop(
        Store::import_json_stage_new(
            &path,
            json_stage_with_format(vec![quarantined_capability()], 4),
        )
        .unwrap()
        .0,
    );
    let staged = StagedStore::open(&path, StoreKind::Project).unwrap();
    let expected_provenance = staged.provenance().unwrap();
    let binding = ActiveBinding {
        generation: 9,
        database_id: "project-activated".to_owned(),
        kind: StoreKind::Project,
        canonical_path: path.canonicalize().unwrap(),
    };
    let project = ProjectStore::activate(staged, binding.clone(), "test").unwrap();
    let meta = project.meta().unwrap();
    assert_eq!(meta.format_version, 5);
    assert_eq!(meta.goal, "migration");
    assert_eq!(meta.last_write_version, "test");
    assert!(project.application_writes().unwrap());
    drop(project);

    let reopened = Store::open_existing(&path, StoreKind::Project).unwrap();
    assert_eq!(
        reopened.json_stage_provenance().unwrap(),
        Some(expected_provenance)
    );
    assert_eq!(reopened.active_binding().unwrap(), Some(binding));
    drop(reopened);
    let database = redb::Database::open(path).unwrap();
    let transaction = database.begin_read().unwrap();
    assert_eq!(
        transaction
            .open_table(QUARANTINE_TABLE)
            .unwrap()
            .len()
            .unwrap(),
        1
    );
}
