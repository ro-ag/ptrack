use std::fmt;

use ptrack_capability_policy::{AuditEvent, sanitize_audit};
use ptrack_core::Capability;
use ptrack_store::ProjectStore;

/// Capability-owned durable audit sink. Only sanitized opaque records cross
/// the store boundary.
pub struct AuditRecorder<'a> {
    backend: AuditBackend<'a>,
}

enum AuditBackend<'a> {
    None,
    Store(&'a ProjectStore),
    Sink(&'a dyn AuditSink),
}

pub(crate) trait AuditSink: Send + Sync {
    fn record(&self, capability: &Capability, event: &AuditEvent) -> Result<(), AuditError>;
}

impl<'a> AuditRecorder<'a> {
    #[must_use]
    pub const fn new(store: Option<&'a ProjectStore>) -> Self {
        Self {
            backend: match store {
                Some(store) => AuditBackend::Store(store),
                None => AuditBackend::None,
            },
        }
    }

    pub(crate) const fn from_sink(sink: &'a dyn AuditSink) -> Self {
        Self {
            backend: AuditBackend::Sink(sink),
        }
    }

    /// Records one bounded audit event when enabled.
    ///
    /// # Errors
    /// Returns a sanitized storage error without event data.
    pub fn record(&self, capability: &Capability, event: &AuditEvent) -> Result<(), AuditError> {
        if !capability.audit.enabled {
            return Ok(());
        }
        match self.backend {
            AuditBackend::None => Ok(()),
            AuditBackend::Sink(sink) => sink.record(capability, event),
            AuditBackend::Store(store) => {
                let Some(audit) = sanitize_audit(capability, event) else {
                    return Ok(());
                };
                store
                    .record_capability_audit(audit)
                    .map(|_| ())
                    .map_err(|_| AuditError)
            }
        }
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
