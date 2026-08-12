use std::cmp::Ordering;

use redb::ReadableTable;

use crate::import::{
    MAX_IMPORT_BYTES, MAX_IMPORT_KEY_BYTES, MAX_IMPORT_PAYLOAD_BYTES, checked_add, length_u64,
    require_limit,
};
use crate::schema::QUARANTINE_TABLE;
use crate::sha256;
use crate::{StoreError, StoreResult};

const QUARANTINE_KEY_HEADER_BYTES: usize = 2 + 4;
const QUARANTINE_VALUE_HEADER_BYTES: usize = 1 + 32 + 8;

/// A closed explanation for retaining, but never activating, a legacy value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuarantineReason {
    InvalidCapability,
    InvalidCapabilityAudit,
}

impl QuarantineReason {
    const fn tag(self) -> u8 {
        match self {
            Self::InvalidCapability => 1,
            Self::InvalidCapabilityAudit => 2,
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::InvalidCapability),
            2 => Some(Self::InvalidCapabilityAudit),
            _ => None,
        }
    }

    const fn source_bucket(self) -> &'static [u8] {
        match self {
            Self::InvalidCapability => b"capabilities",
            Self::InvalidCapabilityAudit => b"capability_audits",
        }
    }
}

/// Exact forensic bytes excluded from ordinary ptrack collections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantinedLegacyRecord {
    pub source_bucket: Vec<u8>,
    pub source_key: Vec<u8>,
    pub legacy_gob: Vec<u8>,
    pub source_value_sha256: [u8; 32],
    pub reason: QuarantineReason,
}

pub(crate) fn validate(records: &[QuarantinedLegacyRecord]) -> StoreResult<(u64, u64)> {
    let count = length_u64(records.len())?;
    require_limit("quarantine record count", crate::MAX_IMPORT_RECORDS, count)?;
    let mut bytes = 0_u64;
    let mut previous_key: Option<Vec<u8>> = None;
    for record in records {
        let key = encode_key(record)?;
        if let Some(previous) = &previous_key {
            match previous.cmp(&key) {
                Ordering::Less => {}
                Ordering::Equal => {
                    return Err(StoreError::InvalidImport(
                        "quarantine contains a duplicate source bucket and key".to_owned(),
                    ));
                }
                Ordering::Greater => {
                    return Err(StoreError::InvalidImport(
                        "quarantine entries are not in canonical source order".to_owned(),
                    ));
                }
            }
        }
        previous_key = Some(key);
        let encoded_value = encode_value(record)?;
        if record.source_value_sha256 != sha256::digest(&record.legacy_gob) {
            return Err(StoreError::InvalidImport(
                "quarantine source value SHA-256 does not match its exact gob bytes".to_owned(),
            ));
        }
        bytes = checked_add(
            bytes,
            length_u64(previous_key.as_ref().expect("key assigned").len())?,
            "encoded bytes",
            MAX_IMPORT_BYTES,
        )?;
        bytes = checked_add(
            bytes,
            length_u64(encoded_value.len())?,
            "encoded bytes",
            MAX_IMPORT_BYTES,
        )?;
    }
    Ok((count, bytes))
}

pub(crate) fn write(
    transaction: &redb::WriteTransaction,
    records: &[QuarantinedLegacyRecord],
) -> StoreResult<()> {
    let mut table = transaction.open_table(QUARANTINE_TABLE)?;
    for record in records {
        table.insert(
            encode_key(record)?.as_slice(),
            encode_value(record)?.as_slice(),
        )?;
    }
    Ok(())
}

pub(crate) fn verify_written(
    transaction: &redb::WriteTransaction,
    records: &[QuarantinedLegacyRecord],
) -> StoreResult<()> {
    let table = transaction.open_table(QUARANTINE_TABLE)?;
    let mut actual = table.iter()?;
    for expected in records {
        let (key, value) = actual.next().ok_or_else(|| {
            StoreError::InvalidImport("quarantine has fewer records than expected".to_owned())
        })??;
        if key.value() != encode_key(expected)? || value.value() != encode_value(expected)? {
            return Err(StoreError::InvalidImport(
                "quarantine failed post-write verification".to_owned(),
            ));
        }
    }
    if actual.next().transpose()?.is_some() {
        return Err(StoreError::InvalidImport(
            "quarantine has more records than expected".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_stored(
    transaction: &redb::ReadTransaction,
    expected_count: u64,
) -> StoreResult<()> {
    if expected_count > crate::MAX_IMPORT_RECORDS {
        return Err(StoreError::InvalidManifest(
            "quarantine count exceeds the schema limit".to_owned(),
        ));
    }
    let table = transaction.open_table(QUARANTINE_TABLE)?;
    let mut count = 0_u64;
    let mut total_bytes = 0_u64;
    for entry in table.iter()? {
        let (key, value) = entry?;
        total_bytes = total_bytes
            .checked_add(u64::try_from(key.value().len()).map_err(|_| {
                StoreError::InvalidManifest("quarantine key length overflows".to_owned())
            })?)
            .and_then(|total| {
                u64::try_from(value.value().len())
                    .ok()
                    .and_then(|length| total.checked_add(length))
            })
            .ok_or_else(|| {
                StoreError::InvalidManifest("quarantine byte count overflow".to_owned())
            })?;
        if total_bytes > MAX_IMPORT_BYTES {
            return Err(StoreError::InvalidManifest(
                "quarantine bytes exceed the schema limit".to_owned(),
            ));
        }
        let (bucket, source_key) = decode_key(key.value())?;
        let (reason, expected_digest, gob) = decode_value(value.value())?;
        if bucket != reason.source_bucket() {
            return Err(StoreError::InvalidManifest(
                "quarantine reason does not match its source bucket".to_owned(),
            ));
        }
        if source_key.is_empty() {
            return Err(StoreError::InvalidManifest(
                "quarantine contains an empty source key".to_owned(),
            ));
        }
        if u64::try_from(source_key.len()).unwrap_or(u64::MAX) > MAX_IMPORT_KEY_BYTES
            || u64::try_from(gob.len()).unwrap_or(u64::MAX) > MAX_IMPORT_PAYLOAD_BYTES
        {
            return Err(StoreError::InvalidManifest(
                "quarantine key or gob exceeds the schema limit".to_owned(),
            ));
        }
        if expected_digest != sha256::digest(gob) {
            return Err(StoreError::InvalidManifest(
                "quarantine source value SHA-256 does not match its gob bytes".to_owned(),
            ));
        }
        count = count
            .checked_add(1)
            .ok_or_else(|| StoreError::InvalidManifest("quarantine count overflow".to_owned()))?;
    }
    if count != expected_count {
        return Err(StoreError::InvalidManifest(format!(
            "quarantine count is {count}, manifest declares {expected_count}"
        )));
    }
    Ok(())
}

fn encode_key(record: &QuarantinedLegacyRecord) -> StoreResult<Vec<u8>> {
    if record.source_bucket.as_slice() != record.reason.source_bucket() {
        return Err(StoreError::InvalidImport(
            "quarantine reason does not match its source bucket".to_owned(),
        ));
    }
    if record.source_key.is_empty() {
        return Err(StoreError::InvalidImport(
            "quarantine contains an empty source key".to_owned(),
        ));
    }
    require_limit(
        "quarantine source key bytes",
        MAX_IMPORT_KEY_BYTES,
        length_u64(record.source_key.len())?,
    )?;
    let bucket_len = u16::try_from(record.source_bucket.len()).map_err(|_| {
        StoreError::InvalidImport("quarantine source bucket is too long".to_owned())
    })?;
    let key_len = u32::try_from(record.source_key.len())
        .map_err(|_| StoreError::InvalidImport("quarantine source key is too long".to_owned()))?;
    let mut encoded = Vec::with_capacity(
        QUARANTINE_KEY_HEADER_BYTES + record.source_bucket.len() + record.source_key.len(),
    );
    encoded.extend_from_slice(&bucket_len.to_be_bytes());
    encoded.extend_from_slice(&record.source_bucket);
    encoded.extend_from_slice(&key_len.to_be_bytes());
    encoded.extend_from_slice(&record.source_key);
    Ok(encoded)
}

fn encode_value(record: &QuarantinedLegacyRecord) -> StoreResult<Vec<u8>> {
    require_limit(
        "quarantine gob bytes",
        MAX_IMPORT_PAYLOAD_BYTES,
        length_u64(record.legacy_gob.len())?,
    )?;
    let gob_len = u64::try_from(record.legacy_gob.len()).map_err(|_| {
        StoreError::InvalidImport("quarantine gob length cannot be represented".to_owned())
    })?;
    let mut encoded = Vec::with_capacity(QUARANTINE_VALUE_HEADER_BYTES + record.legacy_gob.len());
    encoded.push(record.reason.tag());
    encoded.extend_from_slice(&record.source_value_sha256);
    encoded.extend_from_slice(&gob_len.to_be_bytes());
    encoded.extend_from_slice(&record.legacy_gob);
    Ok(encoded)
}

fn decode_key(encoded: &[u8]) -> StoreResult<(&[u8], &[u8])> {
    if encoded.len() < QUARANTINE_KEY_HEADER_BYTES {
        return Err(StoreError::InvalidManifest(
            "quarantine key header is truncated".to_owned(),
        ));
    }
    let bucket_len = usize::from(u16::from_be_bytes(
        encoded[..2]
            .try_into()
            .expect("quarantine key header checked"),
    ));
    let key_len_offset = 2_usize
        .checked_add(bucket_len)
        .ok_or_else(|| StoreError::InvalidManifest("quarantine key length overflow".to_owned()))?;
    let key_offset = key_len_offset
        .checked_add(4)
        .ok_or_else(|| StoreError::InvalidManifest("quarantine key length overflow".to_owned()))?;
    if key_offset > encoded.len() {
        return Err(StoreError::InvalidManifest(
            "quarantine key bucket is truncated".to_owned(),
        ));
    }
    let source_key_len = usize::try_from(u32::from_be_bytes(
        encoded[key_len_offset..key_offset]
            .try_into()
            .expect("quarantine key length field checked"),
    ))
    .expect("u32 fits usize on supported Rust targets");
    if key_offset.checked_add(source_key_len) != Some(encoded.len()) {
        return Err(StoreError::InvalidManifest(
            "quarantine key length is not canonical".to_owned(),
        ));
    }
    Ok((&encoded[2..key_len_offset], &encoded[key_offset..]))
}

fn decode_value(encoded: &[u8]) -> StoreResult<(QuarantineReason, [u8; 32], &[u8])> {
    if encoded.len() < QUARANTINE_VALUE_HEADER_BYTES {
        return Err(StoreError::InvalidManifest(
            "quarantine value header is truncated".to_owned(),
        ));
    }
    let reason = QuarantineReason::from_tag(encoded[0])
        .ok_or_else(|| StoreError::InvalidManifest("quarantine reason is unknown".to_owned()))?;
    let digest = encoded[1..33]
        .try_into()
        .expect("quarantine digest width checked");
    let gob_len = usize::try_from(u64::from_be_bytes(
        encoded[33..41]
            .try_into()
            .expect("quarantine value header checked"),
    ))
    .map_err(|_| StoreError::InvalidManifest("quarantine gob length overflows".to_owned()))?;
    if QUARANTINE_VALUE_HEADER_BYTES.checked_add(gob_len) != Some(encoded.len()) {
        return Err(StoreError::InvalidManifest(
            "quarantine gob length is not canonical".to_owned(),
        ));
    }
    Ok((reason, digest, &encoded[QUARANTINE_VALUE_HEADER_BYTES..]))
}
