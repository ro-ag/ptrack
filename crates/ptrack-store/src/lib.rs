#![forbid(unsafe_code)]

mod activation;
mod backup;
mod bounded;
mod discovery;
mod envelope;
mod error;
mod global;
mod import;
mod paths;
mod project;
mod quarantine;
mod schema;
mod sha256;
mod store;
mod typed;
mod validation;

pub use activation::{ActivatedStore, ActiveBinding, StagedStore};
pub use bounded::{
    Bounded, MAX_ASSOCIATION_SCAN, MAX_BOUNDED_READ, ScanBounded, TaskAssociations, TaskProgress,
};
pub use discovery::{
    GLOBAL_DATABASE_FILENAME, PROJECT_DATABASE_FILENAME, PROJECT_DIRECTORY, find_project_database,
    global_home_from, init_project_directory,
};
pub use envelope::{
    LEGACY_CODEC_GO_GOB, LEGACY_CODEC_RAW, NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA,
    RECORD_ENVELOPE_VERSION, RecordEnvelope,
};
pub use error::{EnvelopeError, StoreError, StoreResult};
pub use global::GlobalStore;
pub use import::{
    IMPORT_BUNDLE_VERSION, ImportCollection, ImportCollectionReport, ImportData, ImportProvenance,
    ImportRecord, ImportReport, JSON_STAGE_VERSION, JsonStageImportData, JsonStageProvenance,
    MAX_IMPORT_BYTES, MAX_IMPORT_ENVELOPE_BYTES, MAX_IMPORT_KEY_BYTES, MAX_IMPORT_PAYLOAD_BYTES,
    MAX_IMPORT_RECORDS,
};
pub use project::{
    CAPABILITY_AUDIT_GLOBAL_LIMIT, CURRENT_PROJECT_FORMAT, MEMORY_WRITEBACK_REPLAY_LIMIT,
    MemoryWriteRequest, MemoryWriteResult, ProjectStore,
};
pub use quarantine::{QuarantineReason, QuarantinedLegacyRecord};
pub use schema::{Collection, OwnedRecordKey, RecordKey, STORE_SCHEMA_VERSION, StoreKind};
pub use store::{ReadTransaction, Store, WriteTransaction};
pub use typed::{Clock, SystemClock};

#[cfg(test)]
mod backup_test_support;
#[cfg(test)]
mod discovery_test;
#[cfg(test)]
mod envelope_test;
#[cfg(test)]
mod import_test;
#[cfg(test)]
mod project_test;
#[cfg(test)]
mod project_test_support;
#[cfg(test)]
mod quarantine_test;
#[cfg(test)]
mod schema_test;
#[cfg(test)]
mod sha256_test;
#[cfg(test)]
mod store_test;
#[cfg(test)]
mod store_test_support;
#[cfg(test)]
mod validation_test;
