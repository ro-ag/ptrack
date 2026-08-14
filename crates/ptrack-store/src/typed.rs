use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ptrack_core::{
    Capability, CapabilityAudit, Commit, Issue, MemoryWritebackRecord, Meta, Milestone,
    NativeRecord, Note, Plan, ProjectRef, RecordKind, Task, Timestamp, decode_record,
    encode_record,
};

use crate::{
    Collection, NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA, OwnedRecordKey, ReadTransaction,
    RecordEnvelope, RecordKey, StoreError, StoreResult, WriteTransaction,
};

/// Injectable timestamp source. Application mutations use local timestamps;
/// memory write-back explicitly uses UTC timestamps.
pub trait Clock: Send + Sync {
    fn now_local(&self) -> Timestamp;
    fn now_utc(&self) -> Timestamp;
}

/// Dependency-free system clock. The standard library exposes the instant but
/// not the host UTC offset, so both values use the same UTC representation.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_local(&self) -> Timestamp {
        system_timestamp()
    }

    fn now_utc(&self) -> Timestamp {
        system_timestamp()
    }
}

fn system_timestamp() -> Timestamp {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH);
    match duration {
        Ok(value) => Timestamp::Fixed {
            seconds: i64::try_from(value.as_secs()).unwrap_or(i64::MAX),
            nanoseconds: value.subsec_nanos(),
            offset_seconds: 0,
        },
        Err(error) => {
            let value = error.duration();
            let seconds = i64::try_from(value.as_secs()).unwrap_or(i64::MAX);
            if value.subsec_nanos() == 0 {
                Timestamp::Fixed {
                    seconds: -seconds,
                    nanoseconds: 0,
                    offset_seconds: 0,
                }
            } else {
                Timestamp::Fixed {
                    seconds: seconds.saturating_neg().saturating_sub(1),
                    nanoseconds: 1_000_000_000 - value.subsec_nanos(),
                    offset_seconds: 0,
                }
            }
        }
    }
}

pub(crate) trait StoredRecord: Clone + Sized {
    const COLLECTION: Collection;
    const KIND: RecordKind;

    fn key(&self) -> OwnedRecordKey;
    fn into_native(self) -> NativeRecord;
    fn from_native(record: NativeRecord) -> Option<Self>;
}

macro_rules! stored_id_record {
    ($type:ty, $collection:ident, $kind:ident, $variant:ident) => {
        impl StoredRecord for $type {
            const COLLECTION: Collection = Collection::$collection;
            const KIND: RecordKind = RecordKind::$kind;

            fn key(&self) -> OwnedRecordKey {
                OwnedRecordKey::Id(self.id)
            }

            fn into_native(self) -> NativeRecord {
                NativeRecord::$variant(self)
            }

            fn from_native(record: NativeRecord) -> Option<Self> {
                match record {
                    NativeRecord::$variant(value) => Some(value),
                    _ => None,
                }
            }
        }
    };
}

stored_id_record!(Plan, Plans, Plan, Plan);
stored_id_record!(Task, Tasks, Task, Task);
stored_id_record!(Note, Notes, Note, Note);
stored_id_record!(Milestone, Milestones, Milestone, Milestone);
stored_id_record!(Issue, Issues, Issue, Issue);
stored_id_record!(Commit, Commits, Commit, Commit);
stored_id_record!(Capability, Capabilities, Capability, Capability);
stored_id_record!(
    CapabilityAudit,
    CapabilityAudits,
    CapabilityAudit,
    CapabilityAudit
);

impl StoredRecord for Meta {
    const COLLECTION: Collection = Collection::ProjectMeta;
    const KIND: RecordKind = RecordKind::Meta;

    fn key(&self) -> OwnedRecordKey {
        OwnedRecordKey::Singleton
    }

    fn into_native(self) -> NativeRecord {
        NativeRecord::Meta(self)
    }

    fn from_native(record: NativeRecord) -> Option<Self> {
        match record {
            NativeRecord::Meta(value) => Some(value),
            _ => None,
        }
    }
}

impl StoredRecord for MemoryWritebackRecord {
    const COLLECTION: Collection = Collection::MemoryWritebacks;
    const KIND: RecordKind = RecordKind::MemoryWriteback;

    fn key(&self) -> OwnedRecordKey {
        unreachable!("memory write-back keys are request IDs")
    }

    fn into_native(self) -> NativeRecord {
        NativeRecord::MemoryWriteback(self)
    }

    fn from_native(record: NativeRecord) -> Option<Self> {
        match record {
            NativeRecord::MemoryWriteback(value) => Some(value),
            _ => None,
        }
    }
}

impl StoredRecord for ProjectRef {
    const COLLECTION: Collection = Collection::GlobalProjects;
    const KIND: RecordKind = RecordKind::ProjectRef;

    fn key(&self) -> OwnedRecordKey {
        OwnedRecordKey::Bytes(self.path.as_bytes().to_vec())
    }

    fn into_native(self) -> NativeRecord {
        NativeRecord::ProjectRef(self)
    }

    fn from_native(record: NativeRecord) -> Option<Self> {
        match record {
            NativeRecord::ProjectRef(value) => Some(value),
            _ => None,
        }
    }
}

pub(crate) fn encode<R: StoredRecord>(record: &R) -> StoreResult<RecordEnvelope> {
    let payload = encode_record(&record.clone().into_native())
        .map_err(|error| StoreError::InvalidManifest(error.to_string()))?;
    Ok(RecordEnvelope::new(
        NATIVE_CODEC,
        NATIVE_PAYLOAD_SCHEMA,
        payload,
    ))
}

pub(crate) fn decode<R: StoredRecord>(envelope: RecordEnvelope) -> StoreResult<R> {
    let native = decode_record(R::KIND, envelope.payload())
        .map_err(|error| StoreError::InvalidManifest(error.to_string()))?;
    R::from_native(native).ok_or_else(|| {
        StoreError::InvalidManifest("stored record kind does not match its collection".to_owned())
    })
}

pub(crate) fn get<R: StoredRecord>(
    transaction: &ReadTransaction,
    key: RecordKey<'_>,
) -> StoreResult<Option<R>> {
    transaction
        .get(R::COLLECTION, key)?
        .map(decode::<R>)
        .transpose()
}

pub(crate) fn get_write<R: StoredRecord>(
    transaction: &WriteTransaction,
    key: RecordKey<'_>,
) -> StoreResult<Option<R>> {
    transaction
        .get(R::COLLECTION, key)?
        .map(decode::<R>)
        .transpose()
}

pub(crate) fn put<R: StoredRecord>(
    transaction: &mut WriteTransaction,
    key: RecordKey<'_>,
    record: &R,
) -> StoreResult<Option<R>> {
    let expected = record.key();
    if expected.as_borrowed() != key {
        return Err(StoreError::InvalidManifest(
            "typed record key does not match its identity".to_owned(),
        ));
    }
    transaction
        .put(R::COLLECTION, key, &encode(record)?)?
        .map(decode::<R>)
        .transpose()
}

pub(crate) fn scan<R: StoredRecord>(transaction: &ReadTransaction) -> StoreResult<Vec<R>> {
    transaction
        .scan(R::COLLECTION)?
        .into_iter()
        .map(|(_, envelope)| decode::<R>(envelope))
        .collect()
}

pub(crate) fn scan_limited<R: StoredRecord>(
    transaction: &ReadTransaction,
    limit: usize,
    newest_first: bool,
) -> StoreResult<Vec<R>> {
    transaction
        .scan_limited(R::COLLECTION, limit, newest_first)?
        .into_iter()
        .map(|(_, envelope)| decode::<R>(envelope))
        .collect()
}

pub(crate) fn visit<R: StoredRecord>(
    transaction: &ReadTransaction,
    newest_first: bool,
    mut visitor: impl FnMut(R) -> StoreResult<()>,
) -> StoreResult<()> {
    transaction.visit(R::COLLECTION, newest_first, |_, envelope| {
        visitor(decode::<R>(envelope)?)
    })
}

pub(crate) fn visit_until<R: StoredRecord>(
    transaction: &ReadTransaction,
    newest_first: bool,
    deadline: Instant,
    mut visitor: impl FnMut(R) -> StoreResult<()>,
) -> StoreResult<()> {
    transaction.visit(R::COLLECTION, newest_first, |_, envelope| {
        if Instant::now() >= deadline {
            return Err(StoreError::DeadlineExceeded);
        }
        visitor(decode::<R>(envelope)?)
    })?;
    if Instant::now() >= deadline {
        return Err(StoreError::DeadlineExceeded);
    }
    Ok(())
}

pub(crate) fn scan_write<R: StoredRecord>(transaction: &WriteTransaction) -> StoreResult<Vec<R>> {
    transaction
        .scan(R::COLLECTION)?
        .into_iter()
        .map(|(_, envelope)| decode::<R>(envelope))
        .collect()
}
