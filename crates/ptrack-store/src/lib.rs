#![forbid(unsafe_code)]

mod envelope;
mod error;
mod import;
mod quarantine;
mod schema;
mod sha256;
mod store;
mod validation;

pub use envelope::{
    LEGACY_CODEC_GO_GOB, LEGACY_CODEC_RAW, NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA,
    RECORD_ENVELOPE_VERSION, RecordEnvelope,
};
pub use error::{EnvelopeError, StoreError, StoreResult};
pub use import::{
    IMPORT_BUNDLE_VERSION, ImportCollection, ImportCollectionReport, ImportData, ImportProvenance,
    ImportRecord, ImportReport, JSON_STAGE_VERSION, JsonStageImportData, JsonStageProvenance,
    MAX_IMPORT_BYTES, MAX_IMPORT_ENVELOPE_BYTES, MAX_IMPORT_KEY_BYTES, MAX_IMPORT_PAYLOAD_BYTES,
    MAX_IMPORT_RECORDS,
};
pub use quarantine::{QuarantineReason, QuarantinedLegacyRecord};
pub use schema::{Collection, OwnedRecordKey, RecordKey, STORE_SCHEMA_VERSION, StoreKind};
pub use store::{ReadTransaction, Store, WriteTransaction};

#[cfg(test)]
mod envelope_test;
#[cfg(test)]
mod import_test;
#[cfg(test)]
mod quarantine_test;
#[cfg(test)]
mod schema_test;
#[cfg(test)]
mod sha256_test;
#[cfg(test)]
mod store_test;
#[cfg(test)]
mod validation_test;
