use std::fmt;
use std::sync::OnceLock;

use regex::Regex;

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
const MAX_AUDIT_DURATION_MILLIS: i64 = 24 * 60 * 60 * 1_000;
const MAX_AUDIT_BYTES: i64 = 1 << 40;
const MAX_AUDIT_TARGET_BYTES: usize = 256;

/// Maximum accepted UTF-8 bytes in a plan or task hold reason.
///
/// A hold reason is plain single-line prose meant for a list column and a
/// one-line status banner, not a place to park a document. The bound is a
/// deliberate new limit rather than a value borrowed from another field: it is
/// generous enough for a sentence explaining a blocker and small enough that a
/// hold reason can never dominate a record payload.
pub const MAX_HOLD_REASON_BYTES: usize = 1024;

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

/// The ways a hold reason can be unusable, shared by the record validator and
/// the input-boundary check so the two can never disagree about what is
/// storable.
enum HoldReasonProblem {
    Blank,
    TooLong,
    ControlCharacters,
}

fn hold_reason_problem(reason: &str) -> Option<HoldReasonProblem> {
    if reason.trim().is_empty() {
        return Some(HoldReasonProblem::Blank);
    }
    if reason.len() > MAX_HOLD_REASON_BYTES {
        return Some(HoldReasonProblem::TooLong);
    }
    if reason.chars().any(is_forbidden_control) {
        return Some(HoldReasonProblem::ControlCharacters);
    }
    None
}

/// Reports whether a character would break a hold reason out of single-line
/// plain text.
///
/// `char::is_control` covers the C0 and C1 blocks but not the Unicode
/// separators U+2028 and U+2029, which terminate a line; the bidirectional
/// formatting controls U+202A-U+202E and U+2066-U+2069; the directional marks
/// U+200E (LRM), U+200F (RLM), and U+061C (ALM); the zero-width characters
/// U+200B-U+200D (zero-width space, non-joiner, and joiner), U+FEFF (zero-width
/// no-break space / byte order mark), U+2060 (word joiner), and U+180E (Mongolian
/// vowel separator); or the tag block U+E0000-U+E007F, whose invisible ASCII
/// mirror can smuggle a whole second sentence into a reason — all of which can
/// reorder or hide what a reason really says without showing up as a visible
/// character.
fn is_forbidden_control(value: char) -> bool {
    value.is_control()
        || matches!(
            value,
            '\u{2028}'
                | '\u{2029}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
                | '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{feff}'
                | '\u{2060}'
                | '\u{180e}'
                | '\u{e0000}'..='\u{e007f}'
        )
}

/// Rejects a set hold reason that is blank, oversized, or not single-line text.
fn validate_hold_reason(
    value: Option<&String>,
    field: &'static str,
) -> Result<(), ValidationError> {
    match value.and_then(|reason| hold_reason_problem(reason)) {
        None => Ok(()),
        Some(HoldReasonProblem::Blank) => {
            Err(ValidationError::new(field, "must be nonblank when set"))
        }
        Some(HoldReasonProblem::TooLong) => {
            Err(ValidationError::new(field, "exceeds the hold reason bound"))
        }
        Some(HoldReasonProblem::ControlCharacters) => Err(ValidationError::new(
            field,
            "must be single-line text without control characters",
        )),
    }
}

/// Checks a hold reason typed by a person, before it reaches the store.
///
/// The record validator fires deep inside `encode_record`, so without this the
/// user would see a field-path message such as
/// `plan.hold_reason must be nonblank when set`. Both checks share
/// `hold_reason_problem`, so anything accepted here is storable.
///
/// # Errors
///
/// Returns a printable sentence when the reason cannot be stored.
pub fn check_hold_reason(reason: &str) -> Result<(), String> {
    match hold_reason_problem(reason) {
        None => Ok(()),
        Some(HoldReasonProblem::Blank) => Err("the hold reason cannot be blank".to_owned()),
        Some(HoldReasonProblem::TooLong) => Err(format!(
            "the hold reason is {} bytes; the limit is {MAX_HOLD_REASON_BYTES}",
            reason.len()
        )),
        Some(HoldReasonProblem::ControlCharacters) => {
            Err("the hold reason must be one line without control characters".to_owned())
        }
    }
}

/// Maximum accepted UTF-8 bytes in a user identity display name.
///
/// A display name is a short single-line label rendered next to claims and
/// attribution markers; it shares the hold-reason forbidden-character set so
/// the two single-line rules can never diverge.
pub const MAX_IDENTITY_NAME_BYTES: usize = 64;

/// The presentation sentinel for records whose actor is unset.
///
/// Storage keeps `actor: None`; JSON adapters render this sentinel instead of
/// null so consumers never disambiguate two "absent" spellings. It can never
/// collide with a real actor: [`is_identity_id`] rejects it.
pub const LEGACY_ACTOR: &str = "legacy";

const IDENTITY_ID_ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Reports whether a value is a well-formed 26-character lowercase
/// Crockford-base32 identity ID (the exact shape `ptrack config set user`
/// mints, also used by the reserved entity ULID fields).
#[must_use]
pub fn is_identity_id(value: &str) -> bool {
    value.len() == 26
        && value
            .bytes()
            .all(|byte| IDENTITY_ID_ALPHABET.contains(&byte))
}

/// Checks a display name typed by a person, before it reaches storage.
///
/// # Errors
///
/// Returns a printable sentence when the name cannot be stored.
pub fn check_identity_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("the user name cannot be blank".to_owned());
    }
    if name.len() > MAX_IDENTITY_NAME_BYTES {
        return Err(format!(
            "the user name is {} bytes; the limit is {MAX_IDENTITY_NAME_BYTES}",
            name.len()
        ));
    }
    if name.chars().any(is_forbidden_control) {
        return Err("the user name must be one line without control characters".to_owned());
    }
    Ok(())
}

/// Rejects a set identity-shaped field that is not a well-formed identity ID.
fn validate_identity_option(
    value: Option<&String>,
    field: &'static str,
) -> Result<(), ValidationError> {
    match value {
        Some(id) if !is_identity_id(id) => Err(ValidationError::new(
            field,
            "must be a 26-character identity id",
        )),
        _ => Ok(()),
    }
}

/// Rejects an actor-keyed map that is unsorted, duplicated, or not keyed by
/// well-formed identity IDs.
fn validate_actor_map_keys<T>(
    entries: &[(String, T)],
    field: &'static str,
) -> Result<(), ValidationError> {
    for window in entries.windows(2) {
        if window[0].0 >= window[1].0 {
            return Err(ValidationError::new(
                field,
                "must be sorted strictly ascending by identity id",
            ));
        }
    }
    for (id, _) in entries {
        if !is_identity_id(id) {
            return Err(ValidationError::new(field, "must key by identity ids"));
        }
    }
    Ok(())
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
        validate_actor_map_keys(&self.active_plans, "meta.active_plans")?;
        validate_actor_map_keys(&self.actors, "meta.actors")?;
        // The directory holds names typed by people, so it reuses the same
        // single-line rule the input boundary applies to a display name.
        if self
            .actors
            .iter()
            .any(|(_, name)| check_identity_name(name).is_err())
        {
            return Err(ValidationError::new(
                "meta.actors",
                "must hold bounded single-line names",
            ));
        }
        Ok(())
    }
}

impl Validate for Milestone {
    fn validate(&self) -> Result<(), ValidationError> {
        require_id(self.id, "milestone.id")?;
        require_nonnegative(self.order, "milestone.order")?;
        validate_identity_option(self.actor.as_ref(), "milestone.actor")?;
        validate_identity_option(self.ulid.as_ref(), "milestone.ulid")?;
        validate_times(&[self.due, self.created_at, self.updated_at])
    }
}

impl Validate for Plan {
    fn validate(&self) -> Result<(), ValidationError> {
        require_id(self.id, "plan.id")?;
        require_nonnegative(self.order, "plan.order")?;
        validate_hold_reason(self.hold_reason.as_ref(), "plan.hold_reason")?;
        validate_identity_option(self.actor.as_ref(), "plan.actor")?;
        validate_identity_option(self.ulid.as_ref(), "plan.ulid")?;
        validate_identity_option(self.claim_owner.as_ref(), "plan.claim_owner")?;
        // Claim consistency: every owner arrived through a claim that bumped
        // the epoch, and the conflict marker only annotates a live claim. A
        // release clears the owner and keeps the epoch, so a nonzero epoch
        // without an owner is the normal released shape.
        if self.claim_owner.is_some() && self.claim_epoch == 0 {
            return Err(ValidationError::new(
                "plan.claim_epoch",
                "must be nonzero while the plan is claimed",
            ));
        }
        if self.claim_conflict && self.claim_owner.is_none() {
            return Err(ValidationError::new(
                "plan.claim_conflict",
                "must not be set on an unclaimed plan",
            ));
        }
        validate_times(&[self.created_at, self.updated_at])
    }
}

impl Validate for Task {
    fn validate(&self) -> Result<(), ValidationError> {
        require_id(self.id, "task.id")?;
        require_id(self.plan_id, "task.plan_id")?;
        require_nonnegative(self.order, "task.order")?;
        validate_hold_reason(self.hold_reason.as_ref(), "task.hold_reason")?;
        validate_identity_option(self.actor.as_ref(), "task.actor")?;
        validate_identity_option(self.ulid.as_ref(), "task.ulid")?;
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
        validate_identity_option(self.actor.as_ref(), "note.actor")?;
        validate_identity_option(self.ulid.as_ref(), "note.ulid")?;
        self.created_at.validate()
    }
}

impl Validate for Issue {
    fn validate(&self) -> Result<(), ValidationError> {
        require_id(self.id, "issue.id")?;
        validate_identity_option(self.actor.as_ref(), "issue.actor")?;
        validate_identity_option(self.ulid.as_ref(), "issue.ulid")?;
        validate_times(&[self.created_at, self.updated_at])
    }
}

impl Validate for Commit {
    fn validate(&self) -> Result<(), ValidationError> {
        require_id(self.id, "commit.id")?;
        validate_identity_option(self.actor.as_ref(), "commit.actor")?;
        validate_identity_option(self.ulid.as_ref(), "commit.ulid")?;
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
        if self.duration_millis > MAX_AUDIT_DURATION_MILLIS {
            return Err(ValidationError::new(
                "capability_audit.duration_millis",
                "exceeds the metadata bound",
            ));
        }
        if self.request_bytes > MAX_AUDIT_BYTES || self.response_bytes > MAX_AUDIT_BYTES {
            return Err(ValidationError::new(
                "capability_audit.bytes",
                "exceeds the metadata bound",
            ));
        }
        if self.redirects > MAX_REDIRECTS {
            return Err(ValidationError::new(
                "capability_audit.redirects",
                "exceeds the metadata bound",
            ));
        }
        if !valid_audit_profile(&self.agent_profile) {
            return Err(ValidationError::new(
                "capability_audit.agent_profile",
                "must be sanitized",
            ));
        }
        if !valid_audit_operation(self.kind, &self.operation) {
            return Err(ValidationError::new(
                "capability_audit.operation",
                "must be sanitized",
            ));
        }
        if self.target.is_empty()
            || self.target.len() > MAX_AUDIT_TARGET_BYTES
            || self.target.chars().any(char::is_control)
        {
            return Err(ValidationError::new(
                "capability_audit.target",
                "must be sanitized",
            ));
        }
        let valid_error_class = if self.success {
            self.error_class == "none"
        } else {
            matches!(
                self.error_class.as_str(),
                "denied"
                    | "dns"
                    | "routing"
                    | "vpn"
                    | "proxy"
                    | "tls"
                    | "host-key"
                    | "authentication"
                    | "sandbox"
                    | "remote-policy"
                    | "timeout"
                    | "transport"
                    | "request-limit"
                    | "response-limit"
                    | "output-limit"
                    | "cancelled"
                    | "internal"
            )
        };
        if !valid_error_class {
            return Err(ValidationError::new(
                "capability_audit.error_class",
                "is not an allowlisted class for its outcome",
            ));
        }
        self.created_at.validate()
    }
}

fn valid_audit_profile(value: &str) -> bool {
    value == "unknown-profile"
        || (!value.is_empty()
            && value.len() <= 64
            && !value.starts_with('-')
            && value.chars().all(|character| {
                audit_letter_or_decimal_digit(character) || matches!(character, '.' | '_' | '-')
            }))
}

fn audit_letter_or_decimal_digit(character: char) -> bool {
    static VALUE: OnceLock<Regex> = OnceLock::new();
    let mut buffer = [0; 4];
    VALUE
        .get_or_init(|| Regex::new(r"^[\p{L}\p{Nd}]$").expect("static Unicode regex"))
        .is_match(character.encode_utf8(&mut buffer))
}

fn valid_audit_operation(kind: CapabilityKind, value: &str) -> bool {
    let http_operation = value.to_ascii_lowercase();
    value == "unknown"
        || match kind {
            CapabilityKind::Http => matches!(
                http_operation.as_str(),
                "get" | "head" | "post" | "put" | "patch" | "delete" | "options" | "test"
            ),
            CapabilityKind::Git => {
                matches!(
                    value,
                    "status" | "fetch" | "pull" | "push" | "ls-remote" | "test"
                )
            }
            CapabilityKind::Ssh => matches!(
                value,
                "git"
                    | "remote-command"
                    | "upload"
                    | "download"
                    | "interactive-shell"
                    | "local-forward"
                    | "remote-forward"
                    | "test"
            ),
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
