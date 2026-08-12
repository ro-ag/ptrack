use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::Path;

use ptrack_core::{NativeRecord, RecordKind, Timestamp, decode_record};
use redb::{Database, Durability, ReadableTable};

use crate::envelope::RECORD_ENVELOPE_HEADER_LENGTH;
use crate::schema::{
    MANIFEST_KEY_BATCH_MANIFEST_SHA256, MANIFEST_KEY_DATABASE_JSON_SHA256,
    MANIFEST_KEY_IMPORT_BUNDLE_SHA256, MANIFEST_KEY_IMPORT_BUNDLE_VERSION,
    MANIFEST_KEY_IMPORT_SOURCE_FORMAT, MANIFEST_KEY_QUARANTINE_COUNT, MANIFEST_KEY_SOURCE_FORMAT,
    MANIFEST_KEY_STAGE_VERSION, MANIFEST_KEY_STATE, MANIFEST_TABLE, SEQUENCES_TABLE,
    STORE_STATE_READY,
};
use crate::{
    Collection, OwnedRecordKey, QuarantinedLegacyRecord, RecordEnvelope, StoreError, StoreKind,
    StoreResult,
};

/// Maximum number of records accepted by one complete database import.
pub const MAX_IMPORT_RECORDS: u64 = 1_000_000;
/// Maximum combined key, envelope-header, and raw payload bytes in one import.
pub const MAX_IMPORT_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum encoded key length accepted for one imported record.
pub const MAX_IMPORT_KEY_BYTES: u64 = 1024 * 1024;
/// Maximum raw legacy payload length accepted for one imported record.
pub const MAX_IMPORT_PAYLOAD_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum encoded envelope length accepted for one imported record.
pub const MAX_IMPORT_ENVELOPE_BYTES: u64 = MAX_IMPORT_PAYLOAD_BYTES
    .checked_add(RECORD_ENVELOPE_HEADER_LENGTH as u64)
    .expect("the fixed import envelope bound fits in u64");
/// The only compatibility bundle layout accepted by the schema-v3 destination.
pub const IMPORT_BUNDLE_VERSION: u16 = 2;
/// The only standalone JSON staging contract accepted by schema v3.
pub const JSON_STAGE_VERSION: u16 = 1;
pub(crate) const MAX_LEGACY_PROJECT_FORMAT: u64 = 5;

/// Immutable source provenance committed atomically with a completed import.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImportProvenance {
    pub bundle_version: u16,
    pub source_format: u64,
    pub bundle_sha256: [u8; 32],
}

/// One complete, destination-family-specific legacy database image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportData {
    pub kind: StoreKind,
    pub provenance: ImportProvenance,
    pub collections: Vec<ImportCollection>,
}

/// Immutable provenance for one candidate built from the standalone JSON stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JsonStageProvenance {
    pub stage_version: u16,
    pub source_format: u64,
    pub batch_manifest_sha256: [u8; 32],
    pub database_json_sha256: [u8; 32],
    pub quarantine_count: u64,
}

/// One complete candidate image decoded from a validated JSON staging artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JsonStageImportData {
    pub kind: StoreKind,
    pub source_format: u64,
    pub batch_manifest_sha256: [u8; 32],
    pub database_json_sha256: [u8; 32],
    pub collections: Vec<ImportCollection>,
    pub quarantine: Vec<QuarantinedLegacyRecord>,
}

/// Every record and the exact legacy sequence for one collection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportCollection {
    pub collection: Collection,
    pub records: Vec<ImportRecord>,
    /// Exact bbolt bucket high-water mark; required only for sequenced buckets.
    pub sequence: Option<u64>,
}

/// One already-wrapped legacy value addressed by its typed key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportRecord {
    pub key: OwnedRecordKey,
    pub envelope: RecordEnvelope,
}

/// Verified summary of a completed import.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportReport {
    pub kind: StoreKind,
    pub record_count: u64,
    pub quarantine_count: u64,
    pub encoded_bytes: u64,
    pub collections: Vec<ImportCollectionReport>,
}

/// Verified counts and sequence for one imported collection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportCollectionReport {
    pub collection: Collection,
    pub record_count: u64,
    pub encoded_bytes: u64,
    pub sequence: Option<u64>,
}

pub(crate) struct ValidatedImport {
    pub(crate) data: ImportData,
    pub(crate) report: ImportReport,
}

pub(crate) struct ValidatedJsonStageImport {
    pub(crate) data: JsonStageImportData,
    pub(crate) report: ImportReport,
}

impl ImportData {
    pub(crate) fn validate(self) -> StoreResult<ValidatedImport> {
        if self.provenance.bundle_version != IMPORT_BUNDLE_VERSION {
            return Err(StoreError::InvalidImport(format!(
                "bundle version must be {IMPORT_BUNDLE_VERSION}, found {}",
                self.provenance.bundle_version
            )));
        }
        validate_source_format(self.kind, self.provenance.source_format)?;
        let report = validate_collections(self.kind, &self.collections, 0)?;
        Ok(ValidatedImport { data: self, report })
    }
}

impl JsonStageImportData {
    pub(crate) fn validate(self) -> StoreResult<ValidatedJsonStageImport> {
        validate_source_format(self.kind, self.source_format)?;
        if self.kind == StoreKind::Global && !self.quarantine.is_empty() {
            return Err(StoreError::InvalidImport(
                "global JSON stage cannot contain capability quarantine records".to_owned(),
            ));
        }
        let (quarantine_count, quarantine_bytes) = crate::quarantine::validate(&self.quarantine)?;
        validate_json_stage_capabilities(&self.collections)?;
        let mut report = validate_collections(self.kind, &self.collections, quarantine_count)?;
        checked_add(
            report.record_count,
            quarantine_count,
            "record count",
            MAX_IMPORT_RECORDS,
        )?;
        report.encoded_bytes = checked_add(
            report.encoded_bytes,
            quarantine_bytes,
            "encoded bytes",
            MAX_IMPORT_BYTES,
        )?;
        Ok(ValidatedJsonStageImport { data: self, report })
    }
}

fn validate_json_stage_capabilities(collections: &[ImportCollection]) -> StoreResult<()> {
    let Some(capabilities) = collections
        .iter()
        .find(|collection| collection.collection == Collection::Capabilities)
    else {
        return Ok(());
    };
    for record in &capabilities.records {
        let decoded =
            decode_record(RecordKind::Capability, record.envelope.payload()).map_err(|error| {
                StoreError::InvalidImport(format!("invalid JSON-stage capability: {error}"))
            })?;
        let NativeRecord::Capability(capability) = decoded else {
            return Err(StoreError::InvalidImport(
                "JSON-stage capability collection contains another record kind".to_owned(),
            ));
        };
        if capability.enabled
            || capability.approved_at != Timestamp::Zero
            || capability.expires_at != Timestamp::Zero
        {
            return Err(StoreError::InvalidImport(
                "JSON-stage capabilities must be disabled with approval times cleared".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_source_format(kind: StoreKind, source_format: u64) -> StoreResult<()> {
    match kind {
        StoreKind::Project if source_format > MAX_LEGACY_PROJECT_FORMAT => {
            Err(StoreError::InvalidImport(format!(
                "project source format {source_format} exceeds supported format {MAX_LEGACY_PROJECT_FORMAT}"
            )))
        }
        StoreKind::Global if source_format != 0 => Err(StoreError::InvalidImport(format!(
            "global source format must be zero, found {source_format}"
        ))),
        StoreKind::Project | StoreKind::Global => Ok(()),
    }
}

fn validate_collections(
    kind: StoreKind,
    collections: &[ImportCollection],
    quarantine_count: u64,
) -> StoreResult<ImportReport> {
    let expected = Collection::for_store(kind).collect::<Vec<_>>();
    if collections.len() != expected.len() {
        return Err(StoreError::InvalidImport(format!(
            "collection coverage must contain exactly {} collections",
            expected.len()
        )));
    }
    let mut total_records = 0_u64;
    let mut total_bytes = 0_u64;
    let mut reports = Vec::with_capacity(expected.len());

    for (imported, expected_collection) in collections.iter().zip(expected) {
        if imported.collection != expected_collection {
            return Err(StoreError::InvalidImport(format!(
                "collection coverage is not canonical: expected {}, found {}",
                expected_collection.name(),
                imported.collection.name()
            )));
        }
        if imported.collection.store_kind() != kind {
            return Err(StoreError::InvalidImport(format!(
                "collection {} does not belong to the {} database",
                imported.collection.name(),
                kind
            )));
        }
        match (imported.collection.is_sequenced(), imported.sequence) {
            (true, None) => {
                return Err(StoreError::InvalidImport(format!(
                    "collection {} is missing its sequence",
                    imported.collection.name()
                )));
            }
            (false, Some(_)) => {
                return Err(StoreError::InvalidImport(format!(
                    "collection {} must not have a sequence",
                    imported.collection.name()
                )));
            }
            _ => {}
        }

        if imported.collection == Collection::ProjectMeta && imported.records.len() != 1 {
            return Err(StoreError::InvalidImport(
                "project meta must contain exactly one singleton record".to_owned(),
            ));
        }

        let record_count = length_u64(imported.records.len())?;
        total_records = checked_add(
            total_records,
            record_count,
            "record count",
            MAX_IMPORT_RECORDS,
        )?;

        let mut collection_bytes = 0_u64;
        let mut maximum_id = 0_u64;
        let mut previous_key: Option<&OwnedRecordKey> = None;
        for record in &imported.records {
            let key_len = record.key.validated_encoded_len(imported.collection)?;
            if key_len == 0 {
                return Err(StoreError::InvalidImport(format!(
                    "collection {} contains an empty key",
                    imported.collection.name()
                )));
            }
            if let Some(previous) = previous_key {
                match previous.compare_encoded(&record.key, imported.collection)? {
                    Ordering::Less => {}
                    Ordering::Equal => {
                        return Err(StoreError::InvalidImport(format!(
                            "collection {} contains a duplicate key",
                            imported.collection.name()
                        )));
                    }
                    Ordering::Greater => {
                        return Err(StoreError::InvalidImport(format!(
                            "collection {} keys are not strictly increasing",
                            imported.collection.name()
                        )));
                    }
                }
            }
            previous_key = Some(&record.key);
            if matches!(record.key, OwnedRecordKey::Id(0)) {
                return Err(StoreError::InvalidImport(format!(
                    "collection {} contains numeric ID zero",
                    imported.collection.name()
                )));
            }
            if let OwnedRecordKey::Id(id) = &record.key {
                maximum_id = maximum_id.max(*id);
            }
            let record_bytes = validated_record_size(key_len, record.envelope.payload().len())?;
            crate::validation::record(imported.collection, &record.key, &record.envelope)
                .map_err(StoreError::InvalidImport)?;
            collection_bytes = checked_add(
                collection_bytes,
                record_bytes,
                "encoded bytes",
                MAX_IMPORT_BYTES,
            )?;
        }

        total_bytes = checked_add(
            total_bytes,
            collection_bytes,
            "encoded bytes",
            MAX_IMPORT_BYTES,
        )?;
        if imported
            .sequence
            .is_some_and(|sequence| sequence < maximum_id)
        {
            return Err(StoreError::InvalidImport(format!(
                "collection {} sequence is below its maximum numeric key",
                imported.collection.name()
            )));
        }
        reports.push(ImportCollectionReport {
            collection: imported.collection,
            record_count,
            encoded_bytes: collection_bytes,
            sequence: imported.sequence,
        });
    }
    let report = ImportReport {
        kind,
        record_count: total_records,
        quarantine_count,
        encoded_bytes: total_bytes,
        collections: reports,
    };
    Ok(report)
}

pub(crate) fn write_import(
    database: &Database,
    import: &ValidatedImport,
    destination: &Path,
    before_ready: impl FnOnce() -> StoreResult<()>,
) -> StoreResult<()> {
    let mut transaction = database.begin_write()?;
    transaction.set_durability(Durability::Immediate)?;
    transaction.set_quick_repair(true);

    let result = catch_unwind(AssertUnwindSafe(|| -> StoreResult<()> {
        write_collections(&transaction, &import.data.collections)?;
        verify_collections(&transaction, &import.data.collections)?;
        before_ready()?;
        let mut manifest = transaction.open_table(MANIFEST_TABLE)?;
        manifest.insert(
            MANIFEST_KEY_IMPORT_BUNDLE_VERSION,
            import
                .data
                .provenance
                .bundle_version
                .to_be_bytes()
                .as_slice(),
        )?;
        manifest.insert(
            MANIFEST_KEY_IMPORT_SOURCE_FORMAT,
            import
                .data
                .provenance
                .source_format
                .to_be_bytes()
                .as_slice(),
        )?;
        manifest.insert(
            MANIFEST_KEY_IMPORT_BUNDLE_SHA256,
            import.data.provenance.bundle_sha256.as_slice(),
        )?;
        manifest.insert(MANIFEST_KEY_STATE, STORE_STATE_READY)?;
        Ok(())
    }));

    match result {
        Ok(Ok(())) => {
            transaction
                .commit()
                .map_err(|error| StoreError::ImportCommitOutcomeUnknown {
                    path: destination.to_path_buf(),
                    detail: error.to_string(),
                })
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

pub(crate) fn write_json_stage_import(
    database: &Database,
    import: &ValidatedJsonStageImport,
    destination: &Path,
    before_ready: impl FnOnce() -> StoreResult<()>,
) -> StoreResult<()> {
    let mut transaction = database.begin_write()?;
    transaction.set_durability(Durability::Immediate)?;
    transaction.set_quick_repair(true);

    let result = catch_unwind(AssertUnwindSafe(|| -> StoreResult<()> {
        write_collections(&transaction, &import.data.collections)?;
        crate::quarantine::write(&transaction, &import.data.quarantine)?;
        verify_collections(&transaction, &import.data.collections)?;
        crate::quarantine::verify_written(&transaction, &import.data.quarantine)?;
        before_ready()?;
        let mut manifest = transaction.open_table(MANIFEST_TABLE)?;
        manifest.insert(
            MANIFEST_KEY_STAGE_VERSION,
            JSON_STAGE_VERSION.to_be_bytes().as_slice(),
        )?;
        manifest.insert(
            MANIFEST_KEY_BATCH_MANIFEST_SHA256,
            import.data.batch_manifest_sha256.as_slice(),
        )?;
        manifest.insert(
            MANIFEST_KEY_DATABASE_JSON_SHA256,
            import.data.database_json_sha256.as_slice(),
        )?;
        manifest.insert(
            MANIFEST_KEY_SOURCE_FORMAT,
            import.data.source_format.to_be_bytes().as_slice(),
        )?;
        manifest.insert(
            MANIFEST_KEY_QUARANTINE_COUNT,
            import.report.quarantine_count.to_be_bytes().as_slice(),
        )?;
        manifest.insert(MANIFEST_KEY_STATE, STORE_STATE_READY)?;
        Ok(())
    }));

    match result {
        Ok(Ok(())) => {
            transaction
                .commit()
                .map_err(|error| StoreError::ImportCommitOutcomeUnknown {
                    path: destination.to_path_buf(),
                    detail: error.to_string(),
                })
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

fn write_collections(
    transaction: &redb::WriteTransaction,
    collections: &[ImportCollection],
) -> StoreResult<()> {
    for imported in collections {
        let mut table = transaction.open_table(imported.collection.table())?;
        for record in &imported.records {
            let key = record.key.as_borrowed().encode(imported.collection)?;
            let envelope = record.envelope.encode();
            table.insert(key.as_slice(), envelope.as_slice())?;
        }
    }
    let mut sequences = transaction.open_table(SEQUENCES_TABLE)?;
    for imported in collections {
        if let Some(sequence) = imported.sequence {
            sequences.insert(
                imported.collection.name().as_bytes(),
                sequence.to_be_bytes().as_slice(),
            )?;
        }
    }
    Ok(())
}

fn verify_collections(
    transaction: &redb::WriteTransaction,
    collections: &[ImportCollection],
) -> StoreResult<()> {
    for imported in collections {
        let table = transaction.open_table(imported.collection.table())?;
        let mut actual = table.iter()?;
        for expected in &imported.records {
            let (key, value) = actual.next().ok_or_else(|| {
                StoreError::InvalidImport(format!(
                    "collection {} has fewer records than expected",
                    imported.collection.name()
                ))
            })??;
            if !expected
                .key
                .matches_encoded(imported.collection, key.value())?
                || value.value() != expected.envelope.encode()
            {
                return Err(StoreError::InvalidImport(format!(
                    "collection {} failed post-write verification",
                    imported.collection.name()
                )));
            }
        }
        if actual.next().transpose()?.is_some() {
            return Err(StoreError::InvalidImport(format!(
                "collection {} has more records than expected",
                imported.collection.name()
            )));
        }
    }

    let sequences = transaction.open_table(SEQUENCES_TABLE)?;
    let mut actual_sequences = BTreeMap::new();
    for entry in sequences.iter()? {
        let (key, value) = entry?;
        let encoded = value.value();
        let sequence = u64::from_be_bytes(encoded.try_into().map_err(|_| {
            StoreError::InvalidImport("an imported sequence is malformed".to_owned())
        })?);
        actual_sequences.insert(key.value().to_vec(), sequence);
    }
    let expected_sequences = collections
        .iter()
        .filter_map(|imported| {
            imported
                .sequence
                .map(|sequence| (imported.collection.name().as_bytes().to_vec(), sequence))
        })
        .collect::<BTreeMap<_, _>>();
    if actual_sequences != expected_sequences {
        return Err(StoreError::InvalidImport(
            "sequence metadata failed post-write verification".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn length_u64(length: usize) -> StoreResult<u64> {
    u64::try_from(length).map_err(|_| StoreError::ImportLimitExceeded {
        limit: "platform length",
        maximum: u64::MAX,
        actual: u64::MAX,
    })
}

pub(crate) fn validated_record_size(key_len: usize, payload_len: usize) -> StoreResult<u64> {
    let key_len = length_u64(key_len)?;
    let payload_len = length_u64(payload_len)?;
    require_limit("record key bytes", MAX_IMPORT_KEY_BYTES, key_len)?;
    require_limit(
        "record payload bytes",
        MAX_IMPORT_PAYLOAD_BYTES,
        payload_len,
    )?;
    let envelope_len = payload_len
        .checked_add(RECORD_ENVELOPE_HEADER_LENGTH as u64)
        .ok_or(StoreError::ImportLimitExceeded {
            limit: "record envelope bytes",
            maximum: MAX_IMPORT_ENVELOPE_BYTES,
            actual: u64::MAX,
        })?;
    require_limit(
        "record envelope bytes",
        MAX_IMPORT_ENVELOPE_BYTES,
        envelope_len,
    )?;
    checked_add(key_len, envelope_len, "encoded bytes", MAX_IMPORT_BYTES)
}

pub(crate) fn checked_add(
    current: u64,
    added: u64,
    limit: &'static str,
    maximum: u64,
) -> StoreResult<u64> {
    let actual = current
        .checked_add(added)
        .ok_or(StoreError::ImportLimitExceeded {
            limit,
            maximum,
            actual: u64::MAX,
        })?;
    require_limit(limit, maximum, actual)?;
    Ok(actual)
}

pub(crate) fn require_limit(limit: &'static str, maximum: u64, actual: u64) -> StoreResult<()> {
    if actual <= maximum {
        Ok(())
    } else {
        Err(StoreError::ImportLimitExceeded {
            limit,
            maximum,
            actual,
        })
    }
}
