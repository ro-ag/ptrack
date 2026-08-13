use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use redb::{
    Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata, StorageBackend,
    TableHandle,
};

use crate::import::MAX_LEGACY_PROJECT_FORMAT;
use crate::schema::{
    MANIFEST_KEY_ACTIVATION_GENERATION, MANIFEST_KEY_APPLICATION_WRITES,
    MANIFEST_KEY_BATCH_MANIFEST_SHA256, MANIFEST_KEY_CANONICAL_PATH, MANIFEST_KEY_DATABASE_ID,
    MANIFEST_KEY_DATABASE_JSON_SHA256, MANIFEST_KEY_FAMILY, MANIFEST_KEY_IMPORT_BUNDLE_SHA256,
    MANIFEST_KEY_IMPORT_BUNDLE_VERSION, MANIFEST_KEY_IMPORT_SOURCE_FORMAT, MANIFEST_KEY_ORIGIN,
    MANIFEST_KEY_OWNER, MANIFEST_KEY_QUARANTINE_COUNT, MANIFEST_KEY_SCHEMA_VERSION,
    MANIFEST_KEY_SOURCE_FORMAT, MANIFEST_KEY_STAGE_VERSION, MANIFEST_KEY_STATE,
    MANIFEST_KEY_STORE_KIND, MANIFEST_TABLE, QUARANTINE_TABLE, SEQUENCES_TABLE, STORE_FAMILY,
    STORE_ORIGIN_CREATED, STORE_ORIGIN_IMPORTED, STORE_ORIGIN_JSON_STAGE, STORE_OWNER,
    STORE_SCHEMA_VERSION, STORE_STATE_ACTIVE, STORE_STATE_IMPORTING, STORE_STATE_READY,
    collections_for, decode_key,
};
use crate::{
    Collection, IMPORT_BUNDLE_VERSION, ImportData, ImportReport, JSON_STAGE_VERSION,
    JsonStageImportData, JsonStageProvenance, OwnedRecordKey, RecordEnvelope, RecordKey,
    StoreError, StoreKind, StoreResult,
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
    identity: FileIdentity,
}

impl Store {
    /// Creates a new private database without replacing any existing path.
    ///
    /// Legacy bbolt filenames are always rejected so a Rust destination cannot
    /// accidentally occupy or replace the source database path.
    pub fn create_new(path: impl AsRef<Path>, kind: StoreKind) -> StoreResult<Self> {
        Self::create_new_inner(path.as_ref(), kind, || Ok(()), || Ok(()))
    }

    pub(crate) fn create_new_inner(
        path: &Path,
        kind: StoreKind,
        before_create: impl FnOnce() -> StoreResult<()>,
        after_create: impl FnOnce() -> StoreResult<()>,
    ) -> StoreResult<Self> {
        reject_legacy_path(path)?;
        let parent = DestinationParent::capture(path)?;
        ensure_destination_absent(path)?;
        before_create()?;
        parent.ensure_current()?;
        ensure_destination_absent(path)?;

        let file = create_private_file(path)?;
        let identity = FileIdentity::from_file(&file)?;
        // Once create_new succeeds at the filesystem level, leave that exact
        // path in place on every later error. Unlinking by pathname cannot be
        // made race-free with portable std APIs and could delete a replacement.
        after_create()?;
        parent.ensure_destination(path, identity)?;
        parent.sync()?;
        parent.ensure_destination(path, identity)?;
        let database = Database::builder().create_file(file)?;

        if let Err(error) =
            initialize_database(&database, kind, STORE_STATE_READY, STORE_ORIGIN_CREATED)
        {
            drop(database);
            return Err(error);
        }
        parent.ensure_destination(path, identity)?;
        parent.sync()?;
        parent.ensure_destination(path, identity)?;

        Ok(Self {
            database,
            kind,
            path: path.to_path_buf(),
            identity,
        })
    }

    /// Imports one complete legacy database into a new destination.
    ///
    /// All input is bounded and validated before the destination is created.
    /// Once created, its manifest remains `importing` until records, exact
    /// sequences, and post-write verification commit atomically with `ready`.
    /// Failures never unlink the artifact and normal opens reject it.
    pub fn import_new(
        path: impl AsRef<Path>,
        data: ImportData,
    ) -> StoreResult<(Self, ImportReport)> {
        Self::import_new_inner(
            path.as_ref(),
            data,
            || Ok(()),
            || Ok(()),
            || Ok(()),
            || Ok(()),
        )
    }

    pub(crate) fn import_new_inner(
        path: &Path,
        data: ImportData,
        before_create: impl FnOnce() -> StoreResult<()>,
        after_create: impl FnOnce() -> StoreResult<()>,
        before_ready: impl FnOnce() -> StoreResult<()>,
        after_ready: impl FnOnce() -> StoreResult<()>,
    ) -> StoreResult<(Self, ImportReport)> {
        let import = data.validate()?;
        reject_legacy_path(path)?;
        let parent = DestinationParent::capture(path)?;
        ensure_destination_absent(path)?;
        before_create()?;
        parent.ensure_current()?;
        ensure_destination_absent(path)?;

        let file = create_private_file(path)?;
        let identity = FileIdentity::from_file(&file)?;
        after_create()?;
        parent.ensure_destination(path, identity)?;
        parent.sync()?;
        parent.ensure_destination(path, identity)?;
        let database = Database::builder().create_file(file)?;
        initialize_database(
            &database,
            import.data.kind,
            STORE_STATE_IMPORTING,
            STORE_ORIGIN_IMPORTED,
        )?;
        parent.ensure_destination(path, identity)?;
        parent.sync()?;
        parent.ensure_destination(path, identity)?;

        if let Err(error) = crate::import::write_import(&database, &import, path, || {
            before_ready()?;
            parent.ensure_destination(path, identity)
        }) {
            drop(database);
            return Err(error);
        }
        if let Err(error) = after_ready() {
            ensure_committed_import_destination(&parent, path, identity)?;
            return Err(committed_import_verification_error(path, error));
        }
        ensure_committed_import_destination(&parent, path, identity)?;
        parent
            .sync()
            .map_err(|error| committed_import_verification_error(path, error))?;
        ensure_committed_import_destination(&parent, path, identity)?;
        validate_database(&database, import.data.kind)
            .map_err(|error| committed_import_verification_error(path, error))?;
        ensure_committed_import_destination(&parent, path, identity)?;

        let store = Self {
            database,
            kind: import.data.kind,
            path: path.to_path_buf(),
            identity,
        };
        Ok((store, import.report))
    }

    /// Creates one verified candidate from a standalone canonical JSON stage.
    ///
    /// Quarantined legacy capability bytes are committed to a private inert
    /// table and are never available through ordinary collection APIs.
    pub fn import_json_stage_new(
        path: impl AsRef<Path>,
        data: JsonStageImportData,
    ) -> StoreResult<(Self, ImportReport)> {
        Self::import_json_stage_new_inner(path.as_ref(), data, || Ok(()))
    }

    pub(crate) fn import_json_stage_new_inner(
        path: &Path,
        data: JsonStageImportData,
        before_ready: impl FnOnce() -> StoreResult<()>,
    ) -> StoreResult<(Self, ImportReport)> {
        let import = data.validate()?;
        reject_legacy_path(path)?;
        let parent = DestinationParent::capture(path)?;
        ensure_destination_absent(path)?;
        parent.ensure_current()?;
        ensure_destination_absent(path)?;

        let file = create_private_file(path)?;
        let identity = FileIdentity::from_file(&file)?;
        parent.ensure_destination(path, identity)?;
        parent.sync()?;
        parent.ensure_destination(path, identity)?;
        let database = Database::builder().create_file(file)?;
        initialize_database(
            &database,
            import.data.kind,
            STORE_STATE_IMPORTING,
            STORE_ORIGIN_JSON_STAGE,
        )?;
        parent.ensure_destination(path, identity)?;
        parent.sync()?;
        parent.ensure_destination(path, identity)?;

        if let Err(error) = crate::import::write_json_stage_import(&database, &import, path, || {
            before_ready()?;
            parent.ensure_destination(path, identity)
        }) {
            drop(database);
            return Err(error);
        }
        ensure_committed_import_destination(&parent, path, identity)?;
        parent
            .sync()
            .map_err(|error| committed_import_verification_error(path, error))?;
        ensure_committed_import_destination(&parent, path, identity)?;
        validate_database(&database, import.data.kind)
            .map_err(|error| committed_import_verification_error(path, error))?;
        ensure_committed_import_destination(&parent, path, identity)?;

        let store = Self {
            database,
            kind: import.data.kind,
            path: path.to_path_buf(),
            identity,
        };
        Ok((store, import.report))
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
            identity,
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

    /// Returns attested standalone-stage provenance without exposing quarantine bytes.
    pub fn json_stage_provenance(&self) -> StoreResult<Option<JsonStageProvenance>> {
        let transaction = self.database.begin_read()?;
        let entries = read_manifest_entries(&transaction)?;
        json_stage_provenance(&entries)
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
        if self.json_stage_provenance()?.is_some() {
            return Err(StoreError::InvalidImport(
                "JSON-stage databases are immutable through the raw store API; activated application writes require a typed store"
                    .to_owned(),
            ));
        }
        self.write_inner(false, operation)
    }

    pub(crate) fn write_application<R>(
        &self,
        expected: &crate::ActiveBinding,
        operation: impl FnOnce(&mut WriteTransaction) -> StoreResult<R>,
    ) -> StoreResult<R> {
        ensure_path_identity(&self.path, self.identity)?;
        crate::activation::validate_binding_for_path(expected, self.kind, &self.path)?;
        if self.active_binding()?.as_ref() != Some(expected) {
            return Err(StoreError::ActivationBinding(
                "stored binding does not match the active runtime".to_owned(),
            ));
        }
        let result = self.write_inner(true, operation)?;
        ensure_path_identity(&self.path, self.identity)?;
        Ok(result)
    }

    pub(crate) fn write_activation<R>(
        &self,
        expected: &crate::ActiveBinding,
        operation: impl FnOnce(&mut WriteTransaction) -> StoreResult<R>,
    ) -> StoreResult<R> {
        ensure_path_identity(&self.path, self.identity)?;
        crate::activation::validate_binding_for_path(expected, self.kind, &self.path)?;
        if self.active_binding()?.as_ref() != Some(expected) {
            return Err(StoreError::ActivationBinding(
                "stored binding does not match the active runtime".to_owned(),
            ));
        }
        let result = self.write_inner(false, operation)?;
        ensure_path_identity(&self.path, self.identity)?;
        Ok(result)
    }

    fn write_inner<R>(
        &self,
        application_write: bool,
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

        if application_write {
            let mut manifest = transaction.raw().open_table(MANIFEST_TABLE)?;
            manifest.insert(MANIFEST_KEY_APPLICATION_WRITES, b"true".as_slice())?;
        }

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

    pub(crate) fn active_binding(&self) -> StoreResult<Option<crate::ActiveBinding>> {
        let transaction = self.database.begin_read()?;
        let entries = read_manifest_entries(&transaction)?;
        crate::activation::binding_from_manifest(&entries)
    }

    pub(crate) fn application_writes(&self) -> StoreResult<bool> {
        let transaction = self.database.begin_read()?;
        let entries = read_manifest_entries(&transaction)?;
        match entries
            .get(MANIFEST_KEY_APPLICATION_WRITES)
            .map(Vec::as_slice)
        {
            None | Some(b"false") => Ok(false),
            Some(b"true") => Ok(true),
            Some(_) => Err(StoreError::InvalidManifest(
                "application_writes must be true or false".to_owned(),
            )),
        }
    }

    pub(crate) fn activate(&self, binding: &crate::ActiveBinding) -> StoreResult<()> {
        crate::activation::validate_binding_for_path(binding, self.kind, &self.path)?;
        let current = self.active_binding()?;
        if let Some(current) = current {
            return if current == *binding {
                Ok(())
            } else {
                Err(StoreError::ActivationBinding(
                    "store is already bound to another generation".to_owned(),
                ))
            };
        }
        let mut transaction = self.database.begin_write()?;
        transaction.set_durability(Durability::Immediate)?;
        transaction.set_quick_repair(true);
        {
            let mut manifest = transaction.open_table(MANIFEST_TABLE)?;
            manifest.insert(MANIFEST_KEY_STATE, STORE_STATE_ACTIVE)?;
            manifest.insert(
                MANIFEST_KEY_ACTIVATION_GENERATION,
                binding.generation.to_be_bytes().as_slice(),
            )?;
            manifest.insert(MANIFEST_KEY_DATABASE_ID, binding.database_id.as_bytes())?;
            manifest.insert(
                MANIFEST_KEY_CANONICAL_PATH,
                binding.canonical_path.as_os_str().as_encoded_bytes(),
            )?;
            manifest.insert(MANIFEST_KEY_APPLICATION_WRITES, b"false".as_slice())?;
        }
        transaction.commit()?;
        validate_database(&self.database, self.kind)
    }

    pub(crate) fn with_writer_barrier<R>(
        &self,
        operation: impl FnOnce(&Path) -> StoreResult<R>,
    ) -> StoreResult<R> {
        let transaction = self.database.begin_write()?;
        let result = operation(&self.path);
        transaction.abort()?;
        result
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

    /// Returns a collection's exact row count without decoding its values.
    pub fn collection_len(&self, collection: Collection) -> StoreResult<usize> {
        collection.validate_store(self.kind)?;
        let table = self.inner.open_table(collection.table())?;
        usize::try_from(table.len()?).map_err(|_| {
            StoreError::InvalidManifest("collection length does not fit usize".to_owned())
        })
    }

    /// Decodes at most `limit` rows from one end of a collection.
    pub fn scan_limited(
        &self,
        collection: Collection,
        limit: usize,
        newest_first: bool,
    ) -> StoreResult<Vec<(OwnedRecordKey, RecordEnvelope)>> {
        collection.validate_store(self.kind)?;
        let table = self.inner.open_table(collection.table())?;
        let mut iterator = table.iter()?;
        let mut records = Vec::with_capacity(limit);
        while records.len() < limit {
            let entry = if newest_first {
                iterator.next_back()
            } else {
                iterator.next()
            };
            let Some(entry) = entry else { break };
            let (key, value) = entry?;
            records.push((
                decode_key(collection, key.value())?,
                RecordEnvelope::decode(value.value())?,
            ));
        }
        Ok(records)
    }

    /// Visits rows without collecting them, optionally newest first.
    pub fn visit(
        &self,
        collection: Collection,
        newest_first: bool,
        mut visitor: impl FnMut(OwnedRecordKey, RecordEnvelope) -> StoreResult<()>,
    ) -> StoreResult<()> {
        collection.validate_store(self.kind)?;
        let table = self.inner.open_table(collection.table())?;
        let mut iterator = table.iter()?;
        loop {
            let entry = if newest_first {
                iterator.next_back()
            } else {
                iterator.next()
            };
            let Some(entry) = entry else { break };
            let (key, value) = entry?;
            visitor(
                decode_key(collection, key.value())?,
                RecordEnvelope::decode(value.value())?,
            )?;
        }
        Ok(())
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
        let encoded_key = key.encode(collection)?;
        let owned_key = match key {
            RecordKey::Singleton => OwnedRecordKey::Singleton,
            RecordKey::Id(id) => OwnedRecordKey::Id(id),
            RecordKey::Bytes(bytes) => OwnedRecordKey::Bytes(bytes.to_vec()),
        };
        crate::validation::record(collection, &owned_key, record)
            .map_err(StoreError::InvalidImport)?;
        let numeric_id = match key {
            RecordKey::Id(id) => Some(id),
            _ => None,
        };
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

fn initialize_database(
    database: &Database,
    kind: StoreKind,
    state: &[u8],
    origin: &[u8],
) -> StoreResult<()> {
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
        manifest.insert(MANIFEST_KEY_STATE, state)?;
        manifest.insert(MANIFEST_KEY_STORE_KIND, kind.as_bytes())?;
        manifest.insert(MANIFEST_KEY_ORIGIN, origin)?;
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
    let entries = read_manifest_entries(&transaction)?;

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

    let common_manifest_keys = BTreeSet::from([
        MANIFEST_KEY_FAMILY.to_vec(),
        MANIFEST_KEY_ORIGIN.to_vec(),
        MANIFEST_KEY_OWNER.to_vec(),
        MANIFEST_KEY_SCHEMA_VERSION.to_vec(),
        MANIFEST_KEY_STATE.to_vec(),
        MANIFEST_KEY_STORE_KIND.to_vec(),
    ]);
    let origin = entries
        .get(MANIFEST_KEY_ORIGIN)
        .ok_or_else(|| StoreError::InvalidManifest("origin is missing".to_owned()))?;
    let expected_manifest_keys = match origin.as_slice() {
        STORE_ORIGIN_CREATED => common_manifest_keys,
        STORE_ORIGIN_IMPORTED => {
            let mut keys = common_manifest_keys;
            keys.extend([
                MANIFEST_KEY_IMPORT_BUNDLE_VERSION.to_vec(),
                MANIFEST_KEY_IMPORT_SOURCE_FORMAT.to_vec(),
                MANIFEST_KEY_IMPORT_BUNDLE_SHA256.to_vec(),
            ]);
            keys
        }
        STORE_ORIGIN_JSON_STAGE => {
            let mut keys = common_manifest_keys;
            keys.extend([
                MANIFEST_KEY_STAGE_VERSION.to_vec(),
                MANIFEST_KEY_BATCH_MANIFEST_SHA256.to_vec(),
                MANIFEST_KEY_DATABASE_JSON_SHA256.to_vec(),
                MANIFEST_KEY_SOURCE_FORMAT.to_vec(),
                MANIFEST_KEY_QUARANTINE_COUNT.to_vec(),
            ]);
            keys
        }
        _ => {
            return Err(StoreError::InvalidManifest(
                "unexpected origin value".to_owned(),
            ));
        }
    };
    let state = entries
        .get(MANIFEST_KEY_STATE)
        .ok_or_else(|| StoreError::InvalidManifest("state is missing".to_owned()))?;
    let mut expected_manifest_keys = expected_manifest_keys;
    if state.as_slice() == STORE_STATE_ACTIVE {
        expected_manifest_keys.extend([
            MANIFEST_KEY_ACTIVATION_GENERATION.to_vec(),
            MANIFEST_KEY_DATABASE_ID.to_vec(),
            MANIFEST_KEY_CANONICAL_PATH.to_vec(),
            MANIFEST_KEY_APPLICATION_WRITES.to_vec(),
        ]);
    } else if state.as_slice() != STORE_STATE_READY {
        return Err(StoreError::InvalidManifest(
            "unexpected state value".to_owned(),
        ));
    }
    let actual_manifest_keys = entries.keys().cloned().collect::<BTreeSet<_>>();
    if actual_manifest_keys != expected_manifest_keys {
        return Err(StoreError::InvalidManifest(
            "schema metadata keys do not match the version-4 origin contract".to_owned(),
        ));
    }

    require_manifest_value(&entries, MANIFEST_KEY_OWNER, STORE_OWNER, "owner")?;
    if state.as_slice() == STORE_STATE_ACTIVE {
        crate::activation::binding_from_manifest(&entries)?.ok_or_else(|| {
            StoreError::InvalidManifest("active store has no activation binding".to_owned())
        })?;
    }

    let source_format = if origin == STORE_ORIGIN_IMPORTED {
        require_manifest_value(
            &entries,
            MANIFEST_KEY_IMPORT_BUNDLE_VERSION,
            IMPORT_BUNDLE_VERSION.to_be_bytes().as_slice(),
            "import bundle version",
        )?;
        let source_format = u64::from_be_bytes(require_manifest_bytes::<8>(
            &entries,
            MANIFEST_KEY_IMPORT_SOURCE_FORMAT,
            "import source format",
        )?);
        require_manifest_bytes::<32>(
            &entries,
            MANIFEST_KEY_IMPORT_BUNDLE_SHA256,
            "import bundle SHA-256",
        )?;
        Some(source_format)
    } else if origin == STORE_ORIGIN_JSON_STAGE {
        Some(
            json_stage_provenance(&entries)?
                .expect("the JSON-stage origin was checked")
                .source_format,
        )
    } else {
        None
    };

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
    if let Some(source_format) = source_format {
        match actual_kind {
            StoreKind::Project if source_format > MAX_LEGACY_PROJECT_FORMAT => {
                return Err(StoreError::InvalidManifest(format!(
                    "import project source format exceeds supported format {MAX_LEGACY_PROJECT_FORMAT}"
                )));
            }
            StoreKind::Global if source_format != 0 => {
                return Err(StoreError::InvalidManifest(
                    "import global source format must be zero".to_owned(),
                ));
            }
            StoreKind::Project | StoreKind::Global => {}
        }
    }

    let is_json_stage = origin == STORE_ORIGIN_JSON_STAGE;
    let expected_tables = expected_table_names(expected_kind, is_json_stage);
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
    validate_stored_records(&transaction, expected_kind, origin != STORE_ORIGIN_CREATED)?;
    if is_json_stage {
        let provenance = json_stage_provenance(&entries)?.expect("JSON-stage origin was checked");
        crate::quarantine::validate_stored(&transaction, provenance.quarantine_count)?;
    }
    Ok(())
}

fn read_manifest_entries(
    transaction: &redb::ReadTransaction,
) -> StoreResult<BTreeMap<Vec<u8>, Vec<u8>>> {
    let manifest = transaction.open_table(MANIFEST_TABLE).map_err(|error| {
        StoreError::InvalidManifest(format!("schema table is unavailable: {error}"))
    })?;
    let mut entries = BTreeMap::new();
    for entry in manifest.iter()? {
        let (key, value) = entry?;
        entries.insert(key.value().to_vec(), value.value().to_vec());
    }
    Ok(entries)
}

fn json_stage_provenance(
    entries: &BTreeMap<Vec<u8>, Vec<u8>>,
) -> StoreResult<Option<JsonStageProvenance>> {
    if entries.get(MANIFEST_KEY_ORIGIN).map(Vec::as_slice) != Some(STORE_ORIGIN_JSON_STAGE) {
        return Ok(None);
    }
    let stage_version = u16::from_be_bytes(require_manifest_bytes::<2>(
        entries,
        MANIFEST_KEY_STAGE_VERSION,
        "JSON stage version",
    )?);
    if stage_version != JSON_STAGE_VERSION {
        return Err(StoreError::InvalidManifest(format!(
            "unsupported JSON stage version {stage_version}"
        )));
    }
    Ok(Some(JsonStageProvenance {
        stage_version,
        source_format: u64::from_be_bytes(require_manifest_bytes::<8>(
            entries,
            MANIFEST_KEY_SOURCE_FORMAT,
            "JSON stage source format",
        )?),
        batch_manifest_sha256: require_manifest_bytes::<32>(
            entries,
            MANIFEST_KEY_BATCH_MANIFEST_SHA256,
            "batch manifest SHA-256",
        )?,
        database_json_sha256: require_manifest_bytes::<32>(
            entries,
            MANIFEST_KEY_DATABASE_JSON_SHA256,
            "database JSON SHA-256",
        )?,
        quarantine_count: u64::from_be_bytes(require_manifest_bytes::<8>(
            entries,
            MANIFEST_KEY_QUARANTINE_COUNT,
            "quarantine count",
        )?),
    }))
}

fn validate_stored_records(
    transaction: &redb::ReadTransaction,
    kind: StoreKind,
    require_complete_sequences: bool,
) -> StoreResult<()> {
    let sequences = transaction.open_table(SEQUENCES_TABLE)?;
    let mut sequence_values = BTreeMap::new();
    for entry in sequences.iter()? {
        let (key, value) = entry?;
        let collection = Collection::from_legacy_name(key.value())
            .filter(|collection| collection.store_kind() == kind && collection.is_sequenced());
        let Some(collection) = collection else {
            return Err(StoreError::InvalidManifest(
                "sequence table contains an unknown collection".to_owned(),
            ));
        };
        let encoded: [u8; 8] = value.value().try_into().map_err(|_| {
            StoreError::InvalidManifest(format!(
                "collection {} has a malformed sequence",
                collection.name()
            ))
        })?;
        sequence_values.insert(collection, u64::from_be_bytes(encoded));
    }

    for collection in collections_for(kind) {
        let table = transaction.open_table(collection.table())?;
        let mut maximum_id = 0_u64;
        for entry in table.iter()? {
            let (key, value) = entry?;
            let key = decode_key(collection, key.value())?;
            if matches!(key, OwnedRecordKey::Id(0)) {
                return Err(StoreError::InvalidManifest(format!(
                    "collection {} contains numeric ID zero",
                    collection.name()
                )));
            }
            if let OwnedRecordKey::Id(id) = &key {
                maximum_id = maximum_id.max(*id);
            }
            let envelope = RecordEnvelope::decode(value.value())?;
            crate::validation::record(collection, &key, &envelope)
                .map_err(StoreError::InvalidManifest)?;
        }
        if collection.is_sequenced() {
            let sequence = sequence_values.get(&collection).copied();
            if require_complete_sequences && sequence.is_none() {
                return Err(StoreError::InvalidManifest(format!(
                    "imported collection {} is missing its sequence",
                    collection.name()
                )));
            }
            if sequence.unwrap_or(0) < maximum_id {
                return Err(StoreError::InvalidManifest(format!(
                    "collection {} sequence is below its maximum ID",
                    collection.name()
                )));
            }
        }
    }
    Ok(())
}

fn expected_table_names(kind: StoreKind, include_quarantine: bool) -> BTreeSet<String> {
    let mut names = BTreeSet::from([
        MANIFEST_TABLE.name().to_owned(),
        SEQUENCES_TABLE.name().to_owned(),
    ]);
    names.extend(collections_for(kind).map(|collection| collection.table().name().to_owned()));
    if include_quarantine {
        names.insert(QUARANTINE_TABLE.name().to_owned());
    }
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

fn require_manifest_bytes<const N: usize>(
    entries: &BTreeMap<Vec<u8>, Vec<u8>>,
    key: &[u8],
    label: &str,
) -> StoreResult<[u8; N]> {
    entries
        .get(key)
        .ok_or_else(|| StoreError::InvalidManifest(format!("{label} is missing")))?
        .as_slice()
        .try_into()
        .map_err(|_| StoreError::InvalidManifest(format!("{label} must contain exactly {N} bytes")))
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
    FileIdentity::from_path(path, false)
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
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, WRITE_DAC, WRITE_OWNER,
        };

        options.access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE | WRITE_DAC | WRITE_OWNER);
        // Identity attestation reopens the path read-only. Keep that possible
        // while still denying competing writers and rename/delete replacement.
        options.share_mode(FILE_SHARE_READ);
    }

    match options.open(path) {
        Ok(file) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            #[cfg(windows)]
            crate::private_windows::protect_handle(&file)?;
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
            Err(StoreError::Io(error)) if open_conflicts_with_writer(&error) => {
                if start.elapsed() >= LOCK_TIMEOUT {
                    return Err(StoreError::Busy);
                }
                thread::sleep(LOCK_RETRY_INTERVAL);
                continue;
            }
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        validate_private_permissions(path, &metadata)?;
        let expected = FileIdentity::from_file(&file)?;
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

fn open_conflicts_with_writer(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::PermissionDenied {
        return true;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;

        return error.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32);
    }
    #[cfg(not(windows))]
    false
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
        if FileIdentity::from_file(file)? != expected {
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
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

        // Permit only the read-only handle used for identity attestation;
        // writes and rename/delete replacement remain denied.
        options.share_mode(FILE_SHARE_READ);
    }
    Ok(options.open(path)?)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume: u32,
    #[cfg(windows)]
    index: u64,
}

impl FileIdentity {
    pub(crate) fn from_file(file: &File) -> StoreResult<Self> {
        #[cfg(unix)]
        {
            Ok(Self::from_metadata(&file.metadata()?))
        }
        #[cfg(windows)]
        {
            let identity = crate::private_windows::identity(file)?;
            Ok(Self {
                volume: identity.volume,
                index: identity.index,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = file;
            Err(StoreError::DestinationParentIdentityUnavailable {
                path: PathBuf::new(),
            })
        }
    }

    pub(crate) fn from_path(path: &Path, directory: bool) -> StoreResult<Self> {
        #[cfg(unix)]
        {
            let _ = directory;
            Ok(Self::from_metadata(&fs::symlink_metadata(path)?))
        }
        #[cfg(windows)]
        {
            let file = crate::private_windows::open_no_reparse(path, directory, false, false)?;
            Self::from_file(&file)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = directory;
            Err(StoreError::DestinationParentIdentityUnavailable {
                path: path.to_path_buf(),
            })
        }
    }

    #[cfg(not(windows))]
    pub(crate) fn from_metadata(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = metadata;
            Self {}
        }
    }
}

struct DestinationParent {
    path: PathBuf,
    #[cfg(any(unix, windows))]
    identity: FileIdentity,
    #[cfg(any(unix, windows))]
    directory: File,
}

impl DestinationParent {
    fn capture(destination: &Path) -> StoreResult<Self> {
        let path = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        #[cfg(not(any(unix, windows)))]
        {
            Err(StoreError::DestinationParentIdentityUnavailable { path })
        }

        #[cfg(windows)]
        {
            let directory = crate::private_windows::open_no_reparse(&path, true, true, false)
                .map_err(|_| StoreError::DestinationParentInvalid { path: path.clone() })?;
            crate::private_windows::verify_private(&path)
                .map_err(|_| StoreError::DestinationParentInvalid { path: path.clone() })?;
            let identity = FileIdentity::from_file(&directory)?;
            if FileIdentity::from_path(&path, true)? != identity {
                return Err(StoreError::DestinationParentChanged { path });
            }
            Ok(Self {
                path,
                identity,
                directory,
            })
        }

        #[cfg(unix)]
        {
            let before = fs::symlink_metadata(&path)
                .map_err(|_| StoreError::DestinationParentInvalid { path: path.clone() })?;
            if before.file_type().is_symlink() || !before.is_dir() {
                return Err(StoreError::DestinationParentInvalid { path });
            }
            let identity = FileIdentity::from_metadata(&before);
            let directory = File::open(&path)?;
            let opened = directory.metadata()?;
            if !opened.is_dir() || FileIdentity::from_metadata(&opened) != identity {
                return Err(StoreError::DestinationParentChanged { path });
            }
            let after = fs::symlink_metadata(&path)
                .map_err(|_| StoreError::DestinationParentChanged { path: path.clone() })?;
            if after.file_type().is_symlink()
                || !after.is_dir()
                || FileIdentity::from_metadata(&after) != identity
            {
                return Err(StoreError::DestinationParentChanged { path });
            }
            Ok(Self {
                path,
                identity,
                directory,
            })
        }
    }

    fn ensure_current(&self) -> StoreResult<()> {
        #[cfg(not(any(unix, windows)))]
        {
            Err(StoreError::DestinationParentIdentityUnavailable {
                path: self.path.clone(),
            })
        }

        #[cfg(unix)]
        {
            let metadata = fs::symlink_metadata(&self.path).map_err(|_| {
                StoreError::DestinationParentChanged {
                    path: self.path.clone(),
                }
            })?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || FileIdentity::from_metadata(&metadata) != self.identity
            {
                return Err(StoreError::DestinationParentChanged {
                    path: self.path.clone(),
                });
            }
            let opened = self.directory.metadata()?;
            if !opened.is_dir() || FileIdentity::from_metadata(&opened) != self.identity {
                return Err(StoreError::DestinationParentChanged {
                    path: self.path.clone(),
                });
            }
            Ok(())
        }

        #[cfg(windows)]
        {
            crate::private_windows::verify_private(&self.path).map_err(|_| {
                StoreError::DestinationParentChanged {
                    path: self.path.clone(),
                }
            })?;
            if FileIdentity::from_path(&self.path, true)? != self.identity
                || FileIdentity::from_file(&self.directory)? != self.identity
            {
                return Err(StoreError::DestinationParentChanged {
                    path: self.path.clone(),
                });
            }
            Ok(())
        }
    }

    fn ensure_destination(&self, destination: &Path, identity: FileIdentity) -> StoreResult<()> {
        self.ensure_current()?;
        ensure_path_identity(destination, identity)?;
        self.ensure_current()
    }

    fn sync(&self) -> StoreResult<()> {
        self.ensure_current()?;
        #[cfg(any(unix, windows))]
        self.directory.sync_all()?;
        self.ensure_current()
    }
}

fn ensure_committed_import_destination(
    parent: &DestinationParent,
    destination: &Path,
    identity: FileIdentity,
) -> StoreResult<()> {
    parent
        .ensure_destination(destination, identity)
        .map_err(|_| StoreError::ImportCommittedPathChanged {
            path: destination.to_path_buf(),
        })
}

fn committed_import_verification_error(path: &Path, error: StoreError) -> StoreError {
    match error {
        StoreError::DestinationParentChanged { .. }
        | StoreError::DestinationParentInvalid { .. }
        | StoreError::DestinationParentIdentityUnavailable { .. }
        | StoreError::PathChanged { .. } => StoreError::ImportCommittedPathChanged {
            path: path.to_path_buf(),
        },
        other => StoreError::ImportCommittedVerificationFailed {
            path: path.to_path_buf(),
            detail: other.to_string(),
        },
    }
}

fn validate_opened_path(path: &Path, opened: FileIdentity) -> StoreResult<()> {
    let path_identity = validate_existing_path(path)?;
    if path_identity != opened {
        return Err(StoreError::PathChanged {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

pub(crate) fn ensure_path_identity(path: &Path, expected: FileIdentity) -> StoreResult<()> {
    let metadata = fs::symlink_metadata(path)?;
    let invalid_type = metadata.file_type().is_symlink() || !metadata.is_file();
    let wrong_identity = FileIdentity::from_path(path, false)? != expected;
    if invalid_type || wrong_identity {
        Err(StoreError::PathChanged {
            path: path.to_path_buf(),
        })
    } else {
        Ok(())
    }
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

#[cfg(windows)]
fn validate_private_permissions(path: &Path, _metadata: &fs::Metadata) -> StoreResult<()> {
    crate::private_windows::verify_private(path).map_err(|_| StoreError::InsecurePermissions {
        path: path.to_path_buf(),
        mode: 0,
    })
}

#[cfg(not(any(unix, windows)))]
fn validate_private_permissions(_path: &Path, _metadata: &fs::Metadata) -> StoreResult<()> {
    Ok(())
}
