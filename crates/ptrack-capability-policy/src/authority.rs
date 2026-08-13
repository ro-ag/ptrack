use ptrack_core::{Capability, Digest32};

use crate::{CapabilityError, normalize};

/// Opaque evidence that the exact persisted revision was normalized and its
/// effective scope digest was explicitly confirmed.
#[derive(Clone, Debug)]
pub struct ApprovalProof {
    capability_id: u64,
    revision: u64,
    digest: Digest32,
}

/// Recomputes policy over an exact stored record and confirms its preview.
///
/// # Errors
/// Returns an error when the record is not normalized exactly as stored or the
/// confirmed digest is empty or stale.
pub fn confirm_approval(
    stored: &Capability,
    expected_digest: Digest32,
) -> Result<ApprovalProof, CapabilityError> {
    let preview = normalize(stored)?;
    if expected_digest.is_empty()
        || stored.scope_digest != preview.scope_digest
        || expected_digest != preview.scope_digest
    {
        return Err(CapabilityError::message(
            "effective scope changed; preview again before enabling",
        ));
    }
    Ok(ApprovalProof {
        capability_id: stored.id,
        revision: stored.revision,
        digest: preview.scope_digest,
    })
}

impl ApprovalProof {
    /// Persisted identity fenced by this proof.
    #[doc(hidden)]
    #[must_use]
    pub const fn capability_id(&self) -> u64 {
        self.capability_id
    }

    /// Checks the transaction-local record against all proof fences.
    #[doc(hidden)]
    #[must_use]
    pub fn matches(&self, capability_id: u64, revision: u64, digest: Digest32) -> bool {
        self.capability_id == capability_id
            && self.revision == revision
            && self.digest.0 == digest.0
    }

    /// Revision used for a diagnostic CAS error.
    #[doc(hidden)]
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}
