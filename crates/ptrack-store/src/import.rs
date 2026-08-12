use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use redb::{Database, Durability, ReadableTable};

use crate::envelope::RECORD_ENVELOPE_HEADER_LENGTH;
use crate::schema::{MANIFEST_KEY_STATE, MANIFEST_TABLE, SEQUENCES_TABLE, STORE_STATE_READY};
use crate::{Collection, OwnedRecordKey, RecordEnvelope, StoreError, StoreKind, StoreResult};

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

/// One complete, destination-family-specific legacy database image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportData {
    pub kind: StoreKind,
    pub collections: Vec<ImportCollection>,
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

impl ImportData {
    pub(crate) fn validate(self) -> StoreResult<ValidatedImport> {
        let expected = Collection::for_store(self.kind).collect::<Vec<_>>();
        if self.collections.len() != expected.len() {
            return Err(StoreError::InvalidImport(format!(
                "collection coverage must contain exactly {} collections",
                expected.len()
            )));
        }
        let mut total_records = 0_u64;
        let mut total_bytes = 0_u64;
        let mut reports = Vec::with_capacity(expected.len());

        for (imported, expected_collection) in self.collections.iter().zip(expected) {
            if imported.collection != expected_collection {
                return Err(StoreError::InvalidImport(format!(
                    "collection coverage is not canonical: expected {}, found {}",
                    expected_collection.name(),
                    imported.collection.name()
                )));
            }
            if imported.collection.store_kind() != self.kind {
                return Err(StoreError::InvalidImport(format!(
                    "collection {} does not belong to the {} database",
                    imported.collection.name(),
                    self.kind
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
                if record.envelope.codec() != imported.collection.legacy_codec() {
                    return Err(StoreError::InvalidImport(format!(
                        "collection {} uses codec {} instead of legacy codec {}",
                        imported.collection.name(),
                        record.envelope.codec(),
                        imported.collection.legacy_codec()
                    )));
                }
                let record_bytes = validated_record_size(key_len, record.envelope.payload().len())?;
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
            kind: self.kind,
            record_count: total_records,
            encoded_bytes: total_bytes,
            collections: reports,
        };
        Ok(ValidatedImport { data: self, report })
    }
}

pub(crate) fn write_import(
    database: &Database,
    import: &ValidatedImport,
    before_ready: impl FnOnce() -> StoreResult<()>,
) -> StoreResult<()> {
    let mut transaction = database.begin_write()?;
    transaction.set_durability(Durability::Immediate)?;
    transaction.set_quick_repair(true);

    let result = catch_unwind(AssertUnwindSafe(|| -> StoreResult<()> {
        for imported in &import.data.collections {
            let mut table = transaction.open_table(imported.collection.table())?;
            for record in &imported.records {
                let key = record.key.as_borrowed().encode(imported.collection)?;
                let envelope = record.envelope.encode();
                table.insert(key.as_slice(), envelope.as_slice())?;
            }
        }
        {
            let mut sequences = transaction.open_table(SEQUENCES_TABLE)?;
            for imported in &import.data.collections {
                if let Some(sequence) = imported.sequence {
                    sequences.insert(
                        imported.collection.name().as_bytes(),
                        sequence.to_be_bytes().as_slice(),
                    )?;
                }
            }
        }
        verify_import(&transaction, import)?;
        before_ready()?;
        let mut manifest = transaction.open_table(MANIFEST_TABLE)?;
        manifest.insert(MANIFEST_KEY_STATE, STORE_STATE_READY)?;
        Ok(())
    }));

    match result {
        Ok(Ok(())) => {
            transaction.commit()?;
            Ok(())
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

fn verify_import(
    transaction: &redb::WriteTransaction,
    import: &ValidatedImport,
) -> StoreResult<()> {
    for imported in &import.data.collections {
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
    let expected_sequences = import
        .data
        .collections
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

fn length_u64(length: usize) -> StoreResult<u64> {
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

fn checked_add(current: u64, added: u64, limit: &'static str, maximum: u64) -> StoreResult<u64> {
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

fn require_limit(limit: &'static str, maximum: u64, actual: u64) -> StoreResult<()> {
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
