use std::fmt;

use crate::model::CAPABILITY_MODEL_VERSION;
use crate::{
    Capability, CapabilityAudit, CapabilityKind, Commit, Issue, MemoryKind, MemoryWritebackRecord,
    Meta, Milestone, NativeRecord, Note, NoteTarget, Plan, ProjectRef, Task, Timestamp,
};

const MAX_APPROVAL_SECONDS: i64 = 30 * 24 * 60 * 60;
const MAX_TIMEOUT_SECONDS: i64 = 300;
const MAX_TRANSFER_BYTES: i64 = 32 * 1024 * 1024;
const MAX_REDIRECTS: i64 = 10;
const MAX_CONCURRENT: i64 = 8;
const MAX_AUDIT_RECORDS: i64 = 1_000;

/// A stable field-level reason a native record cannot be trusted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationError {
    field: &'static str,
    reason: &'static str,
}

impl ValidationError {
    const fn new(field: &'static str, reason: &'static str) -> Self {
        Self { field, reason }
    }

    /// Returns the rejected field path.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        self.field
    }

    /// Returns the non-sensitive rejection reason.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.field, self.reason)
    }
}

impl std::error::Error for ValidationError {}

/// Fail-closed semantic validation for persistent values.
pub trait Validate {
    /// Rejects values that cannot be safely admitted to native storage.
    ///
    /// # Errors
    ///
    /// Returns a field-level reason when the value is not safe to persist.
    fn validate(&self) -> Result<(), ValidationError>;
}

fn require_id(value: u64, field: &'static str) -> Result<(), ValidationError> {
    if value == 0 {
        Err(ValidationError::new(field, "must be nonzero"))
    } else {
        Ok(())
    }
}

fn require_nonnegative(value: i64, field: &'static str) -> Result<(), ValidationError> {
    if value < 0 {
        Err(ValidationError::new(field, "must be nonnegative"))
    } else {
        Ok(())
    }
}

fn require_nonempty(value: &str, field: &'static str) -> Result<(), ValidationError> {
    if value.is_empty() {
        Err(ValidationError::new(field, "must be nonempty"))
    } else {
        Ok(())
    }
}

impl Validate for Timestamp {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Self::Fixed {
            nanoseconds,
            offset_seconds,
            ..
        } = self
        {
            if *nanoseconds >= 1_000_000_000 {
                return Err(ValidationError::new(
                    "timestamp.nanoseconds",
                    "must be below one second",
                ));
            }
            if !(-86_400..=86_400).contains(offset_seconds) {
                return Err(ValidationError::new(
                    "timestamp.offset_seconds",
                    "must be within 24 hours of UTC",
                ));
            }
        }
        Ok(())
    }
}

fn validate_times(values: &[Timestamp]) -> Result<(), ValidationError> {
    for value in values {
        value.validate()?;
    }
    Ok(())
}

impl Validate for Meta {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_times(&[self.created_at, self.updated_at])?;
        if self.format_version > 5 {
            return Err(ValidationError::new(
                "meta.format_version",
                "must be a supported legacy format from 0 through 5",
            ));
        }
        Ok(())
    }
}

impl Validate for Milestone {
    fn validate(&self) -> Result<(), ValidationError> {
        require_id(self.id, "milestone.id")?;
        require_nonnegative(self.order, "milestone.order")?;
        validate_times(&[self.due, self.created_at, self.updated_at])
    }
}

impl Validate for Plan {
    fn validate(&self) -> Result<(), ValidationError> {
        require_id(self.id, "plan.id")?;
        require_nonnegative(self.order, "plan.order")?;
        validate_times(&[self.created_at, self.updated_at])
    }
}

impl Validate for Task {
    fn validate(&self) -> Result<(), ValidationError> {
        require_id(self.id, "task.id")?;
        require_id(self.plan_id, "task.plan_id")?;
        require_nonnegative(self.order, "task.order")?;
        validate_times(&[self.created_at, self.updated_at])
    }
}

impl Validate for Note {
    fn validate(&self) -> Result<(), ValidationError> {
        require_id(self.id, "note.id")?;
        match self.target {
            NoteTarget::Project if self.target_id != 0 => {
                return Err(ValidationError::new(
                    "note.target_id",
                    "must be zero for project notes",
                ));
            }
            NoteTarget::Plan | NoteTarget::Task if self.target_id == 0 => {
                return Err(ValidationError::new(
                    "note.target_id",
                    "must be nonzero for plan or task notes",
                ));
            }
            _ => {}
        }
        if self.kind == MemoryKind::Summary {
            return Err(ValidationError::new(
                "note.kind",
                "must not contain the summary command kind",
            ));
        }
        self.created_at.validate()
    }
}

impl Validate for Issue {
    fn validate(&self) -> Result<(), ValidationError> {
        require_id(self.id, "issue.id")?;
        validate_times(&[self.created_at, self.updated_at])
    }
}

impl Validate for Commit {
    fn validate(&self) -> Result<(), ValidationError> {
        require_id(self.id, "commit.id")?;
        self.created_at.validate()
    }
}

fn validate_capability_scope(capability: &Capability) -> Result<(), ValidationError> {
    let matches = match capability.kind {
        CapabilityKind::Http => {
            capability.http.is_some() && capability.git.is_none() && capability.ssh.is_none()
        }
        CapabilityKind::Git => {
            capability.http.is_none() && capability.git.is_some() && capability.ssh.is_none()
        }
        CapabilityKind::Ssh => {
            capability.http.is_none() && capability.git.is_none() && capability.ssh.is_some()
        }
    };
    if !matches {
        return Err(ValidationError::new(
            "capability.scope",
            "must contain exactly the scope matching its kind",
        ));
    }
    match capability.kind {
        CapabilityKind::Http => {
            let scope = capability.http.as_ref().expect("matching scope checked");
            require_nonempty(&scope.base_url, "capability.http.base_url")?;
            if scope.methods.is_empty() {
                return Err(ValidationError::new(
                    "capability.http.methods",
                    "must be nonempty",
                ));
            }
            if scope.path_prefixes.is_empty() {
                return Err(ValidationError::new(
                    "capability.http.path_prefixes",
                    "must be nonempty",
                ));
            }
        }
        CapabilityKind::Git => {
            let scope = capability.git.as_ref().expect("matching scope checked");
            require_nonempty(&scope.remote_name, "capability.git.remote_name")?;
            require_nonempty(&scope.remote_url, "capability.git.remote_url")?;
            if scope.operations.is_empty() {
                return Err(ValidationError::new(
                    "capability.git.operations",
                    "must be nonempty",
                ));
            }
        }
        CapabilityKind::Ssh => {
            let scope = capability.ssh.as_ref().expect("matching scope checked");
            require_nonempty(&scope.host, "capability.ssh.host")?;
            require_nonempty(&scope.user, "capability.ssh.user")?;
            require_nonempty(&scope.host_key, "capability.ssh.host_key")?;
            if scope.port == 0 {
                return Err(ValidationError::new(
                    "capability.ssh.port",
                    "must be nonzero",
                ));
            }
            if scope.allow_interactive_shell {
                return Err(ValidationError::new(
                    "capability.ssh.allow_interactive_shell",
                    "is reserved and must be false",
                ));
            }
        }
    }
    Ok(())
}

impl Validate for Capability {
    fn validate(&self) -> Result<(), ValidationError> {
        require_id(self.id, "capability.id")?;
        require_id(self.revision, "capability.revision")?;
        if self.model_version != CAPABILITY_MODEL_VERSION {
            return Err(ValidationError::new(
                "capability.model_version",
                "must equal 1",
            ));
        }
        require_nonempty(&self.name, "capability.name")?;
        require_nonempty(&self.agent_profile, "capability.agent_profile")?;
        if self.scope_digest.is_empty() {
            return Err(ValidationError::new(
                "capability.scope_digest",
                "must be nonempty",
            ));
        }
        if !(60..=MAX_APPROVAL_SECONDS).contains(&self.approval_duration_seconds) {
            return Err(ValidationError::new(
                "capability.approval_duration_seconds",
                "is outside the supported range",
            ));
        }
        let limits = &self.limits;
        if !(1..=MAX_TIMEOUT_SECONDS).contains(&limits.timeout_seconds) {
            return Err(ValidationError::new(
                "capability.limits.timeout_seconds",
                "is outside the supported range",
            ));
        }
        for (field, value) in [
            (
                "capability.limits.max_request_bytes",
                limits.max_request_bytes,
            ),
            (
                "capability.limits.max_response_bytes",
                limits.max_response_bytes,
            ),
            (
                "capability.limits.max_output_bytes",
                limits.max_output_bytes,
            ),
        ] {
            if !(1..=MAX_TRANSFER_BYTES).contains(&value) {
                return Err(ValidationError::new(
                    field,
                    "is outside the supported range",
                ));
            }
        }
        if !(0..=MAX_REDIRECTS).contains(&limits.max_redirects) {
            return Err(ValidationError::new(
                "capability.limits.max_redirects",
                "is outside the supported range",
            ));
        }
        if !(1..=MAX_CONCURRENT).contains(&limits.max_concurrent) {
            return Err(ValidationError::new(
                "capability.limits.max_concurrent",
                "is outside the supported range",
            ));
        }
        if !(0..=MAX_AUDIT_RECORDS).contains(&self.audit.retain_last) {
            return Err(ValidationError::new(
                "capability.audit.retain_last",
                "is outside the supported range",
            ));
        }
        validate_capability_scope(self)?;
        validate_times(&[
            self.approved_at,
            self.expires_at,
            self.created_at,
            self.updated_at,
        ])?;

        if self.enabled {
            let approved = self.approved_at.unix_nanoseconds().ok_or_else(|| {
                ValidationError::new("capability.approved_at", "must be set when enabled")
            })?;
            let expires = self.expires_at.unix_nanoseconds().ok_or_else(|| {
                ValidationError::new("capability.expires_at", "must be set when enabled")
            })?;
            let maximum = approved + i128::from(self.approval_duration_seconds) * 1_000_000_000;
            if expires <= approved || expires > maximum {
                return Err(ValidationError::new(
                    "capability.expires_at",
                    "must follow approval without exceeding its duration",
                ));
            }
        } else if !self.approved_at.is_zero() || !self.expires_at.is_zero() {
            return Err(ValidationError::new(
                "capability.approval",
                "must be cleared when disabled",
            ));
        }
        Ok(())
    }
}

impl Validate for CapabilityAudit {
    fn validate(&self) -> Result<(), ValidationError> {
        require_id(self.id, "capability_audit.id")?;
        require_id(self.capability_id, "capability_audit.capability_id")?;
        require_nonnegative(self.duration_millis, "capability_audit.duration_millis")?;
        require_nonnegative(self.request_bytes, "capability_audit.request_bytes")?;
        require_nonnegative(self.response_bytes, "capability_audit.response_bytes")?;
        require_nonnegative(self.redirects, "capability_audit.redirects")?;
        if self.success && self.error_class != "none" {
            return Err(ValidationError::new(
                "capability_audit.error_class",
                "must be none on success",
            ));
        }
        self.created_at.validate()
    }
}

impl Validate for MemoryWritebackRecord {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.digest.is_empty() {
            return Err(ValidationError::new(
                "memory_writeback.digest",
                "must be nonempty",
            ));
        }
        require_id(self.sequence, "memory_writeback.sequence")?;
        match self.kind {
            MemoryKind::Summary if self.note_id != 0 => Err(ValidationError::new(
                "memory_writeback.note_id",
                "must be zero for summary writes",
            )),
            MemoryKind::Decision | MemoryKind::Blocker | MemoryKind::Handoff
                if self.note_id == 0 =>
            {
                Err(ValidationError::new(
                    "memory_writeback.note_id",
                    "must be nonzero for typed notes",
                ))
            }
            MemoryKind::Legacy => Err(ValidationError::new(
                "memory_writeback.kind",
                "must be a write-back command kind",
            )),
            _ => Ok(()),
        }
    }
}

impl Validate for ProjectRef {
    fn validate(&self) -> Result<(), ValidationError> {
        require_nonempty(&self.name, "project_ref.name")?;
        require_nonempty(&self.path, "project_ref.path")?;
        self.last_seen.validate()
    }
}

impl Validate for NativeRecord {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Meta(value) => value.validate(),
            Self::Plan(value) => value.validate(),
            Self::Task(value) => value.validate(),
            Self::Note(value) => value.validate(),
            Self::Milestone(value) => value.validate(),
            Self::Issue(value) => value.validate(),
            Self::Commit(value) => value.validate(),
            Self::Capability(value) => value.validate(),
            Self::CapabilityAudit(value) => value.validate(),
            Self::MemoryWriteback(value) => value.validate(),
            Self::ProjectRef(value) => value.validate(),
        }
    }
}
