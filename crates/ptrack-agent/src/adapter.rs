use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    EVENT_MODEL_VERSION, EventKind, EventNotificationKind, EventObservation, EventOutcome,
    EventPhase, Timestamp,
};

pub const PROVIDER_EVENT_MODEL_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEvent {
    pub model_version: u32,
    pub id: String,
    pub sequence: u64,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default, skip_serializing_if = "is_unset_kind")]
    pub category: EventKind,
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
}

#[derive(Clone, Copy, Debug, Default)]
struct Mapping {
    kind: EventKind,
    phase: EventPhase,
    outcome: EventOutcome,
    error_class: &'static str,
    notification: EventNotificationKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterError(&'static str);

impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for AdapterError {}

#[must_use]
pub fn supported_event_providers() -> Vec<&'static str> {
    vec!["agy", "claude", "codex", "gemini", "kimi", "opencode"]
}

/// Maps a provider-specific event into the closed observation schema.
///
/// # Errors
///
/// Returns a fixed error for unknown providers, types, or disallowed fields.
pub fn normalize_provider_event(
    provider: &str,
    input: ProviderEvent,
) -> Result<EventObservation, AdapterError> {
    let provider = provider.trim().to_ascii_lowercase();
    if provider.is_empty() || provider.len() > 64 || !stable_event_name(&provider) {
        return Err(AdapterError("invalid agent event provider"));
    }
    if input.model_version != PROVIDER_EVENT_MODEL_VERSION || input.sequence == 0 {
        return Err(AdapterError("unsupported provider event contract"));
    }
    let event_type = input.event_type.trim();
    if event_type.is_empty() || event_type.len() > 64 || !stable_event_name(event_type) {
        return Err(AdapterError("invalid provider event type"));
    }
    let lower_type = event_type.to_ascii_lowercase();
    let known_provider = supported_event_providers().contains(&provider.as_str());
    let mut mapped = provider_mapping(&provider, &lower_type);
    if mapped.is_none() && provider == "codex" {
        mapped = codex_item(&lower_type, input.category);
    }
    if mapped.is_none() {
        mapped = canonical_event(&lower_type);
        if let Some(mapping) = mapped {
            if mapping.kind == EventKind::Summary {
                return Err(AdapterError(
                    "provider summaries require an explicit trusted adapter",
                ));
            }
            if !known_provider && mapping.kind != EventKind::Lifecycle {
                return Err(AdapterError("future provider adapter is lifecycle-only"));
            }
        }
    }
    if input.category != EventKind::Unset
        && !(provider == "codex" && lower_type.starts_with("item."))
    {
        return Err(AdapterError(
            "provider event category is not allowed for this type",
        ));
    }
    let Some(mut mapping) = mapped else {
        return Err(AdapterError("unsupported provider event type"));
    };
    if input.exit_code.is_some_and(|code| code != 0)
        && matches!(
            mapping.kind,
            EventKind::Lifecycle | EventKind::Command | EventKind::Test
        )
    {
        mapping.phase = EventPhase::Failed;
        mapping.outcome = EventOutcome::Failed;
        if mapping.notification != EventNotificationKind::Unset {
            mapping.notification = EventNotificationKind::Failure;
        }
    }
    if mapping.notification != EventNotificationKind::Unset {
        return Ok(EventObservation {
            model_version: EVENT_MODEL_VERSION,
            source_id: input.id,
            source_sequence: input.sequence,
            kind: mapping.kind,
            phase: mapping.phase,
            outcome: mapping.outcome,
            error_class: mapping.error_class.to_owned(),
            occurred_at: input.occurred_at,
            notification: mapping.notification,
            recognized_notification: true,
            ..EventObservation::default()
        });
    }
    Ok(EventObservation {
        model_version: EVENT_MODEL_VERSION,
        source_id: input.id,
        source_sequence: input.sequence,
        kind: mapping.kind,
        phase: mapping.phase,
        outcome: mapping.outcome,
        subject: input.subject,
        paths: input.paths,
        commit_sha: input.commit_sha,
        exit_code: input.exit_code,
        error_class: if input.error_class.is_empty() {
            mapping.error_class.to_owned()
        } else {
            input.error_class
        },
        summary: input.summary,
        occurred_at: input.occurred_at,
        ..EventObservation::default()
    })
}

#[allow(clippy::match_same_arms, clippy::too_many_lines)]
fn provider_mapping(provider: &str, event_type: &str) -> Option<Mapping> {
    use EventKind::{Command, Error, File, Lifecycle, Tool};
    use EventNotificationKind::{ApprovalRequested, Completion, Failure, Question};
    use EventOutcome::{Failed, Succeeded};
    use EventPhase::{Completed, Failed as PhaseFailed, Progress, Started, Waiting};
    let value = match (provider, event_type) {
        ("codex" | "claude" | "kimi", "sessionstart") => Mapping {
            kind: Lifecycle,
            phase: Started,
            ..Mapping::default()
        },
        ("codex", "turn.started") | ("kimi", "turnstarted") => Mapping {
            kind: Lifecycle,
            phase: Progress,
            ..Mapping::default()
        },
        ("codex", "turn.completed") | ("codex" | "claude" | "kimi", "stop") => Mapping {
            kind: Lifecycle,
            phase: Waiting,
            ..Mapping::default()
        },
        ("codex", "turn.failed") | ("kimi", "stopfailure") => Mapping {
            kind: Error,
            phase: PhaseFailed,
            outcome: Failed,
            notification: Failure,
            ..Mapping::default()
        },
        ("codex" | "claude" | "kimi", "pretooluse") => Mapping {
            kind: Tool,
            phase: Started,
            ..Mapping::default()
        },
        ("codex" | "claude" | "kimi", "posttooluse") => Mapping {
            kind: Tool,
            phase: Completed,
            outcome: Succeeded,
            ..Mapping::default()
        },
        ("codex" | "claude" | "kimi", "posttoolusefailure") => Mapping {
            kind: Tool,
            phase: PhaseFailed,
            outcome: Failed,
            ..Mapping::default()
        },
        ("codex" | "claude" | "gemini" | "kimi", "permissionrequest") => Mapping {
            kind: Lifecycle,
            phase: Waiting,
            notification: ApprovalRequested,
            ..Mapping::default()
        },
        ("codex" | "claude" | "gemini", "question") => Mapping {
            kind: Lifecycle,
            phase: Waiting,
            notification: Question,
            ..Mapping::default()
        },
        ("codex" | "claude" | "gemini" | "kimi", "sessionend") => Mapping {
            kind: Lifecycle,
            phase: Completed,
            outcome: Succeeded,
            notification: Completion,
            ..Mapping::default()
        },
        ("gemini", "sessionstart") => Mapping {
            kind: Lifecycle,
            phase: Started,
            ..Mapping::default()
        },
        ("gemini", "beforetool") => Mapping {
            kind: Tool,
            phase: Started,
            ..Mapping::default()
        },
        ("gemini", "aftertool") => Mapping {
            kind: Tool,
            phase: Completed,
            outcome: Succeeded,
            ..Mapping::default()
        },
        ("gemini", "notification") => Mapping {
            kind: Lifecycle,
            phase: Progress,
            ..Mapping::default()
        },
        ("agy", "session.started") | ("opencode", "session.created") => Mapping {
            kind: Lifecycle,
            phase: Started,
            ..Mapping::default()
        },
        ("agy", "session.waiting") | ("opencode", "session.idle") => Mapping {
            kind: Lifecycle,
            phase: Waiting,
            ..Mapping::default()
        },
        ("agy" | "opencode", "session.completed") => Mapping {
            kind: Lifecycle,
            phase: Completed,
            outcome: Succeeded,
            notification: Completion,
            ..Mapping::default()
        },
        ("agy", "session.failed") => Mapping {
            kind: Lifecycle,
            phase: PhaseFailed,
            outcome: Failed,
            notification: Failure,
            ..Mapping::default()
        },
        ("agy" | "opencode", "session.question") => Mapping {
            kind: Lifecycle,
            phase: Waiting,
            notification: Question,
            ..Mapping::default()
        },
        ("agy" | "opencode", "permission.requested") => Mapping {
            kind: Lifecycle,
            phase: Waiting,
            notification: ApprovalRequested,
            ..Mapping::default()
        },
        ("opencode", "session.error") => Mapping {
            kind: Error,
            phase: PhaseFailed,
            outcome: Failed,
            error_class: "session_failure",
            notification: Failure,
        },
        ("opencode", "tool.execute.before") => Mapping {
            kind: Tool,
            phase: Started,
            ..Mapping::default()
        },
        ("opencode", "tool.execute.after") => Mapping {
            kind: Tool,
            phase: Completed,
            outcome: Succeeded,
            ..Mapping::default()
        },
        ("opencode", "file.edited") => Mapping {
            kind: File,
            phase: Completed,
            outcome: Succeeded,
            ..Mapping::default()
        },
        ("opencode", "command.executed") => Mapping {
            kind: Command,
            phase: Completed,
            outcome: Succeeded,
            ..Mapping::default()
        },
        _ => return None,
    };
    Some(value)
}

fn codex_item(event_type: &str, category: EventKind) -> Option<Mapping> {
    if !category.is_valid() || matches!(category, EventKind::Lifecycle | EventKind::Summary) {
        return None;
    }
    match event_type {
        "item.started" => Some(Mapping {
            kind: category,
            phase: EventPhase::Started,
            ..Mapping::default()
        }),
        "item.completed" => Some(Mapping {
            kind: category,
            phase: EventPhase::Completed,
            outcome: EventOutcome::Succeeded,
            ..Mapping::default()
        }),
        "item.failed" => Some(Mapping {
            kind: category,
            phase: EventPhase::Failed,
            outcome: EventOutcome::Failed,
            ..Mapping::default()
        }),
        _ => None,
    }
}

fn canonical_event(event_type: &str) -> Option<Mapping> {
    let (kind, phase) = event_type.split_once('.')?;
    if phase.contains('.') {
        return None;
    }
    let kind = parse_kind(kind)?;
    let phase = parse_phase(phase)?;
    let outcome = match phase {
        EventPhase::Completed => EventOutcome::Succeeded,
        EventPhase::Failed => EventOutcome::Failed,
        _ => EventOutcome::Unset,
    };
    let notification = match (kind, phase) {
        (EventKind::Lifecycle, EventPhase::Completed) => EventNotificationKind::Completion,
        (EventKind::Lifecycle, EventPhase::Failed) => EventNotificationKind::Failure,
        _ => EventNotificationKind::Unset,
    };
    Some(Mapping {
        kind,
        phase,
        outcome,
        notification,
        ..Mapping::default()
    })
}

fn parse_kind(value: &str) -> Option<EventKind> {
    Some(match value {
        "lifecycle" => EventKind::Lifecycle,
        "tool" => EventKind::Tool,
        "command" => EventKind::Command,
        "file" => EventKind::File,
        "test" => EventKind::Test,
        "commit" => EventKind::Commit,
        "error" => EventKind::Error,
        "summary" => EventKind::Summary,
        _ => return None,
    })
}

fn parse_phase(value: &str) -> Option<EventPhase> {
    Some(match value {
        "started" => EventPhase::Started,
        "progress" => EventPhase::Progress,
        "waiting" => EventPhase::Waiting,
        "blocked" => EventPhase::Blocked,
        "completed" => EventPhase::Completed,
        "failed" => EventPhase::Failed,
        _ => return None,
    })
}

fn stable_event_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

const fn is_unset_kind(value: &EventKind) -> bool {
    matches!(value, EventKind::Unset)
}
const fn is_zero_timestamp(value: &Timestamp) -> bool {
    value.is_zero()
}
