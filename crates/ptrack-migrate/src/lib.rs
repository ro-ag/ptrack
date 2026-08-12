//! Strict validation for the retired PTRKMIG1 compatibility bundle.
//!
//! The embedded SHA-256 detects accidental corruption. It is an integrity
//! check, not authentication of a potentially malicious bundle producer.

mod bundle;
mod sha256;

pub use bundle::{Bucket, BundleError, BundleKind, Record, ValidatedBundle, validate_path};

#[cfg(test)]
mod bundle_test;
#[cfg(test)]
mod sha256_test;
