//! Small helpers every desktop runtime module shares: JSON conversion, error
//! construction, poisoned-lock recovery, tokens, and timestamp formatting.

use std::sync::{Mutex, MutexGuard};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ptrack_core::{Plan, Task, TaskStatus, Timestamp};
use ptrack_store::FIRST_RUN_TITLE_MAX_BYTES;
use serde::Serialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::wire::{FirstPlanV1, FirstTaskV1};
use crate::{AppError, AppResult};

pub(super) fn bound(shown: usize, total: usize) -> Value {
    json!({ "shown": shown, "total": total, "more": total.saturating_sub(shown) })
}

pub(super) fn timestamp_expired(timestamp: Timestamp) -> bool {
    timestamp
        .unix_nanoseconds()
        .is_some_and(|value| value <= OffsetDateTime::now_utc().unix_timestamp_nanos())
}

pub(super) fn timestamp(value: Timestamp) -> String {
    timestamp_datetime(value).map_or_else(
        || "0001-01-01T00:00:00Z".to_owned(),
        |timestamp| timestamp.format(&Rfc3339).unwrap_or_default(),
    )
}

/// The scan's own wall-clock stamp, in the same fixed-offset shape the store
/// writes elsewhere.
pub(super) fn now_timestamp() -> Timestamp {
    let now = OffsetDateTime::now_utc();
    Timestamp::Fixed {
        seconds: now.unix_timestamp(),
        nanoseconds: now.nanosecond(),
        offset_seconds: 0,
    }
}

pub(super) fn parse_first_run_timestamp(value: &str) -> AppResult<Timestamp> {
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| AppError::Message("first task timestamp is invalid".to_owned()))?;
    let timestamp = Timestamp::Fixed {
        seconds: parsed.unix_timestamp(),
        nanoseconds: parsed.nanosecond(),
        offset_seconds: parsed.offset().whole_seconds(),
    };
    if self::timestamp(timestamp) != value {
        return Err(AppError::Message(
            "first task timestamp is not canonical".to_owned(),
        ));
    }
    Ok(timestamp)
}

pub(super) fn first_plan_view(plan: &Plan) -> FirstPlanV1 {
    FirstPlanV1 {
        id: plan.id,
        title: plan.title.clone(),
        status: plan.status.as_str().to_owned(),
        created_at: timestamp(plan.created_at),
        updated_at: timestamp(plan.updated_at),
    }
}

pub(super) fn first_task_view(task: &Task) -> FirstTaskV1 {
    FirstTaskV1 {
        id: task.id,
        plan_id: task.plan_id,
        title: task.title.clone(),
        status: task.status.as_str().to_owned(),
        created_at: timestamp(task.created_at),
        updated_at: timestamp(task.updated_at),
    }
}

pub(super) fn timestamp_datetime(value: Timestamp) -> Option<OffsetDateTime> {
    let Timestamp::Fixed {
        seconds,
        nanoseconds,
        ..
    } = value
    else {
        return None;
    };
    let nanos = i32::try_from(nanoseconds).unwrap_or_default();
    let mut timestamp = OffsetDateTime::from_unix_timestamp(seconds).ok()?;
    timestamp += time::Duration::nanoseconds(i64::from(nanos));
    Some(timestamp)
}

pub(super) fn random_token() -> AppResult<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| AppError::Message(error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub(super) fn parse_task_status(value: &str) -> AppResult<TaskStatus> {
    TaskStatus::from_name(value)
        .ok_or_else(|| AppError::Message(format!("invalid task status {value:?}")))
}

pub(super) fn value<T: Serialize>(value: T) -> AppResult<Value> {
    serde_json::to_value(value).map_err(|error| AppError::Message(error.to_string()))
}

pub(super) fn trimmed_nonempty(value: &str, error: &str) -> AppResult<String> {
    let value = value.trim();
    if value.is_empty() {
        Err(AppError::Message(error.to_owned()))
    } else {
        Ok(value.to_owned())
    }
}

/// Treats an empty or blank bridge string argument as "not provided".
pub(super) fn optional_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

pub(super) fn first_run_title(value: &str, kind: &str) -> AppResult<String> {
    let title = value.trim();
    if title.is_empty() || title.len() > FIRST_RUN_TITLE_MAX_BYTES {
        return Err(AppError::Message(format!(
            "{kind} title must contain 1 to {FIRST_RUN_TITLE_MAX_BYTES} UTF-8 bytes"
        )));
    }
    Ok(title.to_owned())
}

pub(super) fn unavailable(feature: &str) -> AppError {
    AppError::Message(format!("{feature} is unavailable"))
}

pub(super) fn message(error: impl std::fmt::Display) -> AppError {
    AppError::Message(error.to_string())
}

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
