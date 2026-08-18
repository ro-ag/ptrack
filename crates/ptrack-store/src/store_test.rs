use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ptrack_core::{
    Digest32, Issue, IssueStatus, MemoryKind, MemoryWritebackRecord, NativeRecord, Note,
    NoteTarget, Plan, PlanStatus, Severity, Task, TaskStatus, Timestamp, encode_record,
};
use redb::{ReadableDatabase, ReadableTable, TableDefinition};

use super::{
    Collection, NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA, OwnedRecordKey, RecordEnvelope, RecordKey,
    Store, StoreError, StoreKind,
};
use crate::schema::{
    MANIFEST_KEY_FAMILY, MANIFEST_KEY_ORIGIN, MANIFEST_KEY_OWNER, MANIFEST_KEY_SCHEMA_VERSION,
    MANIFEST_KEY_STATE, MANIFEST_KEY_STORE_KIND, MANIFEST_TABLE, STORE_FAMILY,
    STORE_ORIGIN_CREATED, STORE_OWNER, STORE_STATE_READY,
};
use crate::store::{FileIdentity, ensure_path_identity};
use crate::{protect_private_directory, protect_private_file};

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let number = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ptrack-store-test-{}-{number}", std::process::id()));
        fs::create_dir(&path).unwrap();
        protect_private_directory(&path).unwrap();
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

fn project_store() -> (TestDirectory, PathBuf, Store) {
    let directory = TestDirectory::new();
    let path = directory.path("project.redb");
    let store = Store::create_new(&path, StoreKind::Project).unwrap();
    (directory, path, store)
}

fn native(record: NativeRecord) -> RecordEnvelope {
    RecordEnvelope::new(
        NATIVE_CODEC,
        NATIVE_PAYLOAD_SCHEMA,
        encode_record(&record).unwrap(),
    )
}

fn plan(id: u64) -> RecordEnvelope {
    native(NativeRecord::Plan(Plan {
        id,
        title: format!("plan {id}"),
        status: PlanStatus::Active,
        milestone_id: 0,
        order: 0,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
    }))
}

fn task(id: u64) -> RecordEnvelope {
    native(NativeRecord::Task(Task {
        id,
        plan_id: 1,
        title: format!("task {id}"),
        status: TaskStatus::Todo,
        order: 0,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
        hold_reason: None,
    }))
}

fn note(id: u64) -> RecordEnvelope {
    native(NativeRecord::Note(Note {
        id,
        target: NoteTarget::Project,
        target_id: 0,
        kind: MemoryKind::Decision,
        body: "note".to_owned(),
        created_at: Timestamp::Zero,
    }))
}

fn issue(id: u64) -> RecordEnvelope {
    native(NativeRecord::Issue(Issue {
        id,
        title: format!("issue {id}"),
        body: String::new(),
        status: IssueStatus::Open,
        severity: Severity::Low,
        task_id: 0,
        created_at: Timestamp::Zero,
        updated_at: Timestamp::Zero,
    }))
}

fn memory(sequence: u64) -> RecordEnvelope {
    native(NativeRecord::MemoryWriteback(MemoryWritebackRecord {
        digest: Digest32([1; 32]),
        sequence,
        kind: MemoryKind::Summary,
        note_id: 0,
    }))
}

#[test]
fn created_store_uses_exact_schema_v4_manifest() {
    let (directory, path, store) = project_store();
    drop(store);

    let entries = manifest_entries(&path);
    assert_eq!(
        entries,
        BTreeMap::from([
            (MANIFEST_KEY_FAMILY.to_vec(), STORE_FAMILY.to_vec()),
            (MANIFEST_KEY_ORIGIN.to_vec(), STORE_ORIGIN_CREATED.to_vec()),
            (MANIFEST_KEY_OWNER.to_vec(), STORE_OWNER.to_vec()),
            (
                MANIFEST_KEY_SCHEMA_VERSION.to_vec(),
                4_u32.to_be_bytes().to_vec(),
            ),
            (MANIFEST_KEY_STATE.to_vec(), STORE_STATE_READY.to_vec()),
            (MANIFEST_KEY_STORE_KIND.to_vec(), b"project".to_vec()),
        ])
    );
    drop(directory);
}

#[test]
fn committed_records_and_sequences_survive_reopen() {
    let (directory, path, store) = project_store();
    let plan = plan(7);
    let replay = memory(1);

    store
        .write(|transaction| {
            transaction.put(Collection::Plans, RecordKey::Id(7), &plan)?;
            transaction.advance_high_water(Collection::Plans, 12)?;
            assert_eq!(transaction.next_id(Collection::MemoryWritebacks)?, 1);
            transaction.put(
                Collection::MemoryWritebacks,
                RecordKey::Bytes(b"request-1"),
                &replay,
            )?;
            Ok(())
        })
        .unwrap();
    drop(store);

    let reopened = Store::open_existing(&path, StoreKind::Project).unwrap();
    assert_eq!(reopened.path(), path);
    assert_eq!(reopened.kind(), StoreKind::Project);
    reopened
        .read(|transaction| {
            assert_eq!(
                transaction.get(Collection::Plans, RecordKey::Id(7))?,
                Some(plan.clone())
            );
            assert_eq!(transaction.sequence_high_water(Collection::Plans)?, 12);
            assert_eq!(
                transaction.sequence_high_water(Collection::MemoryWritebacks)?,
                1
            );
            assert_eq!(
                transaction.scan(Collection::MemoryWritebacks)?,
                vec![(OwnedRecordKey::Bytes(b"request-1".to_vec()), replay.clone())]
            );
            Ok(())
        })
        .unwrap();

    drop(reopened);
    drop(directory);
}

#[test]
fn closure_error_rolls_back_records_and_sequence_allocation() {
    let (_directory, _path, store) = project_store();

    let error = store.write(|transaction| -> Result<(), StoreError> {
        assert_eq!(transaction.next_id(Collection::Tasks)?, 1);
        transaction.put(Collection::Tasks, RecordKey::Id(1), &task(1))?;
        Err(StoreError::InvalidManifest("sentinel".to_owned()))
    });
    assert!(matches!(error, Err(StoreError::InvalidManifest(_))));

    store
        .read(|transaction| {
            assert_eq!(transaction.get(Collection::Tasks, RecordKey::Id(1))?, None);
            assert_eq!(transaction.sequence_high_water(Collection::Tasks)?, 0);
            Ok(())
        })
        .unwrap();
}

#[test]
fn closure_panic_rolls_back_records_and_sequence_allocation() {
    let (_directory, _path, store) = project_store();

    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _ = store.write(|transaction| -> Result<(), StoreError> {
            let id = transaction.next_id(Collection::Notes)?;
            transaction.put(Collection::Notes, RecordKey::Id(id), &note(id))?;
            panic!("test panic");
        });
    }));
    assert!(panic.is_err());

    store
        .read(|transaction| {
            assert_eq!(transaction.scan(Collection::Notes)?, []);
            assert_eq!(transaction.sequence_high_water(Collection::Notes)?, 0);
            Ok(())
        })
        .unwrap();
}

#[test]
fn corrupt_stored_record_is_rejected_on_open_without_mutation() {
    let directory = TestDirectory::new();
    let path = directory.path("poison.redb");
    drop(Store::create_new(&path, StoreKind::Project).unwrap());

    let corrupt_value = b"corrupt old envelope";
    let database = redb::Database::open(&path).unwrap();
    let transaction = database.begin_write().unwrap();
    {
        let mut tasks = transaction.open_table(Collection::Tasks.table()).unwrap();
        tasks
            .insert(1_u64.to_be_bytes().as_slice(), corrupt_value.as_slice())
            .unwrap();
    }
    transaction.commit().unwrap();
    drop(database);

    let before = fs::read(&path).unwrap();
    assert!(matches!(
        Store::open_existing(&path, StoreKind::Project),
        Err(StoreError::Envelope(_))
    ));
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn high_water_marks_preserve_deletions_gaps_and_overflow() {
    let (_directory, _path, store) = project_store();
    let record = issue(5);

    store
        .write(|transaction| {
            transaction.put(Collection::Issues, RecordKey::Id(5), &record)?;
            transaction.delete(Collection::Issues, RecordKey::Id(5))?;
            transaction.advance_high_water(Collection::Issues, 10)?;
            assert_eq!(transaction.next_id(Collection::Issues)?, 11);
            transaction.advance_high_water(Collection::Capabilities, u64::MAX)?;
            Ok(())
        })
        .unwrap();

    assert!(matches!(
        store.write(|transaction| transaction.advance_high_water(Collection::Issues, 9)),
        Err(StoreError::SequenceWouldDecrease { .. })
    ));
    assert!(matches!(
        store.write(|transaction| transaction.next_id(Collection::Capabilities).map(|_| ())),
        Err(StoreError::SequenceOverflow { .. })
    ));
    store
        .read(|transaction| {
            assert_eq!(transaction.scan(Collection::Issues)?, []);
            assert_eq!(transaction.sequence_high_water(Collection::Issues)?, 11);
            assert_eq!(
                transaction.sequence_high_water(Collection::Capabilities)?,
                u64::MAX
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn project_and_global_collections_cannot_cross() {
    let directory = TestDirectory::new();
    let path = directory.path("global.redb");
    let store = Store::create_new(&path, StoreKind::Global).unwrap();

    assert!(matches!(
        store.read(|transaction| transaction.get(Collection::Tasks, RecordKey::Id(1))),
        Err(StoreError::CollectionStoreMismatch { .. })
    ));
    assert!(matches!(
        store.write(|transaction| transaction.put(
            Collection::GlobalConfig,
            RecordKey::Id(1),
            &RecordEnvelope::new(2, 1, [])
        )),
        Err(StoreError::KeyKindMismatch { .. })
    ));
    assert!(matches!(
        store.read(|transaction| transaction.sequence_high_water(Collection::GlobalConfig)),
        Err(StoreError::SequenceNotSupported { .. })
    ));
}

#[test]
fn create_new_never_clobbers_an_existing_file() {
    let directory = TestDirectory::new();
    let path = directory.path("existing.redb");
    write_private_file(&path, b"legacy bbolt bytes");
    let before = fs::read(&path).unwrap();

    assert!(matches!(
        Store::create_new(&path, StoreKind::Project),
        Err(StoreError::DestinationExists { .. })
    ));
    assert_eq!(fs::read(path).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn create_new_requires_an_existing_real_parent_directory() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new();
    let missing = directory.path("missing").join("project.redb");
    assert!(matches!(
        Store::create_new(&missing, StoreKind::Project),
        Err(StoreError::DestinationParentInvalid { .. })
    ));
    assert!(!missing.exists());

    let file_parent = directory.path("file-parent");
    write_private_file(&file_parent, b"parent");
    assert!(matches!(
        Store::create_new(file_parent.join("project.redb"), StoreKind::Project),
        Err(StoreError::DestinationParentInvalid { .. })
    ));
    assert_eq!(fs::read(file_parent).unwrap(), b"parent");

    let real_parent = directory.path("real-parent");
    let linked_parent = directory.path("linked-parent");
    fs::create_dir(&real_parent).unwrap();
    symlink(&real_parent, &linked_parent).unwrap();
    assert!(matches!(
        Store::create_new(linked_parent.join("project.redb"), StoreKind::Project),
        Err(StoreError::DestinationParentInvalid { .. })
    ));
    assert!(!real_parent.join("project.redb").exists());
}

#[cfg(unix)]
#[test]
fn create_new_rejects_parent_swap_before_creation_without_artifact() {
    let directory = TestDirectory::new();
    let parent = directory.path("destination-parent");
    let held_parent = directory.path("held-parent");
    fs::create_dir(&parent).unwrap();
    let path = parent.join("project.redb");
    let marker = parent.join("marker");
    write_private_file(&marker, b"original");

    let swap_parent = parent.clone();
    let move_to = held_parent.clone();
    let result = Store::create_new_with_creation_hooks(
        &path,
        StoreKind::Project,
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
    assert!(!held_parent.join("project.redb").exists());
    assert_eq!(fs::read(held_parent.join("marker")).unwrap(), b"original");
    assert_eq!(
        fs::read(parent.join("replacement-marker")).unwrap(),
        b"replacement"
    );
}

#[cfg(unix)]
#[test]
fn create_new_rejects_parent_swap_after_creation_before_redb_writes() {
    use std::os::unix::fs::MetadataExt;

    let directory = TestDirectory::new();
    let parent = directory.path("destination-parent");
    let held_parent = directory.path("held-parent");
    fs::create_dir(&parent).unwrap();
    let path = parent.join("project.redb");
    let marker = parent.join("marker");
    write_private_file(&marker, b"original");

    let swap_parent = parent.clone();
    let move_to = held_parent.clone();
    let result = Store::create_new_with_creation_hooks(
        &path,
        StoreKind::Project,
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

#[test]
fn file_identity_detects_path_replacement() {
    let directory = TestDirectory::new();
    let path = directory.path("identity.redb");
    write_private_file(&path, b"first");
    let identity = FileIdentity::from_path(&path, false).unwrap();
    fs::rename(&path, directory.path("old.redb")).unwrap();
    write_private_file(&path, b"second");

    assert!(matches!(
        ensure_path_identity(&path, identity),
        Err(StoreError::PathChanged { .. })
    ));
}

#[test]
fn reserved_bbolt_filenames_are_rejected_even_when_absent() {
    let directory = TestDirectory::new();
    for name in ["ptrack.db", "PTrAcK.Db", "global.db", "GlObAl.Db"] {
        let path = directory.path(name);
        assert!(matches!(
            Store::create_new(&path, StoreKind::Project),
            Err(StoreError::LegacyPathForbidden { .. })
        ));
        assert!(!path.exists());
    }
}

#[test]
fn wrong_kind_and_foreign_databases_are_rejected_without_mutation() {
    let (directory, project_path, store) = project_store();
    drop(store);
    let before = fs::read(&project_path).unwrap();

    assert!(matches!(
        Store::open_existing(&project_path, StoreKind::Global),
        Err(StoreError::WrongStoreKind { .. })
    ));
    assert_eq!(fs::read(&project_path).unwrap(), before);

    let foreign_path = directory.path("foreign.redb");
    drop(redb::Database::create(&foreign_path).unwrap());
    make_private(&foreign_path);
    let before = fs::read(&foreign_path).unwrap();
    assert!(matches!(
        Store::open_existing(&foreign_path, StoreKind::Project),
        Err(StoreError::InvalidManifest(_))
    ));
    assert_eq!(fs::read(foreign_path).unwrap(), before);
}

#[test]
fn newer_application_schema_is_rejected_without_mutation() {
    let (_directory, path, store) = project_store();
    drop(store);

    let database = redb::Database::open(&path).unwrap();
    let transaction = database.begin_write().unwrap();
    {
        let mut manifest = transaction.open_table(MANIFEST_TABLE).unwrap();
        manifest
            .insert(MANIFEST_KEY_SCHEMA_VERSION, 5_u32.to_be_bytes().as_slice())
            .unwrap();
        manifest
            .insert(b"future_key".as_slice(), b"future_value".as_slice())
            .unwrap();
    }
    transaction.commit().unwrap();
    drop(database);
    let before = fs::read(&path).unwrap();

    assert!(matches!(
        Store::open_existing(&path, StoreKind::Project),
        Err(StoreError::UnsupportedSchemaVersion {
            actual: 5,
            current: 4
        })
    ));
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn older_application_schema_is_rejected_without_mutation() {
    let (_directory, path, store) = project_store();
    drop(store);

    for version in [1, 2, 3] {
        set_schema_version(&path, version);
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            Store::open_existing(&path, StoreKind::Project),
            Err(StoreError::UnsupportedSchemaVersion {
                actual,
                current: 4
            }) if actual == version
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn malformed_application_schema_is_rejected_without_mutation() {
    let (_directory, path, store) = project_store();
    drop(store);

    let database = redb::Database::open(&path).unwrap();
    let transaction = database.begin_write().unwrap();
    {
        let mut manifest = transaction.open_table(MANIFEST_TABLE).unwrap();
        manifest
            .insert(MANIFEST_KEY_SCHEMA_VERSION, [0_u8; 3].as_slice())
            .unwrap();
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
fn non_redb_existing_file_is_probed_without_mutation() {
    let directory = TestDirectory::new();
    let path = directory.path("legacy-copy.redb");
    write_private_file(&path, b"not a redb; representative legacy bytes");
    let before = fs::read(&path).unwrap();

    assert!(Store::open_existing(&path, StoreKind::Project).is_err());
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn unexpected_tables_and_corrupt_record_envelopes_are_rejected() {
    const EXTRA: TableDefinition<&[u8], &[u8]> = TableDefinition::new("unexpected");
    let directory = TestDirectory::new();
    let catalog_path = directory.path("catalog.redb");
    drop(Store::create_new(&catalog_path, StoreKind::Project).unwrap());

    let database = redb::Database::open(&catalog_path).unwrap();
    let transaction = database.begin_write().unwrap();
    transaction.open_table(EXTRA).unwrap();
    transaction.commit().unwrap();
    drop(database);
    assert!(matches!(
        Store::open_existing(&catalog_path, StoreKind::Project),
        Err(StoreError::InvalidManifest(_))
    ));

    let record_path = directory.path("record.redb");
    drop(Store::create_new(&record_path, StoreKind::Project).unwrap());
    let database = redb::Database::open(&record_path).unwrap();
    let transaction = database.begin_write().unwrap();
    {
        let mut tasks = transaction.open_table(Collection::Tasks.table()).unwrap();
        tasks
            .insert(1_u64.to_be_bytes().as_slice(), b"bad".as_slice())
            .unwrap();
    }
    transaction.commit().unwrap();
    drop(database);
    assert!(matches!(
        Store::open_existing(record_path, StoreKind::Project),
        Err(StoreError::Envelope(_))
    ));
}

#[test]
fn second_writer_reports_busy_after_the_bounded_wait() {
    let (_directory, path, _owner) = project_store();
    let start = Instant::now();

    assert!(matches!(
        Store::open_existing(path, StoreKind::Project),
        Err(StoreError::Busy)
    ));
    assert!(start.elapsed() >= Duration::from_millis(900));
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[cfg(unix)]
#[test]
fn files_are_private_and_symlinks_or_insecure_files_are_rejected() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    let directory = TestDirectory::new();
    let path = directory.path("private.redb");
    drop(Store::create_new(&path, StoreKind::Project).unwrap());
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);

    // Leaked group/other bits — a git checkout, a copy under a default umask —
    // are healed by tightening on open, never refused: removing access cannot
    // leak anything, while failing closed here bricked every command and
    // crashed the desktop at launch (v0.24.0 field report).
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    drop(Store::open_existing(&path, StoreKind::Project).unwrap());
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);

    let link = directory.path("link.redb");
    symlink(&path, &link).unwrap();
    assert!(matches!(
        Store::create_new(link, StoreKind::Project),
        Err(StoreError::SymbolicLink { .. })
    ));
}

fn write_private_file(path: &Path, bytes: &[u8]) {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn set_schema_version(path: &Path, version: u32) {
    let database = redb::Database::open(path).unwrap();
    let transaction = database.begin_write().unwrap();
    {
        let mut manifest = transaction.open_table(MANIFEST_TABLE).unwrap();
        manifest
            .insert(
                MANIFEST_KEY_SCHEMA_VERSION,
                version.to_be_bytes().as_slice(),
            )
            .unwrap();
    }
    transaction.commit().unwrap();
    drop(database);
}

fn manifest_entries(path: &Path) -> BTreeMap<Vec<u8>, Vec<u8>> {
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

fn make_private(path: &Path) {
    protect_private_file(path).unwrap();
}
