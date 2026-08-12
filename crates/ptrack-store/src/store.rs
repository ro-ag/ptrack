use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use redb::{Database, Durability, ReadableDatabase, ReadableTable, StorageBackend, TableHandle};

use crate::schema::{
    MANIFEST_KEY_FAMILY, MANIFEST_KEY_OWNER, MANIFEST_KEY_SCHEMA_VERSION, MANIFEST_KEY_STATE,
    MANIFEST_KEY_STORE_KIND, MANIFEST_TABLE, SEQUENCES_TABLE, STORE_FAMILY, STORE_OWNER,
    STORE_SCHEMA_VERSION, STORE_STATE_READY, collections_for, decode_key,
};
use crate::{
    Collection, OwnedRecordKey, RecordEnvelope, RecordKey, StoreError, StoreKind, StoreResult,
};

const LOCK_TIMEOUT: Duration = Duration::from_secs(1);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(20);
const LEGACY_PROJECT_FILENAME: &str = "ptrack.db";
const LEGACY_GLOBAL_FILENAME: &str = "global.db";

/// An exclusively owned, versioned ptrack redb database.
///
/// The raw engine handle is deliberately private. Real paths are intended to
/// be supplied only by the ptrack storage or migration executable.
pub struct Store {
    database: Database,
    kind: StoreKind,
    path: PathBuf,
}

impl Store {
    /// Creates a new private database without replacing any existing path.
    ///
    /// Legacy bbolt filenames are always rejected so a Rust destination cannot
    /// accidentally occupy or replace the source database path.
    pub fn create_new(path: impl AsRef<Path>, kind: StoreKind) -> StoreResult<Self> {
        let path = path.as_ref();
        reject_legacy_path(path)?;
        ensure_destination_absent(path)?;

        let file = create_private_file(path)?;
        let identity = FileIdentity::from_metadata(&file.metadata()?);
        // Once create_new succeeds at the filesystem level, leave that exact
        // path in place on every later error. Unlinking by pathname cannot be
        // made race-free with portable std APIs and could delete a replacement.
        sync_parent_directory(path)?;
        let database = Database::builder().create_file(file)?;

        if let Err(error) = initialize_database(&database, kind) {
            drop(database);
            return Err(error);
        }
        ensure_path_identity(path, identity)?;
        sync_parent_directory(path)?;

        Ok(Self {
            database,
            kind,
            path: path.to_path_buf(),
        })
    }

    /// Opens an existing database after a read-only schema and ownership probe.
    ///
    /// This method never creates a file or performs an application-schema
    /// upgrade. Foreign, older, newer, and wrong-kind databases are rejected
    /// before a writable engine handle is acquired.
    pub fn open_existing(path: impl AsRef<Path>, expected: StoreKind) -> StoreResult<Self> {
        let path = path.as_ref();
        reject_legacy_path(path)?;
        // The file resolved by this exclusive open is authoritative. Avoid a
        // check-then-open window in which a pathname could be replaced.
        let (file, identity) = probe_existing_with_retry(path, expected)?;
        let database = open_writable_with_retry(path, identity, &file)?;
        ensure_path_identity(path, identity)?;
        validate_database(&database, expected)?;

        Ok(Self {
            database,
            kind: expected,
            path: path.to_path_buf(),
        })
    }

    /// Returns whether this is a project or global database.
    #[must_use]
    pub const fn kind(&self) -> StoreKind {
        self.kind
    }

    /// Returns the explicit database path supplied by its owning tool.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Runs a read against one consistent snapshot.
    pub fn read<R>(
        &self,
        operation: impl FnOnce(&ReadTransaction) -> StoreResult<R>,
    ) -> StoreResult<R> {
        let transaction = self.database.begin_read()?;
        let transaction = ReadTransaction {
            inner: transaction,
            kind: self.kind,
        };
        operation(&transaction)
    }

    /// Runs one immediate-durability transaction and commits only on success.
    ///
    /// Closure errors and panics explicitly abort the engine transaction, so
    /// record writes and sequence allocation cannot partially escape.
    pub fn write<R>(
        &self,
        operation: impl FnOnce(&mut WriteTransaction) -> StoreResult<R>,
    ) -> StoreResult<R> {
        let mut inner = self.database.begin_write()?;
        inner.set_durability(Durability::Immediate)?;
        // Persist allocator state with every commit. Besides faster crash
        // recovery, this keeps the next open eligible for the side-effect-free
        // read-only manifest probe that protects foreign/newer databases.
        inner.set_quick_repair(true);
        let mut transaction = WriteTransaction {
            inner: Some(inner),
            kind: self.kind,
            poisoned: false,
        };

        match catch_unwind(AssertUnwindSafe(|| operation(&mut transaction))) {
            Ok(Ok(value)) => {
                if transaction.poisoned {
                    transaction.abort()?;
                    return Err(StoreError::TransactionPoisoned);
                }
                transaction.commit()?;
                Ok(value)
            }
            Ok(Err(error)) => {
                transaction.abort()?;
                Err(error)
            }
            Err(payload) => {
                let _ = transaction.abort();
                resume_unwind(payload)
            }
        }
    }
}

/// A read-only ptrack transaction with no access to raw engine tables.
pub struct ReadTransaction {
    inner: redb::ReadTransaction,
    kind: StoreKind,
}

impl ReadTransaction {
    /// Reads and strictly decodes one record envelope.
    pub fn get(
        &self,
        collection: Collection,
        key: RecordKey<'_>,
    ) -> StoreResult<Option<RecordEnvelope>> {
        collection.validate_store(self.kind)?;
        let encoded_key = key.encode(collection)?;
        let table = self.inner.open_table(collection.table())?;
        let value = table
            .get(encoded_key.as_slice())?
            .map(|guard| guard.value().to_vec());
        value
            .map(|encoded| RecordEnvelope::decode(&encoded).map_err(StoreError::from))
            .transpose()
    }

    /// Scans a collection in stable key order and returns owned data.
    pub fn scan(
        &self,
        collection: Collection,
    ) -> StoreResult<Vec<(OwnedRecordKey, RecordEnvelope)>> {
        collection.validate_store(self.kind)?;
        let table = self.inner.open_table(collection.table())?;
        let mut records = Vec::new();
        for entry in table.iter()? {
            let (key, value) = entry?;
            records.push((
                decode_key(collection, key.value())?,
                RecordEnvelope::decode(value.value())?,
            ));
        }
        Ok(records)
    }

    /// Reads the collection's independent persisted high-water mark.
    pub fn sequence_high_water(&self, collection: Collection) -> StoreResult<u64> {
        collection.validate_store(self.kind)?;
        require_sequence(collection)?;
        let table = self.inner.open_table(SEQUENCES_TABLE)?;
        decode_sequence(
            collection,
            table
                .get(collection.name().as_bytes())?
                .map(|guard| guard.value().to_vec()),
        )
    }
}

/// A ptrack write transaction whose commit and durability are owned by `Store`.
pub struct WriteTransaction {
    inner: Option<redb::WriteTransaction>,
    kind: StoreKind,
    poisoned: bool,
}

impl WriteTransaction {
    fn raw(&self) -> &redb::WriteTransaction {
        self.inner
            .as_ref()
            .expect("completed ptrack transactions are not exposed")
    }

    fn commit(&mut self) -> StoreResult<()> {
        self.inner
            .take()
            .expect("ptrack transaction must be active")
            .commit()?;
        Ok(())
    }

    fn abort(&mut self) -> StoreResult<()> {
        self.inner
            .take()
            .expect("ptrack transaction must be active")
            .abort()?;
        Ok(())
    }

    /// Reads one record, including writes already made by this transaction.
    pub fn get(
        &self,
        collection: Collection,
        key: RecordKey<'_>,
    ) -> StoreResult<Option<RecordEnvelope>> {
        collection.validate_store(self.kind)?;
        let encoded_key = key.encode(collection)?;
        let table = self.raw().open_table(collection.table())?;
        let value = table
            .get(encoded_key.as_slice())?
            .map(|guard| guard.value().to_vec());
        value
            .map(|encoded| RecordEnvelope::decode(&encoded).map_err(StoreError::from))
            .transpose()
    }

    /// Inserts or replaces a record and returns the previous envelope.
    ///
    /// Numeric record IDs advance their collection's high-water mark in this
    /// same transaction. Deletion never lowers it.
    pub fn put(
        &mut self,
        collection: Collection,
        key: RecordKey<'_>,
        record: &RecordEnvelope,
    ) -> StoreResult<Option<RecordEnvelope>> {
        let result = self.put_inner(collection, key, record);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn put_inner(
        &mut self,
        collection: Collection,
        key: RecordKey<'_>,
        record: &RecordEnvelope,
    ) -> StoreResult<Option<RecordEnvelope>> {
        collection.validate_store(self.kind)?;
        let numeric_id = match key {
            RecordKey::Id(id) => Some(id),
            _ => None,
        };
        let encoded_key = key.encode(collection)?;
        let encoded_record = record.encode();
        let old = {
            let mut table = self.raw().open_table(collection.table())?;
            table
                .insert(encoded_key.as_slice(), encoded_record.as_slice())?
                .map(|guard| guard.value().to_vec())
        };

        if let Some(id) = numeric_id {
            let current = self.sequence_high_water(collection)?;
            if id > current {
                self.advance_high_water(collection, id)?;
            }
        }

        old.map(|encoded| RecordEnvelope::decode(&encoded).map_err(StoreError::from))
            .transpose()
    }

    /// Deletes a record without changing its collection's sequence.
    pub fn delete(
        &mut self,
        collection: Collection,
        key: RecordKey<'_>,
    ) -> StoreResult<Option<RecordEnvelope>> {
        let result = self.delete_inner(collection, key);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn delete_inner(
        &mut self,
        collection: Collection,
        key: RecordKey<'_>,
    ) -> StoreResult<Option<RecordEnvelope>> {
        collection.validate_store(self.kind)?;
        let encoded_key = key.encode(collection)?;
        let old = {
            let mut table = self.raw().open_table(collection.table())?;
            table
                .remove(encoded_key.as_slice())?
                .map(|guard| guard.value().to_vec())
        };
        old.map(|encoded| RecordEnvelope::decode(&encoded).map_err(StoreError::from))
            .transpose()
    }

    /// Scans a collection, including writes made by this transaction.
    pub fn scan(
        &self,
        collection: Collection,
    ) -> StoreResult<Vec<(OwnedRecordKey, RecordEnvelope)>> {
        collection.validate_store(self.kind)?;
        let table = self.raw().open_table(collection.table())?;
        let mut records = Vec::new();
        for entry in table.iter()? {
            let (key, value) = entry?;
            records.push((
                decode_key(collection, key.value())?,
                RecordEnvelope::decode(value.value())?,
            ));
        }
        Ok(records)
    }

    /// Reads the collection's high-water mark inside this transaction.
    pub fn sequence_high_water(&self, collection: Collection) -> StoreResult<u64> {
        collection.validate_store(self.kind)?;
        require_sequence(collection)?;
        let table = self.raw().open_table(SEQUENCES_TABLE)?;
        decode_sequence(
            collection,
            table
                .get(collection.name().as_bytes())?
                .map(|guard| guard.value().to_vec()),
        )
    }

    /// Allocates the next sequence value without inserting a record.
    ///
    /// This supports memory-writeback records, whose persisted key is a request
    /// ID while their independent bbolt sequence is retained in the payload.
    pub fn next_id(&mut self, collection: Collection) -> StoreResult<u64> {
        let result = self.next_id_inner(collection);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn next_id_inner(&mut self, collection: Collection) -> StoreResult<u64> {
        let current = self.sequence_high_water(collection)?;
        let next = current.checked_add(1).ok_or(StoreError::SequenceOverflow {
            collection: collection.name(),
        })?;
        self.advance_high_water(collection, next)?;
        Ok(next)
    }

    /// Advances, but never resets or decreases, a collection high-water mark.
    pub fn advance_high_water(
        &mut self,
        collection: Collection,
        requested: u64,
    ) -> StoreResult<()> {
        let result = self.advance_high_water_inner(collection, requested);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn advance_high_water_inner(
        &mut self,
        collection: Collection,
        requested: u64,
    ) -> StoreResult<()> {
        collection.validate_store(self.kind)?;
        require_sequence(collection)?;
        let current = self.sequence_high_water(collection)?;
        if requested < current {
            return Err(StoreError::SequenceWouldDecrease {
                collection: collection.name(),
                current,
                requested,
            });
        }
        if requested == current {
            return Ok(());
        }

        let mut table = self.raw().open_table(SEQUENCES_TABLE)?;
        table.insert(
            collection.name().as_bytes(),
            requested.to_be_bytes().as_slice(),
        )?;
        Ok(())
    }
}

fn initialize_database(database: &Database, kind: StoreKind) -> StoreResult<()> {
    let mut transaction = database.begin_write()?;
    transaction.set_durability(Durability::Immediate)?;
    transaction.set_quick_repair(true);

    {
        let mut manifest = transaction.open_table(MANIFEST_TABLE)?;
        manifest.insert(MANIFEST_KEY_FAMILY, STORE_FAMILY)?;
        manifest.insert(MANIFEST_KEY_OWNER, STORE_OWNER)?;
        manifest.insert(
            MANIFEST_KEY_SCHEMA_VERSION,
            STORE_SCHEMA_VERSION.to_be_bytes().as_slice(),
        )?;
        manifest.insert(MANIFEST_KEY_STATE, STORE_STATE_READY)?;
        manifest.insert(MANIFEST_KEY_STORE_KIND, kind.as_bytes())?;
    }
    transaction.open_table(SEQUENCES_TABLE)?;
    for collection in collections_for(kind) {
        transaction.open_table(collection.table())?;
    }
    transaction.commit()?;
    Ok(())
}

fn validate_database(
    database: &impl ReadableDatabase,
    expected_kind: StoreKind,
) -> StoreResult<()> {
    let transaction = database.begin_read()?;
    let manifest = transaction.open_table(MANIFEST_TABLE).map_err(|error| {
        StoreError::InvalidManifest(format!("schema table is unavailable: {error}"))
    })?;

    let mut entries = BTreeMap::new();
    for entry in manifest.iter()? {
        let (key, value) = entry?;
        entries.insert(key.value().to_vec(), value.value().to_vec());
    }

    require_manifest_value(&entries, MANIFEST_KEY_FAMILY, STORE_FAMILY, "family")?;

    let version_bytes = entries
        .get(MANIFEST_KEY_SCHEMA_VERSION)
        .ok_or_else(|| StoreError::InvalidManifest("schema version is missing".to_owned()))?;
    let actual_version = u32::from_be_bytes(version_bytes.as_slice().try_into().map_err(|_| {
        StoreError::InvalidManifest("schema version must contain exactly four bytes".to_owned())
    })?);
    if actual_version != STORE_SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchemaVersion {
            actual: actual_version,
            current: STORE_SCHEMA_VERSION,
        });
    }

    let expected_manifest_keys = BTreeSet::from([
        MANIFEST_KEY_FAMILY.to_vec(),
        MANIFEST_KEY_OWNER.to_vec(),
        MANIFEST_KEY_SCHEMA_VERSION.to_vec(),
        MANIFEST_KEY_STATE.to_vec(),
        MANIFEST_KEY_STORE_KIND.to_vec(),
    ]);
    let actual_manifest_keys = entries.keys().cloned().collect::<BTreeSet<_>>();
    if actual_manifest_keys != expected_manifest_keys {
        return Err(StoreError::InvalidManifest(
            "schema metadata keys do not match the version-1 contract".to_owned(),
        ));
    }

    require_manifest_value(&entries, MANIFEST_KEY_OWNER, STORE_OWNER, "owner")?;
    require_manifest_value(&entries, MANIFEST_KEY_STATE, STORE_STATE_READY, "state")?;

    let actual_kind = StoreKind::from_bytes(
        entries
            .get(MANIFEST_KEY_STORE_KIND)
            .expect("the exact current-version manifest key set was checked"),
    )?;
    if actual_kind != expected_kind {
        return Err(StoreError::WrongStoreKind {
            expected: expected_kind,
            actual: actual_kind,
        });
    }

    drop(manifest);

    let expected_tables = expected_table_names(expected_kind);
    let actual_tables = transaction
        .list_tables()?
        .map(|handle| handle.name().to_owned())
        .collect::<BTreeSet<_>>();
    if actual_tables != expected_tables {
        return Err(StoreError::InvalidManifest(
            "database table catalog does not match its declared store kind".to_owned(),
        ));
    }
    if transaction.list_multimap_tables()?.next().is_some() {
        return Err(StoreError::InvalidManifest(
            "multimap tables are not part of the ptrack schema".to_owned(),
        ));
    }

    transaction.open_table(SEQUENCES_TABLE)?;
    for collection in collections_for(expected_kind) {
        transaction.open_table(collection.table())?;
    }
    Ok(())
}

fn expected_table_names(kind: StoreKind) -> BTreeSet<String> {
    let mut names = BTreeSet::from([
        MANIFEST_TABLE.name().to_owned(),
        SEQUENCES_TABLE.name().to_owned(),
    ]);
    names.extend(collections_for(kind).map(|collection| collection.table().name().to_owned()));
    names
}

fn require_manifest_value(
    entries: &BTreeMap<Vec<u8>, Vec<u8>>,
    key: &[u8],
    expected: &[u8],
    label: &str,
) -> StoreResult<()> {
    let actual = entries
        .get(key)
        .ok_or_else(|| StoreError::InvalidManifest(format!("{label} is missing")))?;
    if actual == expected {
        Ok(())
    } else {
        Err(StoreError::InvalidManifest(format!(
            "unexpected {label} value"
        )))
    }
}

fn require_sequence(collection: Collection) -> StoreResult<()> {
    if collection.is_sequenced() {
        Ok(())
    } else {
        Err(StoreError::SequenceNotSupported {
            collection: collection.name(),
        })
    }
}

fn decode_sequence(collection: Collection, encoded: Option<Vec<u8>>) -> StoreResult<u64> {
    match encoded {
        None => Ok(0),
        Some(encoded) => Ok(u64::from_be_bytes(encoded.as_slice().try_into().map_err(
            |_| {
                StoreError::InvalidManifest(format!(
                    "collection {} has a malformed sequence",
                    collection.name()
                ))
            },
        )?)),
    }
}

fn reject_legacy_path(path: &Path) -> StoreResult<()> {
    match path.file_name() {
        Some(name)
            if name.to_str().is_some_and(|name| {
                name.eq_ignore_ascii_case(LEGACY_PROJECT_FILENAME)
                    || name.eq_ignore_ascii_case(LEGACY_GLOBAL_FILENAME)
            }) =>
        {
            Err(StoreError::LegacyPathForbidden {
                path: path.to_path_buf(),
            })
        }
        _ => Ok(()),
    }
}

fn ensure_destination_absent(path: &Path) -> StoreResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(StoreError::SymbolicLink {
            path: path.to_path_buf(),
        }),
        Ok(_) => Err(StoreError::DestinationExists {
            path: path.to_path_buf(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn validate_existing_path(path: &Path) -> StoreResult<FileIdentity> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(StoreError::SymbolicLink {
            path: path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(StoreError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    validate_private_permissions(path, &metadata)?;
    Ok(FileIdentity::from_metadata(&metadata))
}

fn create_private_file(path: &Path) -> StoreResult<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        options.share_mode(0);
    }

    match options.open(path) {
        Ok(file) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            Ok(file)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(StoreError::DestinationExists {
                path: path.to_path_buf(),
            })
        }
        Err(error) => Err(error.into()),
    }
}

fn probe_existing_with_retry(path: &Path, kind: StoreKind) -> StoreResult<(File, FileIdentity)> {
    let start = Instant::now();
    loop {
        let file = match open_existing_file(path) {
            Ok(file) => file,
            Err(StoreError::Io(error))
                if error.kind() == io::ErrorKind::PermissionDenied
                    && start.elapsed() < LOCK_TIMEOUT =>
            {
                thread::sleep(LOCK_RETRY_INTERVAL);
                continue;
            }
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        validate_private_permissions(path, &metadata)?;
        let expected = FileIdentity::from_metadata(&metadata);
        validate_opened_path(path, expected)?;
        ensure_path_identity(path, expected)?;

        match file.try_lock_shared() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) if start.elapsed() < LOCK_TIMEOUT => {
                thread::sleep(LOCK_RETRY_INTERVAL);
                continue;
            }
            Err(TryLockError::WouldBlock) => return Err(StoreError::Busy),
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        let snapshot = read_file_snapshot(&file)?;
        let database =
            Database::builder().create_with_backend(MemoryProbeBackend::new(snapshot))?;
        validate_database(&database, kind)?;
        drop(database);
        ensure_path_identity(path, expected)?;
        return Ok((file, expected));
    }
}

fn open_writable_with_retry(
    path: &Path,
    expected: FileIdentity,
    file: &File,
) -> StoreResult<Database> {
    let start = Instant::now();
    loop {
        let file_metadata = file.metadata()?;
        validate_private_permissions(path, &file_metadata)?;
        if FileIdentity::from_metadata(&file_metadata) != expected {
            return Err(StoreError::PathChanged {
                path: path.to_path_buf(),
            });
        }
        ensure_path_identity(path, expected)?;
        match Database::builder().create_file(file.try_clone()?) {
            Ok(database) => {
                ensure_path_identity(path, expected)?;
                return Ok(database);
            }
            Err(redb::DatabaseError::DatabaseAlreadyOpen) if start.elapsed() < LOCK_TIMEOUT => {
                thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// A descriptor-bound redb probe whose repairs and shutdown writes stay in RAM.
#[derive(Debug)]
struct MemoryProbeBackend {
    bytes: Mutex<Vec<u8>>,
}

impl MemoryProbeBackend {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: Mutex::new(bytes),
        }
    }

    fn lock(&self) -> io::Result<std::sync::MutexGuard<'_, Vec<u8>>> {
        self.bytes
            .lock()
            .map_err(|_| io::Error::other("ptrack in-memory manifest probe lock was poisoned"))
    }
}

impl StorageBackend for MemoryProbeBackend {
    fn len(&self) -> io::Result<u64> {
        u64::try_from(self.lock()?.len())
            .map_err(|_| io::Error::other("database snapshot length cannot be represented as u64"))
    }

    fn read(&self, offset: u64, output: &mut [u8]) -> io::Result<()> {
        let offset = usize::try_from(offset).map_err(|_| {
            io::Error::new(io::ErrorKind::UnexpectedEof, "snapshot offset overflow")
        })?;
        let end = offset.checked_add(output.len()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::UnexpectedEof, "snapshot range overflow")
        })?;
        let bytes = self.lock()?;
        let source = bytes.get(offset..end).ok_or_else(|| {
            io::Error::new(io::ErrorKind::UnexpectedEof, "snapshot read past end")
        })?;
        output.copy_from_slice(source);
        Ok(())
    }

    fn set_len(&self, length: u64) -> io::Result<()> {
        let length = usize::try_from(length)
            .map_err(|_| io::Error::new(io::ErrorKind::OutOfMemory, "snapshot length overflow"))?;
        self.lock()?.resize(length, 0);
        Ok(())
    }

    fn sync_data(&self) -> io::Result<()> {
        Ok(())
    }

    fn write(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        let offset = usize::try_from(offset)
            .map_err(|_| io::Error::new(io::ErrorKind::WriteZero, "snapshot offset overflow"))?;
        let end = offset
            .checked_add(data.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::WriteZero, "snapshot range overflow"))?;
        let mut bytes = self.lock()?;
        if end > bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "snapshot write past allocated end",
            ));
        }
        bytes[offset..end].copy_from_slice(data);
        Ok(())
    }
}

fn read_file_snapshot(file: &File) -> StoreResult<Vec<u8>> {
    let length = usize::try_from(file.metadata()?.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::OutOfMemory,
            "database is too large to validate safely",
        )
    })?;
    let mut snapshot = vec![0; length];

    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;

        file.read_exact_at(&mut snapshot, 0)?;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;

        let mut offset = 0_u64;
        let mut position = 0;
        while position < snapshot.len() {
            let read = file.seek_read(&mut snapshot[position..], offset)?;
            if read == 0 {
                return Err(StoreError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "database changed while its snapshot was read",
                )));
            }
            position += read;
            offset += read as u64;
        }
    }

    Ok(snapshot)
}

fn open_existing_file(path: &Path) -> StoreResult<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        // Prevent rename/delete replacement while the database handle exists.
        options.share_mode(0);
    }
    Ok(options.open(path)?)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl FileIdentity {
    pub(crate) fn from_metadata(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            Self {}
        }
    }
}

fn validate_opened_path(path: &Path, opened: FileIdentity) -> StoreResult<()> {
    let path_identity = validate_existing_path(path)?;
    #[cfg(unix)]
    if path_identity != opened {
        return Err(StoreError::PathChanged {
            path: path.to_path_buf(),
        });
    }
    #[cfg(not(unix))]
    let _ = (path_identity, opened);
    Ok(())
}

pub(crate) fn ensure_path_identity(path: &Path, expected: FileIdentity) -> StoreResult<()> {
    let metadata = fs::symlink_metadata(path)?;
    let invalid_type = metadata.file_type().is_symlink() || !metadata.is_file();
    #[cfg(unix)]
    let wrong_identity = FileIdentity::from_metadata(&metadata) != expected;
    #[cfg(not(unix))]
    let wrong_identity = {
        let _ = expected;
        false
    };
    if invalid_type || wrong_identity {
        Err(StoreError::PathChanged {
            path: path.to_path_buf(),
        })
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> StoreResult<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> StoreResult<()> {
    Ok(())
}

#[cfg(unix)]
fn validate_private_permissions(path: &Path, metadata: &fs::Metadata) -> StoreResult<()> {
    use std::os::unix::fs::MetadataExt;

    let mode = metadata.mode() & 0o777;
    if mode & 0o077 == 0 {
        Ok(())
    } else {
        Err(StoreError::InsecurePermissions {
            path: path.to_path_buf(),
            mode,
        })
    }
}

#[cfg(not(unix))]
fn validate_private_permissions(_path: &Path, _metadata: &fs::Metadata) -> StoreResult<()> {
    Ok(())
}
