use serde::{Deserialize, Serialize};

use crate::{EventCorrelation, Timestamp};

pub const EVENT_MODEL_VERSION: u32 = 1;

macro_rules! event_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
        pub enum $name {
            #[default]
            #[serde(rename = "")]
            Unset,
            $(#[serde(rename = $value)] $variant),+
        }
        impl $name {
            #[must_use]
            pub const fn is_valid(self) -> bool { !matches!(self, Self::Unset) }
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { Self::Unset => "", $(Self::$variant => $value),+ }
            }
        }
    };
}

event_enum!(EventKind {
    Lifecycle => "lifecycle", Tool => "tool", Command => "command", File => "file",
    Test => "test", Commit => "commit", Error => "error", Summary => "summary",
});
event_enum!(EventPhase {
    Started => "started", Progress => "progress", Waiting => "waiting",
    Blocked => "blocked", Completed => "completed", Failed => "failed",
});
event_enum!(EventOutcome { Succeeded => "succeeded", Failed => "failed" });
event_enum!(EventNotificationKind {
    ApprovalRequested => "approvalRequested", Question => "question",
    Failure => "failure", Completion => "completion",
});

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventObservation {
    pub model_version: u32,
    pub source_id: String,
    pub source_sequence: u64,
    pub kind: EventKind,
    pub phase: EventPhase,
    #[serde(default, skip_serializing_if = "is_unset_outcome")]
    pub outcome: EventOutcome,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "is_zero_timestamp")]
    pub occurred_at: Timestamp,
    #[serde(default, skip_serializing_if = "is_unset_notification")]
    pub notification: EventNotificationKind,
    #[serde(skip)]
    pub(crate) recognized_notification: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub model_version: u32,
    pub id: String,
    pub run_id: String,
    pub provider: String,
    pub source_id: String,
    pub source_sequence: u64,
    pub host_sequence: u64,
    pub lifecycle_revision: u64,
    pub kind: EventKind,
    pub phase: EventPhase,
    #[serde(default, skip_serializing_if = "is_unset_outcome")]
    pub outcome: EventOutcome,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_class: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "is_zero_timestamp")]
    pub occurred_at: Timestamp,
    pub observed_at: Timestamp,
    pub correlation: EventCorrelation,
    #[serde(default, skip_serializing_if = "is_unset_notification")]
    pub notification: EventNotificationKind,
}

pub(crate) fn observation_from_persisted_event(event: &Event) -> EventObservation {
    EventObservation {
        model_version: event.model_version,
        source_id: event.source_id.clone(),
        source_sequence: event.source_sequence,
        kind: event.kind,
        phase: event.phase,
        outcome: event.outcome,
        subject: event.subject.clone(),
        paths: event.paths.clone(),
        commit_sha: event.commit_sha.clone(),
        exit_code: event.exit_code,
        error_class: event.error_class.clone(),
        summary: event.summary.clone(),
        occurred_at: event.occurred_at,
        notification: event.notification,
        recognized_notification: event.notification.is_valid(),
    }
}

const fn is_unset_outcome(value: &EventOutcome) -> bool {
    matches!(value, EventOutcome::Unset)
}

const fn is_unset_notification(value: &EventNotificationKind) -> bool {
    matches!(value, EventNotificationKind::Unset)
}

const fn is_zero_timestamp(value: &Timestamp) -> bool {
    value.is_zero()
}
