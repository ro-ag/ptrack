#![forbid(unsafe_code)]

mod envelope;
mod error;
mod import;
mod schema;
mod store;

pub use envelope::{
    LEGACY_CODEC_GO_GOB, LEGACY_CODEC_RAW, RECORD_ENVELOPE_VERSION, RecordEnvelope,
};
pub use error::{EnvelopeError, StoreError, StoreResult};
pub use import::{
    ImportCollection, ImportCollectionReport, ImportData, ImportRecord, ImportReport,
    MAX_IMPORT_BYTES, MAX_IMPORT_ENVELOPE_BYTES, MAX_IMPORT_KEY_BYTES, MAX_IMPORT_PAYLOAD_BYTES,
    MAX_IMPORT_RECORDS,
};
pub use schema::{Collection, OwnedRecordKey, RecordKey, STORE_SCHEMA_VERSION, StoreKind};
pub use store::{ReadTransaction, Store, WriteTransaction};

#[cfg(test)]
mod envelope_test;
#[cfg(test)]
mod import_test;
#[cfg(test)]
mod schema_test;
#[cfg(test)]
mod store_test;
