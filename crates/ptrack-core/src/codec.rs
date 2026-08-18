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
pub const NATIVE_PAYLOAD_SCHEMA: u32 = 3;
/// Oldest native payload schema this build still decodes.
///
/// Schema 1 predates the plan and task hold reason, which schema 2 added.
/// Schema 3 adds actor attribution, reserved entity ULIDs, plan claims, and the
/// per-actor `Meta` maps. Payloads at either older schema decode with all of
/// those fields empty and are re-encoded at [`NATIVE_PAYLOAD_SCHEMA`] on their
/// next write, so stored records upgrade lazily and no database is rewritten on
/// open.
pub const MIN_NATIVE_PAYLOAD_SCHEMA: u32 = 1;
/// Maximum accepted bytes in one native record payload.
pub const MAX_PAYLOAD_BYTES: usize = 256 * 1024 * 1024;
/// Maximum accepted UTF-8 bytes in one string field.
pub const MAX_STRING_BYTES: usize = MAX_PAYLOAD_BYTES;
/// Maximum accepted elements in one string list, matching the Go encoder.
pub const MAX_LIST_ITEMS: usize = 1_000_000;

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
    UnsupportedPayloadSchema(u32),
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
            Self::UnsupportedPayloadSchema(schema) => write!(
                formatter,
                "native payload schema {schema} is outside the supported range \
                 {MIN_NATIVE_PAYLOAD_SCHEMA} through {NATIVE_PAYLOAD_SCHEMA}"
            ),
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
    encode_record_at_schema(record, NATIVE_PAYLOAD_SCHEMA)
}

/// Encodes one validated record as canonical bytes for a known payload schema.
///
/// Used to re-derive the canonical form of a stored record without upgrading
/// it, so a schema-1 payload can be checked against the schema-1 layout it was
/// written with.
///
/// # Errors
///
/// Returns an error for an unsupported payload schema, for a semantically
/// invalid record, for a defensive encoding bound, and for a record that has no
/// canonical form at the requested schema.
pub fn encode_record_at_schema(
    record: &NativeRecord,
    payload_schema: u32,
) -> Result<Vec<u8>, CodecError> {
    if !(MIN_NATIVE_PAYLOAD_SCHEMA..=NATIVE_PAYLOAD_SCHEMA).contains(&payload_schema) {
        return Err(CodecError::UnsupportedPayloadSchema(payload_schema));
    }
    record.validate()?;
    encode_unchecked(record, payload_schema)
}

/// Strictly decodes and validates one field-only payload written at the current
/// payload schema.
///
/// # Errors
///
/// Returns an error for any malformed, noncanonical, oversized, trailing, or
/// semantically invalid input.
pub fn decode_record(kind: RecordKind, payload: &[u8]) -> Result<NativeRecord, CodecError> {
    decode_record_at_schema(kind, NATIVE_PAYLOAD_SCHEMA, payload)
}

/// Strictly decodes and validates one field-only payload written at a known
/// payload schema.
///
/// Canonicality is checked against the encoding of that same schema, so a
/// schema-1 payload must round-trip byte for byte through the schema-1 layout.
///
/// # Errors
///
/// Returns an error for an unsupported payload schema, and for any malformed,
/// noncanonical, oversized, trailing, or semantically invalid input.
pub fn decode_record_at_schema(
    kind: RecordKind,
    payload_schema: u32,
    payload: &[u8],
) -> Result<NativeRecord, CodecError> {
    if !(MIN_NATIVE_PAYLOAD_SCHEMA..=NATIVE_PAYLOAD_SCHEMA).contains(&payload_schema) {
        return Err(CodecError::UnsupportedPayloadSchema(payload_schema));
    }
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(CodecError::PayloadTooLarge {
            actual: payload.len(),
            maximum: MAX_PAYLOAD_BYTES,
        });
    }
    let mut reader = Reader::new(payload);
    let record = match kind {
        RecordKind::Meta => NativeRecord::Meta(decode_meta(&mut reader, payload_schema)?),
        RecordKind::Plan => NativeRecord::Plan(decode_plan(&mut reader, payload_schema)?),
        RecordKind::Task => NativeRecord::Task(decode_task(&mut reader, payload_schema)?),
        RecordKind::Note => NativeRecord::Note(decode_note(&mut reader, payload_schema)?),
        RecordKind::Milestone => {
            NativeRecord::Milestone(decode_milestone(&mut reader, payload_schema)?)
        }
        RecordKind::Issue => NativeRecord::Issue(decode_issue(&mut reader, payload_schema)?),
        RecordKind::Commit => NativeRecord::Commit(decode_commit(&mut reader, payload_schema)?),
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
    if encode_unchecked(&record, payload_schema)? != payload {
        return Err(CodecError::NonCanonical);
    }
    Ok(record)
}

fn encode_unchecked(record: &NativeRecord, payload_schema: u32) -> Result<Vec<u8>, CodecError> {
    let mut writer = Writer::default();
    match record {
        NativeRecord::Meta(value) => encode_meta(&mut writer, value, payload_schema)?,
        NativeRecord::Plan(value) => encode_plan(&mut writer, value, payload_schema)?,
        NativeRecord::Task(value) => encode_task(&mut writer, value, payload_schema)?,
        NativeRecord::Note(value) => encode_note(&mut writer, value, payload_schema)?,
        NativeRecord::Milestone(value) => encode_milestone(&mut writer, value, payload_schema)?,
        NativeRecord::Issue(value) => encode_issue(&mut writer, value, payload_schema)?,
        NativeRecord::Commit(value) => encode_commit(&mut writer, value, payload_schema)?,
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
        let count = self.entry_count(4)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(count)
            .map_err(|_| CodecError::LengthOverflow)?;
        for _ in 0..count {
            values.push(self.string()?);
        }
        Ok(values)
    }

    /// Reads a list length, rejecting counts past the list bound or past what
    /// the remaining bytes could hold at `minimum_entry_bytes` per entry.
    fn entry_count(&mut self, minimum_entry_bytes: usize) -> Result<usize, CodecError> {
        let count = usize::try_from(self.u32()?).map_err(|_| CodecError::LengthOverflow)?;
        if count > MAX_LIST_ITEMS {
            return Err(CodecError::ListTooLarge {
                actual: count,
                maximum: MAX_LIST_ITEMS,
            });
        }
        if count > self.remaining() / minimum_entry_bytes {
            return Err(CodecError::Truncated {
                needed: count.saturating_mul(minimum_entry_bytes),
                remaining: self.remaining(),
            });
        }
        Ok(count)
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

fn encode_meta(writer: &mut Writer, value: &Meta, payload_schema: u32) -> Result<(), CodecError> {
    writer.string(&value.goal)?;
    writer.string(&value.summary)?;
    writer.u64(value.active_plan)?;
    writer.timestamp(value.created_at)?;
    writer.timestamp(value.updated_at)?;
    writer.u64(value.format_version)?;
    writer.string(&value.last_write_version)?;
    encode_meta_maps(writer, value, payload_schema)
}

fn decode_meta(reader: &mut Reader<'_>, payload_schema: u32) -> Result<Meta, CodecError> {
    let mut meta = Meta {
        goal: reader.string()?,
        summary: reader.string()?,
        active_plan: reader.u64()?,
        created_at: reader.timestamp()?,
        updated_at: reader.timestamp()?,
        format_version: reader.u64()?,
        last_write_version: reader.string()?,
        active_plans: Vec::new(),
        actors: Vec::new(),
    };
    decode_meta_maps(reader, &mut meta, payload_schema)?;
    Ok(meta)
}

/// The payload schema that introduced the plan and task hold reason.
///
/// This is deliberately an absolute schema number rather than a comparison
/// against [`NATIVE_PAYLOAD_SCHEMA`]: the next bump to 3 must keep writing and
/// reading the hold-reason option byte for schema-2 records.
pub(crate) const HOLD_REASON_PAYLOAD_SCHEMA: u32 = 2;

/// Writes the trailing hold reason, which exists only from payload schema 2.
///
/// A schema-1 payload has no canonical form for a set hold reason, so encoding
/// one at that schema is rejected rather than silently dropped.
fn encode_hold_reason(
    writer: &mut Writer,
    value: Option<&String>,
    payload_schema: u32,
) -> Result<(), CodecError> {
    if payload_schema >= HOLD_REASON_PAYLOAD_SCHEMA {
        return writer.option(value, |writer, reason| writer.string(reason));
    }
    if value.is_some() {
        Err(CodecError::NonCanonical)
    } else {
        Ok(())
    }
}

/// Reads the trailing hold reason, absent before payload schema 2.
fn decode_hold_reason(
    reader: &mut Reader<'_>,
    payload_schema: u32,
) -> Result<Option<String>, CodecError> {
    if payload_schema >= HOLD_REASON_PAYLOAD_SCHEMA {
        reader.option(Reader::string)
    } else {
        Ok(None)
    }
}

/// The payload schema that introduced actor attribution, reserved entity
/// ULIDs, plan claims, and the per-actor Meta maps.
///
/// Deliberately an absolute schema number rather than a comparison against
/// [`NATIVE_PAYLOAD_SCHEMA`], exactly like [`HOLD_REASON_PAYLOAD_SCHEMA`]: a
/// future bump to 4 must keep writing and reading these fields for schema-3
/// records.
pub(crate) const ACTOR_PAYLOAD_SCHEMA: u32 = 3;

/// Writes the trailing actor and reserved-ULID options, present only from
/// payload schema 3. An older schema has no canonical form for a set value.
fn encode_actor_ulid(
    writer: &mut Writer,
    actor: Option<&String>,
    ulid: Option<&String>,
    payload_schema: u32,
) -> Result<(), CodecError> {
    if payload_schema >= ACTOR_PAYLOAD_SCHEMA {
        writer.option(actor, |writer, value| writer.string(value))?;
        return writer.option(ulid, |writer, value| writer.string(value));
    }
    if actor.is_some() || ulid.is_some() {
        Err(CodecError::NonCanonical)
    } else {
        Ok(())
    }
}

/// Reads the trailing actor and reserved-ULID options, absent before schema 3.
fn decode_actor_ulid(
    reader: &mut Reader<'_>,
    payload_schema: u32,
) -> Result<(Option<String>, Option<String>), CodecError> {
    if payload_schema >= ACTOR_PAYLOAD_SCHEMA {
        Ok((
            reader.option(Reader::string)?,
            reader.option(Reader::string)?,
        ))
    } else {
        Ok((None, None))
    }
}

/// Writes the trailing plan claim, present only from payload schema 3.
fn encode_plan_claim(
    writer: &mut Writer,
    value: &Plan,
    payload_schema: u32,
) -> Result<(), CodecError> {
    if payload_schema >= ACTOR_PAYLOAD_SCHEMA {
        writer.option(value.claim_owner.as_ref(), |writer, owner| {
            writer.string(owner)
        })?;
        writer.u64(value.claim_epoch)?;
        return writer.bool(value.claim_conflict);
    }
    if value.claim_owner.is_some() || value.claim_epoch != 0 || value.claim_conflict {
        Err(CodecError::NonCanonical)
    } else {
        Ok(())
    }
}

/// Reads the trailing plan claim, absent before payload schema 3.
fn decode_plan_claim(
    reader: &mut Reader<'_>,
    payload_schema: u32,
) -> Result<(Option<String>, u64, bool), CodecError> {
    if payload_schema >= ACTOR_PAYLOAD_SCHEMA {
        Ok((
            reader.option(Reader::string)?,
            reader.u64()?,
            reader.bool()?,
        ))
    } else {
        Ok((None, 0, false))
    }
}

/// Writes the trailing per-actor Meta maps, present only from payload schema 3.
fn encode_meta_maps(
    writer: &mut Writer,
    value: &Meta,
    payload_schema: u32,
) -> Result<(), CodecError> {
    if payload_schema >= ACTOR_PAYLOAD_SCHEMA {
        if value.active_plans.len() > MAX_LIST_ITEMS || value.actors.len() > MAX_LIST_ITEMS {
            return Err(CodecError::ListTooLarge {
                actual: value.active_plans.len().max(value.actors.len()),
                maximum: MAX_LIST_ITEMS,
            });
        }
        writer.u32(
            u32::try_from(value.active_plans.len()).map_err(|_| CodecError::LengthOverflow)?,
        )?;
        for (actor, plan) in &value.active_plans {
            writer.string(actor)?;
            writer.u64(*plan)?;
        }
        writer.u32(u32::try_from(value.actors.len()).map_err(|_| CodecError::LengthOverflow)?)?;
        for (actor, name) in &value.actors {
            writer.string(actor)?;
            writer.string(name)?;
        }
        return Ok(());
    }
    if !value.active_plans.is_empty() || !value.actors.is_empty() {
        Err(CodecError::NonCanonical)
    } else {
        Ok(())
    }
}

/// Reads the trailing per-actor Meta maps, absent before payload schema 3.
fn decode_meta_maps(
    reader: &mut Reader<'_>,
    value: &mut Meta,
    payload_schema: u32,
) -> Result<(), CodecError> {
    if payload_schema < ACTOR_PAYLOAD_SCHEMA {
        return Ok(());
    }
    // Each entry costs at least the length prefixes of its parts, matching the
    // truncation guard the string-list reader uses before it reserves.
    for _ in 0..reader.entry_count(12)? {
        value.active_plans.push((reader.string()?, reader.u64()?));
    }
    for _ in 0..reader.entry_count(8)? {
        value.actors.push((reader.string()?, reader.string()?));
    }
    Ok(())
}

fn encode_plan(writer: &mut Writer, value: &Plan, payload_schema: u32) -> Result<(), CodecError> {
    writer.u64(value.id)?;
    writer.string(&value.title)?;
    encode_enum!(writer, value.status);
    writer.u64(value.milestone_id)?;
    writer.i64(value.order)?;
    writer.timestamp(value.created_at)?;
    writer.timestamp(value.updated_at)?;
    encode_hold_reason(writer, value.hold_reason.as_ref(), payload_schema)?;
    encode_actor_ulid(
        writer,
        value.actor.as_ref(),
        value.ulid.as_ref(),
        payload_schema,
    )?;
    encode_plan_claim(writer, value, payload_schema)
}

fn decode_plan(reader: &mut Reader<'_>, payload_schema: u32) -> Result<Plan, CodecError> {
    let mut plan = Plan {
        id: reader.u64()?,
        title: reader.string()?,
        status: decode_enum!(reader, PlanStatus, "plan status"),
        milestone_id: reader.u64()?,
        order: reader.i64()?,
        created_at: reader.timestamp()?,
        updated_at: reader.timestamp()?,
        hold_reason: decode_hold_reason(reader, payload_schema)?,
        actor: None,
        ulid: None,
        claim_owner: None,
        claim_epoch: 0,
        claim_conflict: false,
    };
    (plan.actor, plan.ulid) = decode_actor_ulid(reader, payload_schema)?;
    (plan.claim_owner, plan.claim_epoch, plan.claim_conflict) =
        decode_plan_claim(reader, payload_schema)?;
    Ok(plan)
}

fn encode_task(writer: &mut Writer, value: &Task, payload_schema: u32) -> Result<(), CodecError> {
    writer.u64(value.id)?;
    writer.u64(value.plan_id)?;
    writer.string(&value.title)?;
    encode_enum!(writer, value.status);
    writer.i64(value.order)?;
    writer.timestamp(value.created_at)?;
    writer.timestamp(value.updated_at)?;
    encode_hold_reason(writer, value.hold_reason.as_ref(), payload_schema)?;
    encode_actor_ulid(
        writer,
        value.actor.as_ref(),
        value.ulid.as_ref(),
        payload_schema,
    )
}

fn decode_task(reader: &mut Reader<'_>, payload_schema: u32) -> Result<Task, CodecError> {
    let mut task = Task {
        id: reader.u64()?,
        plan_id: reader.u64()?,
        title: reader.string()?,
        status: decode_enum!(reader, TaskStatus, "task status"),
        order: reader.i64()?,
        created_at: reader.timestamp()?,
        updated_at: reader.timestamp()?,
        hold_reason: decode_hold_reason(reader, payload_schema)?,
        actor: None,
        ulid: None,
    };
    (task.actor, task.ulid) = decode_actor_ulid(reader, payload_schema)?;
    Ok(task)
}

fn encode_note(writer: &mut Writer, value: &Note, payload_schema: u32) -> Result<(), CodecError> {
    writer.u64(value.id)?;
    encode_enum!(writer, value.target);
    writer.u64(value.target_id)?;
    encode_enum!(writer, value.kind);
    writer.string(&value.body)?;
    writer.timestamp(value.created_at)?;
    encode_actor_ulid(
        writer,
        value.actor.as_ref(),
        value.ulid.as_ref(),
        payload_schema,
    )
}

fn decode_note(reader: &mut Reader<'_>, payload_schema: u32) -> Result<Note, CodecError> {
    let mut note = Note {
        id: reader.u64()?,
        target: decode_enum!(reader, NoteTarget, "note target"),
        target_id: reader.u64()?,
        kind: decode_enum!(reader, MemoryKind, "memory kind"),
        body: reader.string()?,
        created_at: reader.timestamp()?,
        actor: None,
        ulid: None,
    };
    (note.actor, note.ulid) = decode_actor_ulid(reader, payload_schema)?;
    Ok(note)
}

fn encode_milestone(
    writer: &mut Writer,
    value: &Milestone,
    payload_schema: u32,
) -> Result<(), CodecError> {
    writer.u64(value.id)?;
    writer.string(&value.title)?;
    encode_enum!(writer, value.status);
    writer.timestamp(value.due)?;
    writer.i64(value.order)?;
    writer.timestamp(value.created_at)?;
    writer.timestamp(value.updated_at)?;
    encode_actor_ulid(
        writer,
        value.actor.as_ref(),
        value.ulid.as_ref(),
        payload_schema,
    )
}

fn decode_milestone(reader: &mut Reader<'_>, payload_schema: u32) -> Result<Milestone, CodecError> {
    let mut milestone = Milestone {
        id: reader.u64()?,
        title: reader.string()?,
        status: decode_enum!(reader, MilestoneStatus, "milestone status"),
        due: reader.timestamp()?,
        order: reader.i64()?,
        created_at: reader.timestamp()?,
        updated_at: reader.timestamp()?,
        actor: None,
        ulid: None,
    };
    (milestone.actor, milestone.ulid) = decode_actor_ulid(reader, payload_schema)?;
    Ok(milestone)
}

fn encode_issue(writer: &mut Writer, value: &Issue, payload_schema: u32) -> Result<(), CodecError> {
    writer.u64(value.id)?;
    writer.string(&value.title)?;
    writer.string(&value.body)?;
    encode_enum!(writer, value.status);
    encode_enum!(writer, value.severity);
    writer.u64(value.task_id)?;
    writer.timestamp(value.created_at)?;
    writer.timestamp(value.updated_at)?;
    encode_actor_ulid(
        writer,
        value.actor.as_ref(),
        value.ulid.as_ref(),
        payload_schema,
    )
}

fn decode_issue(reader: &mut Reader<'_>, payload_schema: u32) -> Result<Issue, CodecError> {
    let mut issue = Issue {
        id: reader.u64()?,
        title: reader.string()?,
        body: reader.string()?,
        status: decode_enum!(reader, IssueStatus, "issue status"),
        severity: decode_enum!(reader, Severity, "issue severity"),
        task_id: reader.u64()?,
        created_at: reader.timestamp()?,
        updated_at: reader.timestamp()?,
        actor: None,
        ulid: None,
    };
    (issue.actor, issue.ulid) = decode_actor_ulid(reader, payload_schema)?;
    Ok(issue)
}

fn encode_commit(
    writer: &mut Writer,
    value: &Commit,
    payload_schema: u32,
) -> Result<(), CodecError> {
    writer.u64(value.id)?;
    writer.string(&value.sha)?;
    writer.string(&value.subject)?;
    writer.u64(value.plan_id)?;
    writer.u64(value.task_id)?;
    writer.timestamp(value.created_at)?;
    encode_actor_ulid(
        writer,
        value.actor.as_ref(),
        value.ulid.as_ref(),
        payload_schema,
    )
}

fn decode_commit(reader: &mut Reader<'_>, payload_schema: u32) -> Result<Commit, CodecError> {
    let mut commit = Commit {
        id: reader.u64()?,
        sha: reader.string()?,
        subject: reader.string()?,
        plan_id: reader.u64()?,
        task_id: reader.u64()?,
        created_at: reader.timestamp()?,
        actor: None,
        ulid: None,
    };
    (commit.actor, commit.ulid) = decode_actor_ulid(reader, payload_schema)?;
    Ok(commit)
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
