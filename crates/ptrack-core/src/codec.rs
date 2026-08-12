use std::fmt;
use std::str;

use crate::{
    Capability, CapabilityAudit, CapabilityAuditPolicy, CapabilityKind, CapabilityLimits, Commit,
    Digest32, GitScope, HttpScope, Issue, IssueStatus, MemoryKind, MemoryWritebackRecord, Meta,
    Milestone, MilestoneStatus, NativeRecord, Note, NoteTarget, Plan, PlanStatus, ProjectRef,
    RecordKind, Severity, SshScope, Task, TaskStatus, Timestamp, Validate, ValidationError,
};

/// Stable envelope codec ID for native ptrack positional records.
pub const NATIVE_CODEC: u16 = 3;
/// Current schema of native ptrack positional record payloads.
pub const NATIVE_PAYLOAD_SCHEMA: u32 = 1;
/// Maximum accepted bytes in one native record payload.
pub const MAX_PAYLOAD_BYTES: usize = 256 * 1024 * 1024;
/// Maximum accepted UTF-8 bytes in one string field.
pub const MAX_STRING_BYTES: usize = MAX_PAYLOAD_BYTES;
/// Maximum accepted elements in one string list. Every element needs at least
/// a four-byte length, so a larger list cannot fit the payload bound.
pub const MAX_LIST_ITEMS: usize = MAX_PAYLOAD_BYTES / 4;

/// A structural, canonical, or semantic native record error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodecError {
    PayloadTooLarge { actual: usize, maximum: usize },
    StringTooLarge { actual: usize, maximum: usize },
    ListTooLarge { actual: usize, maximum: usize },
    LengthOverflow,
    Truncated { needed: usize, remaining: usize },
    InvalidUtf8,
    InvalidBool(u8),
    InvalidEnum { name: &'static str, tag: u8 },
    InvalidOption(u8),
    InvalidTimestampTag(u8),
    TrailingBytes(usize),
    NonCanonical,
    UnsupportedRecordKind(RecordKind),
    InvalidRecord(ValidationError),
}

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "native payload is {actual} bytes; maximum is {maximum}"
                )
            }
            Self::StringTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "native string is {actual} bytes; maximum is {maximum}"
                )
            }
            Self::ListTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "native string list has {actual} items; maximum is {maximum}"
                )
            }
            Self::LengthOverflow => formatter.write_str("native length cannot be represented"),
            Self::Truncated { needed, remaining } => write!(
                formatter,
                "native payload is truncated: needs {needed} bytes, has {remaining}"
            ),
            Self::InvalidUtf8 => formatter.write_str("native string is not valid UTF-8"),
            Self::InvalidBool(tag) => write!(formatter, "invalid native boolean tag {tag}"),
            Self::InvalidEnum { name, tag } => {
                write!(formatter, "invalid native {name} tag {tag}")
            }
            Self::InvalidOption(tag) => write!(formatter, "invalid native option tag {tag}"),
            Self::InvalidTimestampTag(tag) => {
                write!(formatter, "invalid native timestamp tag {tag}")
            }
            Self::TrailingBytes(count) => {
                write!(formatter, "native payload has {count} trailing bytes")
            }
            Self::NonCanonical => formatter.write_str("native payload is not canonical"),
            Self::UnsupportedRecordKind(kind) => {
                write!(
                    formatter,
                    "record kind {kind:?} has no native model contract"
                )
            }
            Self::InvalidRecord(error) => write!(formatter, "invalid native record: {error}"),
        }
    }
}

impl std::error::Error for CodecError {}

impl From<ValidationError> for CodecError {
    fn from(value: ValidationError) -> Self {
        Self::InvalidRecord(value)
    }
}

/// Encodes one validated record as canonical field-only bytes.
///
/// # Errors
///
/// Returns an error when the record is semantically invalid or exceeds a
/// defensive encoding bound.
pub fn encode_record(record: &NativeRecord) -> Result<Vec<u8>, CodecError> {
    record.validate()?;
    encode_unchecked(record)
}

/// Strictly decodes and validates one field-only payload of an externally known kind.
///
/// # Errors
///
/// Returns an error for any malformed, noncanonical, oversized, trailing, or
/// semantically invalid input.
pub fn decode_record(kind: RecordKind, payload: &[u8]) -> Result<NativeRecord, CodecError> {
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(CodecError::PayloadTooLarge {
            actual: payload.len(),
            maximum: MAX_PAYLOAD_BYTES,
        });
    }
    let mut reader = Reader::new(payload);
    let record = match kind {
        RecordKind::Meta => NativeRecord::Meta(decode_meta(&mut reader)?),
        RecordKind::Plan => NativeRecord::Plan(decode_plan(&mut reader)?),
        RecordKind::Task => NativeRecord::Task(decode_task(&mut reader)?),
        RecordKind::Note => NativeRecord::Note(decode_note(&mut reader)?),
        RecordKind::Milestone => NativeRecord::Milestone(decode_milestone(&mut reader)?),
        RecordKind::Issue => NativeRecord::Issue(decode_issue(&mut reader)?),
        RecordKind::Commit => NativeRecord::Commit(decode_commit(&mut reader)?),
        RecordKind::Capability => NativeRecord::Capability(decode_capability(&mut reader)?),
        RecordKind::CapabilityAudit => {
            NativeRecord::CapabilityAudit(decode_capability_audit(&mut reader)?)
        }
        RecordKind::MemoryWriteback => {
            NativeRecord::MemoryWriteback(decode_memory_writeback(&mut reader)?)
        }
        RecordKind::ProjectRef => NativeRecord::ProjectRef(decode_project_ref(&mut reader)?),
        RecordKind::GlobalConfig | RecordKind::GlobalBackup => {
            return Err(CodecError::UnsupportedRecordKind(kind));
        }
    };
    if reader.remaining() != 0 {
        return Err(CodecError::TrailingBytes(reader.remaining()));
    }
    record.validate()?;
    if encode_unchecked(&record)? != payload {
        return Err(CodecError::NonCanonical);
    }
    Ok(record)
}

fn encode_unchecked(record: &NativeRecord) -> Result<Vec<u8>, CodecError> {
    let mut writer = Writer::default();
    match record {
        NativeRecord::Meta(value) => encode_meta(&mut writer, value)?,
        NativeRecord::Plan(value) => encode_plan(&mut writer, value)?,
        NativeRecord::Task(value) => encode_task(&mut writer, value)?,
        NativeRecord::Note(value) => encode_note(&mut writer, value)?,
        NativeRecord::Milestone(value) => encode_milestone(&mut writer, value)?,
        NativeRecord::Issue(value) => encode_issue(&mut writer, value)?,
        NativeRecord::Commit(value) => encode_commit(&mut writer, value)?,
        NativeRecord::Capability(value) => encode_capability(&mut writer, value)?,
        NativeRecord::CapabilityAudit(value) => encode_capability_audit(&mut writer, value)?,
        NativeRecord::MemoryWriteback(value) => encode_memory_writeback(&mut writer, value)?,
        NativeRecord::ProjectRef(value) => encode_project_ref(&mut writer, value)?,
    }
    Ok(writer.bytes)
}

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn write(&mut self, bytes: &[u8]) -> Result<(), CodecError> {
        let total = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or(CodecError::LengthOverflow)?;
        if total > MAX_PAYLOAD_BYTES {
            return Err(CodecError::PayloadTooLarge {
                actual: total,
                maximum: MAX_PAYLOAD_BYTES,
            });
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn u8(&mut self, value: u8) -> Result<(), CodecError> {
        self.write(&[value])
    }

    fn bool(&mut self, value: bool) -> Result<(), CodecError> {
        self.u8(u8::from(value))
    }

    fn u16(&mut self, value: u16) -> Result<(), CodecError> {
        self.write(&value.to_be_bytes())
    }

    fn u32(&mut self, value: u32) -> Result<(), CodecError> {
        self.write(&value.to_be_bytes())
    }

    fn u64(&mut self, value: u64) -> Result<(), CodecError> {
        self.write(&value.to_be_bytes())
    }

    fn i32(&mut self, value: i32) -> Result<(), CodecError> {
        self.write(&value.to_be_bytes())
    }

    fn i64(&mut self, value: i64) -> Result<(), CodecError> {
        self.write(&value.to_be_bytes())
    }

    fn string(&mut self, value: &str) -> Result<(), CodecError> {
        let length = value.len();
        if length > MAX_STRING_BYTES {
            return Err(CodecError::StringTooLarge {
                actual: length,
                maximum: MAX_STRING_BYTES,
            });
        }
        self.u32(u32::try_from(length).map_err(|_| CodecError::LengthOverflow)?)?;
        self.write(value.as_bytes())
    }

    fn strings(&mut self, values: &[String]) -> Result<(), CodecError> {
        if values.len() > MAX_LIST_ITEMS {
            return Err(CodecError::ListTooLarge {
                actual: values.len(),
                maximum: MAX_LIST_ITEMS,
            });
        }
        self.u32(u32::try_from(values.len()).map_err(|_| CodecError::LengthOverflow)?)?;
        for value in values {
            self.string(value)?;
        }
        Ok(())
    }

    fn timestamp(&mut self, value: Timestamp) -> Result<(), CodecError> {
        match value {
            Timestamp::Zero => self.u8(0),
            Timestamp::Fixed {
                seconds,
                nanoseconds,
                offset_seconds,
            } => {
                self.u8(1)?;
                self.i64(seconds)?;
                self.u32(nanoseconds)?;
                self.i32(offset_seconds)
            }
        }
    }

    fn digest(&mut self, value: Digest32) -> Result<(), CodecError> {
        self.write(&value.0)
    }

    fn option<T>(
        &mut self,
        value: Option<&T>,
        encode: impl FnOnce(&mut Self, &T) -> Result<(), CodecError>,
    ) -> Result<(), CodecError> {
        match value {
            None => self.u8(0),
            Some(inner) => {
                self.u8(1)?;
                encode(self, inner)
            }
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], CodecError> {
        if length > self.remaining() {
            return Err(CodecError::Truncated {
                needed: length,
                remaining: self.remaining(),
            });
        }
        let start = self.offset;
        self.offset += length;
        Ok(&self.bytes[start..self.offset])
    }

    fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }

    fn bool(&mut self) -> Result<bool, CodecError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            tag => Err(CodecError::InvalidBool(tag)),
        }
    }

    fn u16(&mut self) -> Result<u16, CodecError> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().expect("fixed length"),
        ))
    }

    fn u32(&mut self) -> Result<u32, CodecError> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("fixed length"),
        ))
    }

    fn u64(&mut self) -> Result<u64, CodecError> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().expect("fixed length"),
        ))
    }

    fn i32(&mut self) -> Result<i32, CodecError> {
        Ok(i32::from_be_bytes(
            self.take(4)?.try_into().expect("fixed length"),
        ))
    }

    fn i64(&mut self) -> Result<i64, CodecError> {
        Ok(i64::from_be_bytes(
            self.take(8)?.try_into().expect("fixed length"),
        ))
    }

    fn string(&mut self) -> Result<String, CodecError> {
        let length = usize::try_from(self.u32()?).map_err(|_| CodecError::LengthOverflow)?;
        if length > MAX_STRING_BYTES {
            return Err(CodecError::StringTooLarge {
                actual: length,
                maximum: MAX_STRING_BYTES,
            });
        }
        str::from_utf8(self.take(length)?)
            .map(str::to_owned)
            .map_err(|_| CodecError::InvalidUtf8)
    }

    fn strings(&mut self) -> Result<Vec<String>, CodecError> {
        let count = usize::try_from(self.u32()?).map_err(|_| CodecError::LengthOverflow)?;
        if count > MAX_LIST_ITEMS {
            return Err(CodecError::ListTooLarge {
                actual: count,
                maximum: MAX_LIST_ITEMS,
            });
        }
        if count > self.remaining() / 4 {
            return Err(CodecError::Truncated {
                needed: count.saturating_mul(4),
                remaining: self.remaining(),
            });
        }
        let mut values = Vec::new();
        values
            .try_reserve_exact(count)
            .map_err(|_| CodecError::LengthOverflow)?;
        for _ in 0..count {
            values.push(self.string()?);
        }
        Ok(values)
    }

    fn timestamp(&mut self) -> Result<Timestamp, CodecError> {
        match self.u8()? {
            0 => Ok(Timestamp::Zero),
            1 => Ok(Timestamp::Fixed {
                seconds: self.i64()?,
                nanoseconds: self.u32()?,
                offset_seconds: self.i32()?,
            }),
            tag => Err(CodecError::InvalidTimestampTag(tag)),
        }
    }

    fn digest(&mut self) -> Result<Digest32, CodecError> {
        Ok(Digest32(self.take(32)?.try_into().expect("fixed length")))
    }

    fn option<T>(
        &mut self,
        decode: impl FnOnce(&mut Self) -> Result<T, CodecError>,
    ) -> Result<Option<T>, CodecError> {
        match self.u8()? {
            0 => Ok(None),
            1 => decode(self).map(Some),
            tag => Err(CodecError::InvalidOption(tag)),
        }
    }

    fn enum_value<T>(
        &mut self,
        name: &'static str,
        decode: impl FnOnce(u8) -> Option<T>,
    ) -> Result<T, CodecError> {
        let tag = self.u8()?;
        decode(tag).ok_or(CodecError::InvalidEnum { name, tag })
    }
}

macro_rules! encode_enum {
    ($writer:expr, $value:expr) => {
        $writer.u8($value.wire_tag())?
    };
}

macro_rules! decode_enum {
    ($reader:expr, $type:ty, $name:literal) => {
        $reader.enum_value($name, <$type>::from_wire_tag)?
    };
}

fn encode_meta(writer: &mut Writer, value: &Meta) -> Result<(), CodecError> {
    writer.string(&value.goal)?;
    writer.string(&value.summary)?;
    writer.u64(value.active_plan)?;
    writer.timestamp(value.created_at)?;
    writer.timestamp(value.updated_at)?;
    writer.u64(value.format_version)?;
    writer.string(&value.last_write_version)
}

fn decode_meta(reader: &mut Reader<'_>) -> Result<Meta, CodecError> {
    Ok(Meta {
        goal: reader.string()?,
        summary: reader.string()?,
        active_plan: reader.u64()?,
        created_at: reader.timestamp()?,
        updated_at: reader.timestamp()?,
        format_version: reader.u64()?,
        last_write_version: reader.string()?,
    })
}

fn encode_plan(writer: &mut Writer, value: &Plan) -> Result<(), CodecError> {
    writer.u64(value.id)?;
    writer.string(&value.title)?;
    encode_enum!(writer, value.status);
    writer.u64(value.milestone_id)?;
    writer.i64(value.order)?;
    writer.timestamp(value.created_at)?;
    writer.timestamp(value.updated_at)
}

fn decode_plan(reader: &mut Reader<'_>) -> Result<Plan, CodecError> {
    Ok(Plan {
        id: reader.u64()?,
        title: reader.string()?,
        status: decode_enum!(reader, PlanStatus, "plan status"),
        milestone_id: reader.u64()?,
        order: reader.i64()?,
        created_at: reader.timestamp()?,
        updated_at: reader.timestamp()?,
    })
}

fn encode_task(writer: &mut Writer, value: &Task) -> Result<(), CodecError> {
    writer.u64(value.id)?;
    writer.u64(value.plan_id)?;
    writer.string(&value.title)?;
    encode_enum!(writer, value.status);
    writer.i64(value.order)?;
    writer.timestamp(value.created_at)?;
    writer.timestamp(value.updated_at)
}

fn decode_task(reader: &mut Reader<'_>) -> Result<Task, CodecError> {
    Ok(Task {
        id: reader.u64()?,
        plan_id: reader.u64()?,
        title: reader.string()?,
        status: decode_enum!(reader, TaskStatus, "task status"),
        order: reader.i64()?,
        created_at: reader.timestamp()?,
        updated_at: reader.timestamp()?,
    })
}

fn encode_note(writer: &mut Writer, value: &Note) -> Result<(), CodecError> {
    writer.u64(value.id)?;
    encode_enum!(writer, value.target);
    writer.u64(value.target_id)?;
    encode_enum!(writer, value.kind);
    writer.string(&value.body)?;
    writer.timestamp(value.created_at)
}

fn decode_note(reader: &mut Reader<'_>) -> Result<Note, CodecError> {
    Ok(Note {
        id: reader.u64()?,
        target: decode_enum!(reader, NoteTarget, "note target"),
        target_id: reader.u64()?,
        kind: decode_enum!(reader, MemoryKind, "memory kind"),
        body: reader.string()?,
        created_at: reader.timestamp()?,
    })
}

fn encode_milestone(writer: &mut Writer, value: &Milestone) -> Result<(), CodecError> {
    writer.u64(value.id)?;
    writer.string(&value.title)?;
    encode_enum!(writer, value.status);
    writer.timestamp(value.due)?;
    writer.i64(value.order)?;
    writer.timestamp(value.created_at)?;
    writer.timestamp(value.updated_at)
}

fn decode_milestone(reader: &mut Reader<'_>) -> Result<Milestone, CodecError> {
    Ok(Milestone {
        id: reader.u64()?,
        title: reader.string()?,
        status: decode_enum!(reader, MilestoneStatus, "milestone status"),
        due: reader.timestamp()?,
        order: reader.i64()?,
        created_at: reader.timestamp()?,
        updated_at: reader.timestamp()?,
    })
}

fn encode_issue(writer: &mut Writer, value: &Issue) -> Result<(), CodecError> {
    writer.u64(value.id)?;
    writer.string(&value.title)?;
    writer.string(&value.body)?;
    encode_enum!(writer, value.status);
    encode_enum!(writer, value.severity);
    writer.u64(value.task_id)?;
    writer.timestamp(value.created_at)?;
    writer.timestamp(value.updated_at)
}

fn decode_issue(reader: &mut Reader<'_>) -> Result<Issue, CodecError> {
    Ok(Issue {
        id: reader.u64()?,
        title: reader.string()?,
        body: reader.string()?,
        status: decode_enum!(reader, IssueStatus, "issue status"),
        severity: decode_enum!(reader, Severity, "issue severity"),
        task_id: reader.u64()?,
        created_at: reader.timestamp()?,
        updated_at: reader.timestamp()?,
    })
}

fn encode_commit(writer: &mut Writer, value: &Commit) -> Result<(), CodecError> {
    writer.u64(value.id)?;
    writer.string(&value.sha)?;
    writer.string(&value.subject)?;
    writer.u64(value.plan_id)?;
    writer.u64(value.task_id)?;
    writer.timestamp(value.created_at)
}

fn decode_commit(reader: &mut Reader<'_>) -> Result<Commit, CodecError> {
    Ok(Commit {
        id: reader.u64()?,
        sha: reader.string()?,
        subject: reader.string()?,
        plan_id: reader.u64()?,
        task_id: reader.u64()?,
        created_at: reader.timestamp()?,
    })
}

fn encode_limits(writer: &mut Writer, value: &CapabilityLimits) -> Result<(), CodecError> {
    writer.i64(value.timeout_seconds)?;
    writer.i64(value.max_request_bytes)?;
    writer.i64(value.max_response_bytes)?;
    writer.i64(value.max_output_bytes)?;
    writer.i64(value.max_redirects)?;
    writer.i64(value.max_concurrent)
}

fn decode_limits(reader: &mut Reader<'_>) -> Result<CapabilityLimits, CodecError> {
    Ok(CapabilityLimits {
        timeout_seconds: reader.i64()?,
        max_request_bytes: reader.i64()?,
        max_response_bytes: reader.i64()?,
        max_output_bytes: reader.i64()?,
        max_redirects: reader.i64()?,
        max_concurrent: reader.i64()?,
    })
}

fn encode_audit_policy(
    writer: &mut Writer,
    value: &CapabilityAuditPolicy,
) -> Result<(), CodecError> {
    writer.bool(value.enabled)?;
    writer.i64(value.retain_last)
}

fn decode_audit_policy(reader: &mut Reader<'_>) -> Result<CapabilityAuditPolicy, CodecError> {
    Ok(CapabilityAuditPolicy {
        enabled: reader.bool()?,
        retain_last: reader.i64()?,
    })
}

fn encode_http(writer: &mut Writer, value: &HttpScope) -> Result<(), CodecError> {
    writer.string(&value.base_url)?;
    writer.strings(&value.methods)?;
    writer.strings(&value.path_prefixes)
}

fn decode_http(reader: &mut Reader<'_>) -> Result<HttpScope, CodecError> {
    Ok(HttpScope {
        base_url: reader.string()?,
        methods: reader.strings()?,
        path_prefixes: reader.strings()?,
    })
}

fn encode_git(writer: &mut Writer, value: &GitScope) -> Result<(), CodecError> {
    writer.string(&value.remote_name)?;
    writer.string(&value.remote_url)?;
    writer.strings(&value.operations)?;
    writer.strings(&value.branches)?;
    writer.strings(&value.refspecs)?;
    writer.bool(value.allow_tags)?;
    writer.bool(value.allow_force_push)?;
    writer.bool(value.allow_delete_refs)
}

fn decode_git(reader: &mut Reader<'_>) -> Result<GitScope, CodecError> {
    Ok(GitScope {
        remote_name: reader.string()?,
        remote_url: reader.string()?,
        operations: reader.strings()?,
        branches: reader.strings()?,
        refspecs: reader.strings()?,
        allow_tags: reader.bool()?,
        allow_force_push: reader.bool()?,
        allow_delete_refs: reader.bool()?,
    })
}

fn encode_ssh(writer: &mut Writer, value: &SshScope) -> Result<(), CodecError> {
    writer.string(&value.alias)?;
    writer.string(&value.host)?;
    writer.u16(value.port)?;
    writer.string(&value.user)?;
    writer.string(&value.host_key)?;
    writer.bool(value.allow_git)?;
    writer.strings(&value.remote_commands)?;
    writer.bool(value.allow_upload)?;
    writer.bool(value.allow_download)?;
    writer.strings(&value.upload_roots)?;
    writer.strings(&value.download_roots)?;
    writer.strings(&value.upload_remote_roots)?;
    writer.strings(&value.download_remote_roots)?;
    writer.bool(value.allow_interactive_shell)?;
    writer.strings(&value.local_forward_targets)?;
    writer.strings(&value.remote_forward_targets)
}

fn decode_ssh(reader: &mut Reader<'_>) -> Result<SshScope, CodecError> {
    Ok(SshScope {
        alias: reader.string()?,
        host: reader.string()?,
        port: reader.u16()?,
        user: reader.string()?,
        host_key: reader.string()?,
        allow_git: reader.bool()?,
        remote_commands: reader.strings()?,
        allow_upload: reader.bool()?,
        allow_download: reader.bool()?,
        upload_roots: reader.strings()?,
        download_roots: reader.strings()?,
        upload_remote_roots: reader.strings()?,
        download_remote_roots: reader.strings()?,
        allow_interactive_shell: reader.bool()?,
        local_forward_targets: reader.strings()?,
        remote_forward_targets: reader.strings()?,
    })
}

fn encode_capability(writer: &mut Writer, value: &Capability) -> Result<(), CodecError> {
    writer.u64(value.id)?;
    writer.u64(value.model_version)?;
    writer.u64(value.revision)?;
    writer.string(&value.name)?;
    encode_enum!(writer, value.kind);
    writer.string(&value.agent_profile)?;
    writer.bool(value.enabled)?;
    writer.i64(value.approval_duration_seconds)?;
    writer.timestamp(value.approved_at)?;
    writer.timestamp(value.expires_at)?;
    writer.digest(value.scope_digest)?;
    encode_limits(writer, &value.limits)?;
    encode_audit_policy(writer, &value.audit)?;
    writer.option(value.http.as_ref(), encode_http)?;
    writer.option(value.git.as_ref(), encode_git)?;
    writer.option(value.ssh.as_ref(), encode_ssh)?;
    writer.timestamp(value.created_at)?;
    writer.timestamp(value.updated_at)
}

fn decode_capability(reader: &mut Reader<'_>) -> Result<Capability, CodecError> {
    Ok(Capability {
        id: reader.u64()?,
        model_version: reader.u64()?,
        revision: reader.u64()?,
        name: reader.string()?,
        kind: decode_enum!(reader, CapabilityKind, "capability kind"),
        agent_profile: reader.string()?,
        enabled: reader.bool()?,
        approval_duration_seconds: reader.i64()?,
        approved_at: reader.timestamp()?,
        expires_at: reader.timestamp()?,
        scope_digest: reader.digest()?,
        limits: decode_limits(reader)?,
        audit: decode_audit_policy(reader)?,
        http: reader.option(decode_http)?,
        git: reader.option(decode_git)?,
        ssh: reader.option(decode_ssh)?,
        created_at: reader.timestamp()?,
        updated_at: reader.timestamp()?,
    })
}

fn encode_capability_audit(writer: &mut Writer, value: &CapabilityAudit) -> Result<(), CodecError> {
    writer.u64(value.id)?;
    writer.u64(value.capability_id)?;
    writer.string(&value.agent_profile)?;
    encode_enum!(writer, value.kind);
    writer.string(&value.operation)?;
    writer.string(&value.target)?;
    writer.bool(value.success)?;
    writer.string(&value.error_class)?;
    writer.i64(value.duration_millis)?;
    writer.i64(value.request_bytes)?;
    writer.i64(value.response_bytes)?;
    writer.i64(value.redirects)?;
    writer.timestamp(value.created_at)
}

fn decode_capability_audit(reader: &mut Reader<'_>) -> Result<CapabilityAudit, CodecError> {
    Ok(CapabilityAudit {
        id: reader.u64()?,
        capability_id: reader.u64()?,
        agent_profile: reader.string()?,
        kind: decode_enum!(reader, CapabilityKind, "capability kind"),
        operation: reader.string()?,
        target: reader.string()?,
        success: reader.bool()?,
        error_class: reader.string()?,
        duration_millis: reader.i64()?,
        request_bytes: reader.i64()?,
        response_bytes: reader.i64()?,
        redirects: reader.i64()?,
        created_at: reader.timestamp()?,
    })
}

fn encode_memory_writeback(
    writer: &mut Writer,
    value: &MemoryWritebackRecord,
) -> Result<(), CodecError> {
    writer.digest(value.digest)?;
    writer.u64(value.sequence)?;
    encode_enum!(writer, value.kind);
    writer.u64(value.note_id)
}

fn decode_memory_writeback(reader: &mut Reader<'_>) -> Result<MemoryWritebackRecord, CodecError> {
    Ok(MemoryWritebackRecord {
        digest: reader.digest()?,
        sequence: reader.u64()?,
        kind: decode_enum!(reader, MemoryKind, "memory kind"),
        note_id: reader.u64()?,
    })
}

fn encode_project_ref(writer: &mut Writer, value: &ProjectRef) -> Result<(), CodecError> {
    writer.string(&value.name)?;
    writer.string(&value.path)?;
    writer.timestamp(value.last_seen)
}

fn decode_project_ref(reader: &mut Reader<'_>) -> Result<ProjectRef, CodecError> {
    Ok(ProjectRef {
        name: reader.string()?,
        path: reader.string()?,
        last_seen: reader.timestamp()?,
    })
}
