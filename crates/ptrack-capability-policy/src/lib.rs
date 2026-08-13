#![forbid(unsafe_code)]

//! Pure deny-by-default capability policy.
//!
//! This crate owns canonical scope normalization, Go-compatible approval
//! digests, and authorization decisions. It has no network, subprocess,
//! database, terminal, or UI authority.

mod audit;
mod authority;
mod normalize;
mod policy;
mod wire;

pub use normalize::{CapabilityError, Preview, normalize, normalize_remote_path};
pub use policy::{
    Denied, GitAuthorization, SshOperation, approve, authorize, authorize_git, authorize_http,
    authorize_ssh, disable, resolve_project_path,
};
pub use wire::{CapabilityAuditWire, CapabilityDraftWire, CapabilityWire, PreviewWire, WireError};

#[cfg(test)]
mod audit_test;
#[cfg(test)]
mod contract_coverage_test;
#[cfg(test)]
mod fixture_test;
#[cfg(test)]
mod normalize_test;
#[cfg(test)]
mod policy_test;
#[cfg(test)]
mod wire_test;
pub use audit::{AuditEvent, SanitizedAudit, sanitize_audit};
pub use authority::{ApprovalProof, confirm_approval};
