#![forbid(unsafe_code)]

mod envelope;
mod error;
mod schema;
mod store;

pub use envelope::{RECORD_ENVELOPE_VERSION, RecordEnvelope};
pub use error::{EnvelopeError, StoreError, StoreResult};
pub use schema::{Collection, OwnedRecordKey, RecordKey, STORE_SCHEMA_VERSION, StoreKind};
pub use store::{ReadTransaction, Store, WriteTransaction};

#[cfg(test)]
mod envelope_test;
#[cfg(test)]
mod schema_test;
#[cfg(test)]
mod store_test;
