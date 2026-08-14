use std::path::Path;

use ptrack_core::{CapabilityAudit, CapabilityKind, NativeRecord, Timestamp, encode_record};
use redb::Database;

use crate::typed;
#[cfg(unix)]
use crate::{ActiveBinding, PinnedProjectDirectory};
use crate::{
    Collection, NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA, ProjectStore, RecordEnvelope, RecordKey,
    StoreResult,
};

impl ProjectStore {
    #[cfg(unix)]
    pub(crate) fn create_new_pinned_with_before_open(
        pinned: &PinnedProjectDirectory,
        binding: ActiveBinding,
        writer_version: impl Into<String>,
        before_open: impl FnOnce() -> StoreResult<()>,
    ) -> StoreResult<Self> {
        Self::create_new_pinned_inner(pinned, binding, writer_version, before_open)
    }

    #[cfg(unix)]
    pub(crate) fn open_existing_pinned_with_before_open(
        pinned: &PinnedProjectDirectory,
        binding: &ActiveBinding,
        writer_version: impl Into<String>,
        before_open: impl FnOnce() -> StoreResult<()>,
    ) -> StoreResult<Self> {
        Self::open_existing_pinned_inner(pinned, binding, writer_version, before_open)
    }

    pub(crate) fn seed_capability_audits(&self, count: u64) -> StoreResult<()> {
        self.write(|transaction| {
            for id in 1..=count {
                typed::put(transaction, RecordKey::Id(id), &audit(id, id))?;
            }
            Ok(())
        })
    }
}

pub(crate) fn inject_raw_audit_secret(path: &Path) {
    let record = NativeRecord::CapabilityAudit(audit(1, 1));
    let mut payload = encode_record(&record).unwrap();
    let index = payload
        .windows(b"fetch".len())
        .position(|window| window == b"fetch")
        .expect("encoded operation");
    payload[index..index + b"fetch".len()].copy_from_slice(b"token");
    let envelope = RecordEnvelope::new(NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA, payload).encode();
    let database = Database::open(path).unwrap();
    let write = database.begin_write().unwrap();
    {
        let mut table = write
            .open_table(Collection::CapabilityAudits.table())
            .unwrap();
        table
            .insert(1_u64.to_be_bytes().as_slice(), envelope.as_slice())
            .unwrap();
    }
    write.commit().unwrap();
}

fn audit(id: u64, capability_id: u64) -> CapabilityAudit {
    CapabilityAudit {
        id,
        capability_id,
        agent_profile: "agent".to_owned(),
        kind: CapabilityKind::Git,
        operation: "fetch".to_owned(),
        target: "remote:origin".to_owned(),
        success: true,
        error_class: "none".to_owned(),
        duration_millis: 0,
        request_bytes: 0,
        response_bytes: 0,
        redirects: 0,
        created_at: Timestamp::Fixed {
            seconds: 1_700_000_000,
            nanoseconds: 123,
            offset_seconds: 0,
        },
    }
}
