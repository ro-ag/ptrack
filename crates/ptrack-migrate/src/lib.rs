//! Strict validation and explicit, one-way import for ptrack migration bundles.
//!
//! The embedded SHA-256 detects accidental corruption. It is an integrity
//! check, not authentication of a potentially malicious bundle producer.

mod adapter;
mod bundle;
mod sha256;

pub use adapter::{MigrationError, bundle_into_import_data, import_path, import_validated_bundle};
pub use bundle::{Bucket, BundleError, BundleKind, Record, ValidatedBundle, validate_path};
pub use ptrack_store::{ImportData, ImportReport, Store, StoreKind};

#[cfg(test)]
mod adapter_test;
#[cfg(test)]
mod bundle_test;
#[cfg(test)]
mod sha256_test;
