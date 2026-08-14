//! Strict validation for ptrack's explicit JSON database staging format.
//!
//! The importer never opens or activates legacy databases. It consumes an
//! immutable JSON stage and creates verified, inert redb candidates only.

mod activation;
mod error;
mod manifest;
mod sha256;
mod stage;
mod wire;
mod workflow;

pub use activation::{ActivationReceipt, activate_stage, rollback_activation};
pub use error::{ImportError, ImportResult};
pub use stage::{StageReport, validate_stage};
pub use workflow::{ImportReceipt, import_stage};

#[cfg(test)]
mod activation_test;
#[cfg(test)]
mod manifest_test;
#[cfg(test)]
mod stage_test;
#[cfg(test)]
mod workflow_test;
