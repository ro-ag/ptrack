use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use redb::{ReadableDatabase, TableDefinition};

use super::{Collection, OwnedRecordKey, RecordEnvelope, RecordKey, Store, StoreError, StoreKind};
use crate::schema::{MANIFEST_KEY_SCHEMA_VERSION, MANIFEST_TABLE};
use crate::store::{FileIdentity, ensure_path_identity};

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let number = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ptrack-store-test-{}-{number}", std::process::id()));
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

fn project_store() -> (TestDirectory, PathBuf, Store) {
    let directory = TestDirectory::new();
    let path = directory.path("project.redb");
    let store = Store::create_new(&path, StoreKind::Project).unwrap();
    (directory, path, store)
}

#[test]
fn committed_records_and_sequences_survive_reopen() {
    let (directory, path, store) = project_store();
    let plan = RecordEnvelope::new(1, 5, [0x00, 0xff, 0x80]);
    let replay = RecordEnvelope::new(77, 9, b"opaque gob bytes".as_slice());

    store
        .write(|transaction| {
            transaction.put(Collection::Plans, RecordKey::Id(7), &plan)?;
            transaction.advance_high_water(Collection::Plans, 12)?;
            assert_eq!(transaction.next_id(Collection::MemoryWritebacks)?, 1);
            transaction.put(
                Collection::MemoryWritebacks,
                RecordKey::Bytes(&[0, 0xff, 0]),
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
                vec![(OwnedRecordKey::Bytes(vec![0, 0xff, 0]), replay.clone())]
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
        transaction.put(
            Collection::Tasks,
            RecordKey::Id(1),
            &RecordEnvelope::new(1, 1, b"not committed".as_slice()),
        )?;
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
            transaction.put(
                Collection::Notes,
                RecordKey::Id(id),
                &RecordEnvelope::new(1, 1, b"not committed".as_slice()),
            )?;
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
fn swallowed_mutation_error_poisons_and_aborts_the_transaction() {
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

    let store = Store::open_existing(&path, StoreKind::Project).unwrap();
    let result = store.write(|transaction| {
        let error = transaction
            .put(
                Collection::Tasks,
                RecordKey::Id(1),
                &RecordEnvelope::new(1, 1, b"replacement".as_slice()),
            )
            .unwrap_err();
        assert!(matches!(error, StoreError::Envelope(_)));
        Ok(())
    });
    assert!(matches!(result, Err(StoreError::TransactionPoisoned)));
    drop(store);

    let database = redb::Database::open(&path).unwrap();
    let transaction = database.begin_read().unwrap();
    let tasks = transaction.open_table(Collection::Tasks.table()).unwrap();
    assert_eq!(
        tasks
            .get(1_u64.to_be_bytes().as_slice())
            .unwrap()
            .unwrap()
            .value(),
        corrupt_value
    );
}

#[test]
fn high_water_marks_preserve_deletions_gaps_and_overflow() {
    let (_directory, _path, store) = project_store();
    let record = RecordEnvelope::new(1, 1, b"record".as_slice());

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

#[test]
fn file_identity_detects_path_replacement() {
    let directory = TestDirectory::new();
    let path = directory.path("identity.redb");
    write_private_file(&path, b"first");
    let identity = FileIdentity::from_metadata(&fs::metadata(&path).unwrap());
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
            .insert(MANIFEST_KEY_SCHEMA_VERSION, 2_u32.to_be_bytes().as_slice())
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
            actual: 2,
            current: 1
        })
    ));
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn older_application_schema_is_rejected_without_mutation() {
    let (_directory, path, store) = project_store();
    drop(store);

    set_schema_version(&path, 0);
    let before = fs::read(&path).unwrap();
    assert!(matches!(
        Store::open_existing(&path, StoreKind::Project),
        Err(StoreError::UnsupportedSchemaVersion {
            actual: 0,
            current: 1
        })
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
    let store = Store::open_existing(record_path, StoreKind::Project).unwrap();
    assert!(matches!(
        store.read(|transaction| transaction.get(Collection::Tasks, RecordKey::Id(1))),
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

    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        Store::open_existing(&path, StoreKind::Project),
        Err(StoreError::InsecurePermissions { .. })
    ));

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

#[cfg(unix)]
fn make_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(not(unix))]
fn make_private(_path: &Path) {}
