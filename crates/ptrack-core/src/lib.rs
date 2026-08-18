//! Dependency-free native persistent record contract for ptrack.
//!
//! The binary codec is positional, big-endian, bounded, and strict. It does
//! not normalize capability URLs or paths: callers must perform the full
//! policy normalization before enabling or using a decoded capability.

mod codec;
mod guide;
mod model;
mod report;
mod search;
mod snapshot;
mod validation;
mod views;

pub use codec::{
    CodecError, MAX_LIST_ITEMS, MAX_PAYLOAD_BYTES, MAX_STRING_BYTES, MIN_NATIVE_PAYLOAD_SCHEMA,
    NATIVE_CODEC, NATIVE_PAYLOAD_SCHEMA, decode_record, decode_record_at_schema, encode_record,
    encode_record_at_schema,
};
pub use guide::{GUIDE_BEGIN, GUIDE_END, guide_block, guide_body, render_guide, upsert_guide};
pub use model::{
    CAPABILITY_MODEL_VERSION, Capability, CapabilityAudit, CapabilityAuditPolicy, CapabilityKind,
    CapabilityLimits, Commit, Counts, Digest32, GitScope, HttpScope, Issue, IssueStatus,
    MemoryKind, MemoryWritebackRecord, Meta, Milestone, MilestoneStatus, NativeRecord, Note,
    NoteTarget, ParseEnumError, Plan, PlanStatus, ProjectRef, RecordKind, Severity, SshScope,
    StoredDate, Task, TaskStatus, Timestamp,
};
pub use report::{
    Digest, IssueLine, NoteLine, PlanBrief, ReportError, TaskLine, context, hold_marker,
};
pub use search::{SearchView, search};
pub use snapshot::ProjectSnapshot;
pub use validation::{
    LEGACY_ACTOR, MAX_HOLD_REASON_BYTES, MAX_IDENTITY_NAME_BYTES, Validate, ValidationError,
    check_hold_reason, check_identity_name, is_identity_id,
};
pub use views::{
    Board, IssueShow, MilestoneRef, MilestoneShow, NextView, PlanRef, PlanShow, TaskShow,
    board_for, next, show_issue, show_milestone, show_plan, show_task,
};

#[cfg(test)]
mod codec_test;
#[cfg(test)]
mod guide_test;
#[cfg(test)]
mod model_behavior_test;
#[cfg(test)]
mod report_test;
#[cfg(test)]
mod search_test;
#[cfg(test)]
mod snapshot_test;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod validation_test;
#[cfg(test)]
mod views_test;
