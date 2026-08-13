use std::fmt;

use ptrack_capability_policy::{AuditEvent, sanitize_audit};
use ptrack_core::Capability;
use ptrack_store::ProjectStore;

/// Capability-owned durable audit sink. Only sanitized opaque records cross
/// the store boundary.
pub struct AuditRecorder<'a> {
    store: Option<&'a ProjectStore>,
}

impl<'a> AuditRecorder<'a> {
    #[must_use]
    pub const fn new(store: Option<&'a ProjectStore>) -> Self {
        Self { store }
    }

    /// Records one bounded audit event when enabled.
    ///
    /// # Errors
    /// Returns a sanitized storage error without event data.
    pub fn record(&self, capability: &Capability, event: &AuditEvent) -> Result<(), AuditError> {
        let Some(store) = self.store else {
            return Ok(());
        };
        let Some(audit) = sanitize_audit(capability, event) else {
            return Ok(());
        };
        store
            .record_capability_audit(audit)
            .map(|_| ())
            .map_err(|_| AuditError)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuditError;

impl fmt::Display for AuditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("record capability audit: internal")
    }
}

impl std::error::Error for AuditError {}
