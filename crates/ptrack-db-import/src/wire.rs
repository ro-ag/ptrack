use std::collections::BTreeSet;

use ptrack_core::{
    Capability, CapabilityAudit, CapabilityAuditPolicy, CapabilityKind, CapabilityLimits, Commit,
    Digest32, GitScope, HttpScope, Issue, IssueStatus, MemoryKind, MemoryWritebackRecord, Meta,
    Milestone, MilestoneStatus, NativeRecord, Note, NoteTarget, Plan, PlanStatus, ProjectRef,
    Severity, SshScope, Task, TaskStatus, Timestamp, encode_record,
};
use ptrack_store::{QuarantineReason, QuarantinedLegacyRecord};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value as JsonValue;

use crate::error::{ImportError, ImportResult, invalid};
use crate::manifest::{decimal_i64, decimal_u64};

pub(crate) const PROJECT_COLLECTIONS: [&str; 10] = [
    "meta",
    "plans",
    "tasks",
    "notes",
    "milestones",
    "issues",
    "commits",
    "capabilities",
    "capability_audits",
    "memory_writebacks",
];
pub(crate) const GLOBAL_COLLECTIONS: [&str; 3] = ["config", "projects", "backups"];

#[derive(Debug)]
pub(crate) enum Line {
    Header(Header),
    Bucket(Bucket),
    Record(Record),
    Quarantine(Quarantine),
}

#[derive(Deserialize)]
struct LineKind {
    #[serde(rename = "type")]
    kind: String,
}

impl Line {
    pub(crate) fn decode(bytes: &[u8]) -> ImportResult<Self> {
        let kind: LineKind = serde_json::from_slice(bytes)
            .map_err(|error| ImportError::InvalidStage(format!("decode JSONL type: {error}")))?;
        match kind.kind.as_str() {
            "database" => serde_json::from_slice(bytes).map(Self::Header),
            "bucket" => serde_json::from_slice(bytes).map(Self::Bucket),
            "record" => serde_json::from_slice(bytes).map(Self::Record),
            "quarantine" => serde_json::from_slice(bytes).map(Self::Quarantine),
            _ => return invalid(format!("unknown JSONL line type {:?}", kind.kind)),
        }
        .map_err(|error| ImportError::InvalidStage(format!("decode JSONL line: {error}")))
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Header {
    #[serde(rename = "type")]
    pub(crate) _line_type: String,
    pub schema: String,
    pub database_id: String,
    pub kind: String,
    pub source_format: String,
    pub bucket_count: String,
    pub record_count: String,
    pub quarantine_count: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Bucket {
    #[serde(rename = "type")]
    pub(crate) _line_type: String,
    pub name: String,
    pub present: bool,
    pub sequence: Option<String>,
    pub record_count: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    #[serde(rename = "type")]
    pub(crate) _line_type: String,
    pub bucket: String,
    pub key: Key,
    pub source_value_sha256: String,
    pub model: String,
    pub model_version: String,
    pub value: JsonValue,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Quarantine {
    #[serde(rename = "type")]
    pub(crate) _line_type: String,
    pub bucket: String,
    pub key: Key,
    pub source_value_sha256: String,
    pub reason: String,
    pub legacy_codec: String,
    pub legacy_value_hex: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Key {
    pub encoding: String,
    pub value: String,
}

#[derive(Debug)]
pub(crate) struct ConvertedRecord {
    pub key: Vec<u8>,
    pub payload: Vec<u8>,
    pub raw: bool,
}

impl Record {
    pub(crate) fn convert(self) -> ImportResult<ConvertedRecord> {
        let _ = decode_digest(&self.source_value_sha256, "record.source_value_sha256")?;
        let key = self.key.decode_for(&self.bucket)?;
        let (payload, raw) = match self.model.as_str() {
            "meta" => native::<MetaValue>(self.value, NativeRecord::Meta)?,
            "plan" => native::<PlanValue>(self.value, NativeRecord::Plan)?,
            "task" => native::<TaskValue>(self.value, NativeRecord::Task)?,
            "note" => native::<NoteValue>(self.value, NativeRecord::Note)?,
            "milestone" => native::<MilestoneValue>(self.value, NativeRecord::Milestone)?,
            "issue" => native::<IssueValue>(self.value, NativeRecord::Issue)?,
            "commit" => native::<CommitValue>(self.value, NativeRecord::Commit)?,
            "capability" => native::<CapabilityValue>(self.value, NativeRecord::Capability)?,
            "capability_audit" => {
                native::<CapabilityAuditValue>(self.value, NativeRecord::CapabilityAudit)?
            }
            "memory_writeback" => {
                native::<MemoryWritebackValue>(self.value, NativeRecord::MemoryWriteback)?
            }
            "project_ref" => native::<ProjectRefValue>(self.value, NativeRecord::ProjectRef)?,
            "raw" => {
                let raw: RawRecordValue = from_value(self.value, "raw value")?;
                if raw.encoding != "hex" {
                    return invalid("raw record encoding must be hex");
                }
                (decode_hex(&raw.bytes, "raw bytes")?, true)
            }
            _ => return invalid(format!("unknown record model {:?}", self.model)),
        };
        let expected_version = if raw { "0" } else { "1" };
        if self.model_version != expected_version {
            return invalid("record model_version does not match its model");
        }
        validate_model_collection(&self.model, &self.bucket)?;
        validate_key_identity(&self.model, &key, &payload, raw)?;
        Ok(ConvertedRecord { key, payload, raw })
    }
}

impl Key {
    pub(crate) fn decode_for(&self, bucket: &str) -> ImportResult<Vec<u8>> {
        let key = match self.encoding.as_str() {
            "singleton" if self.value == "meta" => b"meta".to_vec(),
            "u64" => {
                let id = decimal_u64(&self.value, "key.value")?;
                if id == 0 {
                    return invalid("numeric record key must be nonzero");
                }
                id.to_be_bytes().to_vec()
            }
            "hex" => decode_hex(&self.value, "key.value")?,
            _ => return invalid("record key encoding/value is invalid"),
        };
        let expected = if bucket == "meta" {
            "singleton"
        } else if matches!(
            bucket,
            "plans"
                | "tasks"
                | "notes"
                | "milestones"
                | "issues"
                | "commits"
                | "capabilities"
                | "capability_audits"
        ) {
            "u64"
        } else {
            "hex"
        };
        if self.encoding != expected || key.is_empty() {
            return invalid(format!("bucket {bucket:?} uses the wrong key encoding"));
        }
        Ok(key)
    }
}

impl Quarantine {
    pub(crate) fn convert(self) -> ImportResult<(Vec<u8>, QuarantinedLegacyRecord)> {
        let reason = match (self.bucket.as_str(), self.reason.as_str()) {
            ("capabilities", "invalid_capability") => QuarantineReason::InvalidCapability,
            ("capability_audits", "invalid_capability_audit") => {
                QuarantineReason::InvalidCapabilityAudit
            }
            _ => return invalid("unsupported quarantine bucket or reason"),
        };
        if self.legacy_codec != "go-gob" {
            return invalid("unsupported quarantine bucket, reason, or legacy codec");
        }
        let key = self.key.decode_for(&self.bucket)?;
        let value = decode_hex(&self.legacy_value_hex, "quarantine legacy_value_hex")?;
        let mut hash = crate::sha256::Sha256::new();
        hash.update(&value);
        if crate::sha256::hex(hash.finish()) != self.source_value_sha256 {
            return invalid("quarantine source_value_sha256 does not match legacy gob bytes");
        }
        let source_value_sha256 =
            decode_digest(&self.source_value_sha256, "quarantine.source_value_sha256")?;
        Ok((
            key.clone(),
            QuarantinedLegacyRecord {
                source_bucket: self.bucket.into_bytes(),
                source_key: key,
                legacy_gob: value,
                source_value_sha256,
                reason,
            },
        ))
    }
}

fn native<T>(
    value: JsonValue,
    construct: impl FnOnce(T::Native) -> NativeRecord,
) -> ImportResult<(Vec<u8>, bool)>
where
    T: DeserializeOwned + IntoNative,
{
    let value: T = from_value(value, "typed record value")?;
    let record = construct(value.into_native()?);
    let payload = encode_record(&record)
        .map_err(|error| ImportError::InvalidStage(format!("invalid typed record: {error}")))?;
    Ok((payload, false))
}

trait IntoNative {
    type Native;
    fn into_native(self) -> ImportResult<Self::Native>;
}

fn from_value<T: DeserializeOwned>(value: JsonValue, name: &str) -> ImportResult<T> {
    serde_json::from_value(value)
        .map_err(|error| ImportError::InvalidStage(format!("decode {name}: {error}")))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimestampValue {
    state: String,
    #[serde(default)]
    unix_seconds: Option<String>,
    #[serde(default)]
    nanoseconds: Option<String>,
    #[serde(default)]
    utc_offset_seconds: Option<String>,
}

impl TimestampValue {
    fn into_native(self) -> ImportResult<Timestamp> {
        match (
            self.state.as_str(),
            self.unix_seconds,
            self.nanoseconds,
            self.utc_offset_seconds,
        ) {
            ("zero", None, None, None) => Ok(Timestamp::Zero),
            ("fixed", Some(seconds), Some(nanos), Some(offset)) => Ok(Timestamp::Fixed {
                seconds: decimal_i64(&seconds, "timestamp.unix_seconds")?,
                nanoseconds: u32::try_from(decimal_u64(&nanos, "timestamp.nanoseconds")?).map_err(
                    |_| ImportError::InvalidStage("timestamp nanoseconds exceeds u32".to_owned()),
                )?,
                offset_seconds: i32::try_from(decimal_i64(
                    &offset,
                    "timestamp.utc_offset_seconds",
                )?)
                .map_err(|_| {
                    ImportError::InvalidStage("timestamp offset exceeds i32".to_owned())
                })?,
            }),
            _ => invalid("timestamp union is inconsistent"),
        }
    }
}

macro_rules! u64_field {
    ($value:expr, $name:literal) => {
        decimal_u64(&$value, $name)?
    };
}
macro_rules! i64_field {
    ($value:expr, $name:literal) => {
        decimal_i64(&$value, $name)?
    };
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MetaValue {
    goal: String,
    summary: String,
    active_plan: String,
    created_at: TimestampValue,
    updated_at: TimestampValue,
    format_version: String,
    last_write_version: String,
}
impl IntoNative for MetaValue {
    type Native = Meta;
    fn into_native(self) -> ImportResult<Meta> {
        Ok(Meta {
            goal: self.goal,
            summary: self.summary,
            active_plan: u64_field!(self.active_plan, "meta.active_plan"),
            created_at: self.created_at.into_native()?,
            updated_at: self.updated_at.into_native()?,
            format_version: u64_field!(self.format_version, "meta.format_version"),
            last_write_version: self.last_write_version,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanValue {
    id: String,
    title: String,
    status: String,
    milestone_id: String,
    order: String,
    created_at: TimestampValue,
    updated_at: TimestampValue,
}
impl IntoNative for PlanValue {
    type Native = Plan;
    fn into_native(self) -> ImportResult<Plan> {
        Ok(Plan {
            id: u64_field!(self.id, "plan.id"),
            title: self.title,
            status: plan_status(&self.status)?,
            milestone_id: u64_field!(self.milestone_id, "plan.milestone_id"),
            order: i64_field!(self.order, "plan.order"),
            created_at: self.created_at.into_native()?,
            updated_at: self.updated_at.into_native()?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskValue {
    id: String,
    plan_id: String,
    title: String,
    status: String,
    order: String,
    created_at: TimestampValue,
    updated_at: TimestampValue,
}
impl IntoNative for TaskValue {
    type Native = Task;
    fn into_native(self) -> ImportResult<Task> {
        Ok(Task {
            id: u64_field!(self.id, "task.id"),
            plan_id: u64_field!(self.plan_id, "task.plan_id"),
            title: self.title,
            status: task_status(&self.status)?,
            order: i64_field!(self.order, "task.order"),
            created_at: self.created_at.into_native()?,
            updated_at: self.updated_at.into_native()?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteValue {
    id: String,
    target: String,
    target_id: String,
    kind: String,
    body: String,
    created_at: TimestampValue,
}
impl IntoNative for NoteValue {
    type Native = Note;
    fn into_native(self) -> ImportResult<Note> {
        Ok(Note {
            id: u64_field!(self.id, "note.id"),
            target: note_target(&self.target)?,
            target_id: u64_field!(self.target_id, "note.target_id"),
            kind: memory_kind(&self.kind)?,
            body: self.body,
            created_at: self.created_at.into_native()?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MilestoneValue {
    id: String,
    title: String,
    status: String,
    due: TimestampValue,
    order: String,
    created_at: TimestampValue,
    updated_at: TimestampValue,
}
impl IntoNative for MilestoneValue {
    type Native = Milestone;
    fn into_native(self) -> ImportResult<Milestone> {
        Ok(Milestone {
            id: u64_field!(self.id, "milestone.id"),
            title: self.title,
            status: milestone_status(&self.status)?,
            due: self.due.into_native()?,
            order: i64_field!(self.order, "milestone.order"),
            created_at: self.created_at.into_native()?,
            updated_at: self.updated_at.into_native()?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IssueValue {
    id: String,
    title: String,
    body: String,
    status: String,
    severity: String,
    task_id: String,
    created_at: TimestampValue,
    updated_at: TimestampValue,
}
impl IntoNative for IssueValue {
    type Native = Issue;
    fn into_native(self) -> ImportResult<Issue> {
        Ok(Issue {
            id: u64_field!(self.id, "issue.id"),
            title: self.title,
            body: self.body,
            status: issue_status(&self.status)?,
            severity: severity(&self.severity)?,
            task_id: u64_field!(self.task_id, "issue.task_id"),
            created_at: self.created_at.into_native()?,
            updated_at: self.updated_at.into_native()?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitValue {
    id: String,
    sha: String,
    subject: String,
    plan_id: String,
    task_id: String,
    created_at: TimestampValue,
}
impl IntoNative for CommitValue {
    type Native = Commit;
    fn into_native(self) -> ImportResult<Commit> {
        Ok(Commit {
            id: u64_field!(self.id, "commit.id"),
            sha: self.sha,
            subject: self.subject,
            plan_id: u64_field!(self.plan_id, "commit.plan_id"),
            task_id: u64_field!(self.task_id, "commit.task_id"),
            created_at: self.created_at.into_native()?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityLimitsValue {
    timeout_seconds: String,
    max_request_bytes: String,
    max_response_bytes: String,
    max_output_bytes: String,
    max_redirects: String,
    max_concurrent: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditPolicyValue {
    enabled: bool,
    retain_last: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpValue {
    base_url: String,
    methods: Vec<String>,
    path_prefixes: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GitValue {
    remote_name: String,
    remote_url: String,
    operations: Vec<String>,
    branches: Vec<String>,
    refspecs: Vec<String>,
    allow_tags: bool,
    allow_force_push: bool,
    allow_delete_refs: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
struct SshValue {
    alias: String,
    host: String,
    port: String,
    user: String,
    host_key: String,
    allow_git: bool,
    remote_commands: Vec<String>,
    allow_upload: bool,
    allow_download: bool,
    upload_roots: Vec<String>,
    download_roots: Vec<String>,
    upload_remote_roots: Vec<String>,
    download_remote_roots: Vec<String>,
    allow_interactive_shell: bool,
    local_forward_targets: Vec<String>,
    remote_forward_targets: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityValue {
    id: String,
    model_version: String,
    revision: String,
    name: String,
    kind: String,
    agent_profile: String,
    enabled: bool,
    approval_duration_seconds: String,
    approved_at: TimestampValue,
    expires_at: TimestampValue,
    scope_digest: String,
    limits: CapabilityLimitsValue,
    audit: AuditPolicyValue,
    http: Option<HttpValue>,
    git: Option<GitValue>,
    ssh: Option<SshValue>,
    created_at: TimestampValue,
    updated_at: TimestampValue,
    migration_disposition: String,
}
impl IntoNative for CapabilityValue {
    type Native = Capability;
    fn into_native(self) -> ImportResult<Capability> {
        if self.migration_disposition != "force_reapproval" {
            return invalid("capability migration_disposition must be force_reapproval");
        }
        let ssh = match self.ssh {
            Some(value) => Some(SshScope {
                alias: value.alias,
                host: value.host,
                port: u16::try_from(decimal_u64(&value.port, "ssh.port")?)
                    .map_err(|_| ImportError::InvalidStage("ssh.port exceeds u16".to_owned()))?,
                user: value.user,
                host_key: value.host_key,
                allow_git: value.allow_git,
                remote_commands: value.remote_commands,
                allow_upload: value.allow_upload,
                allow_download: value.allow_download,
                upload_roots: value.upload_roots,
                download_roots: value.download_roots,
                upload_remote_roots: value.upload_remote_roots,
                download_remote_roots: value.download_remote_roots,
                allow_interactive_shell: value.allow_interactive_shell,
                local_forward_targets: value.local_forward_targets,
                remote_forward_targets: value.remote_forward_targets,
            }),
            None => None,
        };
        let mut capability = Capability {
            id: u64_field!(self.id, "capability.id"),
            model_version: u64_field!(self.model_version, "capability.model_version"),
            revision: u64_field!(self.revision, "capability.revision"),
            name: self.name,
            kind: capability_kind(&self.kind)?,
            agent_profile: self.agent_profile,
            enabled: self.enabled,
            approval_duration_seconds: i64_field!(
                self.approval_duration_seconds,
                "capability.approval_duration_seconds"
            ),
            approved_at: self.approved_at.into_native()?,
            expires_at: self.expires_at.into_native()?,
            scope_digest: Digest32(decode_digest(
                &self.scope_digest,
                "capability.scope_digest",
            )?),
            limits: CapabilityLimits {
                timeout_seconds: i64_field!(self.limits.timeout_seconds, "limits.timeout_seconds"),
                max_request_bytes: i64_field!(
                    self.limits.max_request_bytes,
                    "limits.max_request_bytes"
                ),
                max_response_bytes: i64_field!(
                    self.limits.max_response_bytes,
                    "limits.max_response_bytes"
                ),
                max_output_bytes: i64_field!(
                    self.limits.max_output_bytes,
                    "limits.max_output_bytes"
                ),
                max_redirects: i64_field!(self.limits.max_redirects, "limits.max_redirects"),
                max_concurrent: i64_field!(self.limits.max_concurrent, "limits.max_concurrent"),
            },
            audit: CapabilityAuditPolicy {
                enabled: self.audit.enabled,
                retain_last: i64_field!(self.audit.retain_last, "audit.retain_last"),
            },
            http: self.http.map(|value| HttpScope {
                base_url: value.base_url,
                methods: value.methods,
                path_prefixes: value.path_prefixes,
            }),
            git: self.git.map(|value| GitScope {
                remote_name: value.remote_name,
                remote_url: value.remote_url,
                operations: value.operations,
                branches: value.branches,
                refspecs: value.refspecs,
                allow_tags: value.allow_tags,
                allow_force_push: value.allow_force_push,
                allow_delete_refs: value.allow_delete_refs,
            }),
            ssh,
            created_at: self.created_at.into_native()?,
            updated_at: self.updated_at.into_native()?,
        };
        ptrack_core::Validate::validate(&capability).map_err(|error| {
            ImportError::InvalidStage(format!(
                "source capability is invalid and must be quarantined: {error}"
            ))
        })?;
        capability.enabled = false;
        capability.approved_at = Timestamp::Zero;
        capability.expires_at = Timestamp::Zero;
        Ok(capability)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityAuditValue {
    id: String,
    capability_id: String,
    agent_profile: String,
    kind: String,
    operation: String,
    target: String,
    success: bool,
    error_class: String,
    duration_millis: String,
    request_bytes: String,
    response_bytes: String,
    redirects: String,
    created_at: TimestampValue,
}
impl IntoNative for CapabilityAuditValue {
    type Native = CapabilityAudit;
    fn into_native(self) -> ImportResult<CapabilityAudit> {
        Ok(CapabilityAudit {
            id: u64_field!(self.id, "audit.id"),
            capability_id: u64_field!(self.capability_id, "audit.capability_id"),
            agent_profile: self.agent_profile,
            kind: capability_kind(&self.kind)?,
            operation: self.operation,
            target: self.target,
            success: self.success,
            error_class: self.error_class,
            duration_millis: i64_field!(self.duration_millis, "audit.duration_millis"),
            request_bytes: i64_field!(self.request_bytes, "audit.request_bytes"),
            response_bytes: i64_field!(self.response_bytes, "audit.response_bytes"),
            redirects: i64_field!(self.redirects, "audit.redirects"),
            created_at: self.created_at.into_native()?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryWritebackValue {
    digest_sha256: String,
    sequence: String,
    kind: String,
    note_id: String,
}
impl IntoNative for MemoryWritebackValue {
    type Native = MemoryWritebackRecord;
    fn into_native(self) -> ImportResult<MemoryWritebackRecord> {
        Ok(MemoryWritebackRecord {
            digest: Digest32(decode_digest(&self.digest_sha256, "writeback.digest")?),
            sequence: u64_field!(self.sequence, "writeback.sequence"),
            kind: memory_kind(&self.kind)?,
            note_id: u64_field!(self.note_id, "writeback.note_id"),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectRefValue {
    name: String,
    path: String,
    last_seen: TimestampValue,
}
impl IntoNative for ProjectRefValue {
    type Native = ProjectRef;
    fn into_native(self) -> ImportResult<ProjectRef> {
        Ok(ProjectRef {
            name: self.name,
            path: self.path,
            last_seen: self.last_seen.into_native()?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecordValue {
    encoding: String,
    bytes: String,
}

fn plan_status(value: &str) -> ImportResult<PlanStatus> {
    match value {
        "active" => Ok(PlanStatus::Active),
        "done" => Ok(PlanStatus::Done),
        "archived" => Ok(PlanStatus::Archived),
        _ => invalid("unknown plan status"),
    }
}
fn task_status(value: &str) -> ImportResult<TaskStatus> {
    match value {
        "todo" => Ok(TaskStatus::Todo),
        "doing" => Ok(TaskStatus::Doing),
        "done" => Ok(TaskStatus::Done),
        "blocked" => Ok(TaskStatus::Blocked),
        _ => invalid("unknown task status"),
    }
}
fn note_target(value: &str) -> ImportResult<NoteTarget> {
    match value {
        "project" => Ok(NoteTarget::Project),
        "plan" => Ok(NoteTarget::Plan),
        "task" => Ok(NoteTarget::Task),
        _ => invalid("unknown note target"),
    }
}
fn memory_kind(value: &str) -> ImportResult<MemoryKind> {
    match value {
        "legacy" => Ok(MemoryKind::Legacy),
        "decision" => Ok(MemoryKind::Decision),
        "blocker" => Ok(MemoryKind::Blocker),
        "handoff" => Ok(MemoryKind::Handoff),
        "summary" => Ok(MemoryKind::Summary),
        _ => invalid("unknown memory kind"),
    }
}
fn milestone_status(value: &str) -> ImportResult<MilestoneStatus> {
    match value {
        "open" => Ok(MilestoneStatus::Open),
        "done" => Ok(MilestoneStatus::Done),
        _ => invalid("unknown milestone status"),
    }
}
fn issue_status(value: &str) -> ImportResult<IssueStatus> {
    match value {
        "open" => Ok(IssueStatus::Open),
        "closed" => Ok(IssueStatus::Closed),
        _ => invalid("unknown issue status"),
    }
}
fn severity(value: &str) -> ImportResult<Severity> {
    match value {
        "low" => Ok(Severity::Low),
        "medium" => Ok(Severity::Medium),
        "high" => Ok(Severity::High),
        "critical" => Ok(Severity::Critical),
        _ => invalid("unknown severity"),
    }
}
fn capability_kind(value: &str) -> ImportResult<CapabilityKind> {
    match value {
        "http" => Ok(CapabilityKind::Http),
        "git" => Ok(CapabilityKind::Git),
        "ssh" => Ok(CapabilityKind::Ssh),
        _ => invalid("unknown capability kind"),
    }
}

fn validate_model_collection(model: &str, bucket: &str) -> ImportResult<()> {
    let expected = match model {
        "meta" => "meta",
        "plan" => "plans",
        "task" => "tasks",
        "note" => "notes",
        "milestone" => "milestones",
        "issue" => "issues",
        "commit" => "commits",
        "capability" => "capabilities",
        "capability_audit" => "capability_audits",
        "memory_writeback" => "memory_writebacks",
        "project_ref" => "projects",
        "raw" if matches!(bucket, "config" | "backups") => bucket,
        _ => return invalid("record model is not valid for its bucket"),
    };
    if expected != bucket {
        return invalid("record model does not match its bucket");
    }
    Ok(())
}

fn validate_key_identity(model: &str, key: &[u8], payload: &[u8], raw: bool) -> ImportResult<()> {
    if raw || matches!(model, "meta" | "memory_writeback" | "project_ref") {
        return Ok(());
    }
    let id = u64::from_be_bytes(
        key.try_into()
            .map_err(|_| ImportError::InvalidStage("numeric key length".to_owned()))?,
    );
    if payload.get(..8) != Some(id.to_be_bytes().as_slice()) {
        return invalid("record payload ID does not match key");
    }
    Ok(())
}

pub(crate) fn decode_hex(value: &str, field: &str) -> ImportResult<Vec<u8>> {
    if !value.len().is_multiple_of(2)
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return invalid(format!("{field} is not lowercase even-length hex"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |b| match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                _ => unreachable!(),
            };
            Ok((digit(pair[0]) << 4) | digit(pair[1]))
        })
        .collect()
}
pub(crate) fn decode_digest(value: &str, field: &str) -> ImportResult<[u8; 32]> {
    decode_hex(value, field)?
        .try_into()
        .map_err(|_| ImportError::InvalidStage(format!("{field} must contain 32 bytes")))
}

pub(crate) fn validate_collection_set(names: &BTreeSet<String>, project: bool) -> ImportResult<()> {
    let known = if project {
        PROJECT_COLLECTIONS.as_slice()
    } else {
        GLOBAL_COLLECTIONS.as_slice()
    };
    if names.iter().any(|name| !known.contains(&name.as_str())) {
        return invalid("database contains an unknown collection");
    }
    Ok(())
}
