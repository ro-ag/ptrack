//! Dependency-free native persistent record contract for ptrack.
//!
//! The binary codec is positional, big-endian, bounded, and strict. It does
//! not normalize capability URLs or paths: callers must perform the full
//! policy normalization before enabling or using a decoded capability.

mod codec;
mod model;
mod validation;

pub use codec::{
    CodecError, MAX_LIST_ITEMS, MAX_PAYLOAD_BYTES, MAX_STRING_BYTES, NATIVE_CODEC,
    NATIVE_PAYLOAD_SCHEMA, decode_record, encode_record,
};
pub use model::{
    CAPABILITY_MODEL_VERSION, Capability, CapabilityAudit, CapabilityAuditPolicy, CapabilityKind,
    CapabilityLimits, Commit, Digest32, GitScope, HttpScope, Issue, IssueStatus, MemoryKind,
    MemoryWritebackRecord, Meta, Milestone, MilestoneStatus, NativeRecord, Note, NoteTarget, Plan,
    PlanStatus, ProjectRef, RecordKind, Severity, SshScope, Task, TaskStatus, Timestamp,
};
pub use validation::{Validate, ValidationError};

#[cfg(test)]
mod codec_test;
#[cfg(test)]
mod validation_test;
