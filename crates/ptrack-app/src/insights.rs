//! Read-side aggregation behind the Insights page.
//!
//! Everything here is derived from a [`ProjectSnapshot`] that the store already
//! loads. Nothing is persisted and no record gains a field, so this module can
//! never change the on-disk format.
//!
//! # What the numbers mean
//!
//! The store records `created_at` and `updated_at` per record plus the current
//! status; there is no status-change journal. Creation counts are therefore
//! exact, while anything describing completion is dated by `updated_at` on
//! records whose status is currently done — a later edit moves the point. The
//! payload names those series `completedByUpdate` rather than `completed` so a
//! reader cannot mistake them for a transition log, and the interface labels
//! them the same way.

use std::collections::BTreeMap;

use ptrack_core::{IssueStatus, PlanStatus, ProjectSnapshot, Severity, TaskStatus, Timestamp};
use serde_json::{Value, json};
use time::{Date, Duration, OffsetDateTime, UtcOffset, Weekday};

/// Largest window the page can ask for, matching the activity heatmap.
const MAX_WEEKS: i64 = 52;
const DEFAULT_WEEKS: i64 = 16;

/// Upper edges of the lead-time buckets, in whole days. The last bucket
/// collects everything beyond the final edge.
const LEAD_TIME_EDGES: [i64; 5] = [1, 3, 7, 14, 28];

/// Builds the Insights payload for a window ending today.
pub(crate) fn insights_at(
    snapshot: &ProjectSnapshot,
    requested_weeks: i64,
    now: OffsetDateTime,
    local_offset_at: impl Fn(OffsetDateTime) -> UtcOffset + Copy,
) -> Value {
    let weeks = clamp_weeks(requested_weeks);
    let days = usize::try_from(weeks * 7).unwrap_or(112);
    let today = now.to_offset(local_offset_at(now)).date();
    let first = today - Duration::days(i64::try_from(days - 1).unwrap_or_default());

    json!({
        "weeks": weeks,
        "daily": daily(snapshot, first, days, local_offset_at),
        "cumulative": cumulative(snapshot, first, today, local_offset_at),
        "leadTime": lead_time(snapshot),
        "plans": plans(snapshot),
        "issues": issues(snapshot, today, local_offset_at),
        "punchcard": punchcard(snapshot, first, local_offset_at),
        "totals": totals(snapshot),
    })
}

fn clamp_weeks(requested: i64) -> i64 {
    if requested <= 0 {
        DEFAULT_WEEKS
    } else {
        requested.min(MAX_WEEKS)
    }
}

/// Resolves a timestamp to the calendar day it falls on where the reader is.
///
/// The offset is resolved per timestamp rather than once for the window, so a
/// window spanning a daylight-saving change buckets each day against the offset
/// in force on that day.
fn local_date(
    value: Timestamp,
    local_offset_at: impl Fn(OffsetDateTime) -> UtcOffset,
) -> Option<Date> {
    let timestamp = datetime(value)?;
    Some(timestamp.to_offset(local_offset_at(timestamp)).date())
}

fn local_datetime(
    value: Timestamp,
    local_offset_at: impl Fn(OffsetDateTime) -> UtcOffset,
) -> Option<OffsetDateTime> {
    let timestamp = datetime(value)?;
    Some(timestamp.to_offset(local_offset_at(timestamp)))
}

fn datetime(value: Timestamp) -> Option<OffsetDateTime> {
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
    if nanos > 0 {
        timestamp += Duration::nanoseconds(i64::from(nanos));
    }
    Some(timestamp)
}

/// One row per day in the window, with each kind of activity counted apart.
///
/// The heatmap merges notes and commits into a single number because it is read
/// as a shape; here they are separate, because the question is what kind of
/// work the project has been doing.
fn daily(
    snapshot: &ProjectSnapshot,
    first: Date,
    days: usize,
    local_offset_at: impl Fn(OffsetDateTime) -> UtcOffset + Copy,
) -> Vec<Value> {
    let mut notes = BTreeMap::<Date, usize>::new();
    let mut commits = BTreeMap::<Date, usize>::new();
    let mut created = BTreeMap::<Date, usize>::new();
    let mut completed = BTreeMap::<Date, usize>::new();

    for note in &snapshot.notes {
        if let Some(date) = local_date(note.created_at, local_offset_at) {
            *notes.entry(date).or_default() += 1;
        }
    }
    for commit in &snapshot.commits {
        if let Some(date) = local_date(commit.created_at, local_offset_at) {
            *commits.entry(date).or_default() += 1;
        }
    }
    for task in &snapshot.tasks {
        if let Some(date) = local_date(task.created_at, local_offset_at) {
            *created.entry(date).or_default() += 1;
        }
        if task.status == TaskStatus::Done
            && let Some(date) = local_date(task.updated_at, local_offset_at)
        {
            *completed.entry(date).or_default() += 1;
        }
    }

    (0..days)
        .map(|offset| {
            let date = first + Duration::days(i64::try_from(offset).unwrap_or_default());
            json!({
                "date": date.to_string(),
                "notes": notes.get(&date).copied().unwrap_or(0),
                "commits": commits.get(&date).copied().unwrap_or(0),
                "tasksCreated": created.get(&date).copied().unwrap_or(0),
                "tasksCompletedByUpdate": completed.get(&date).copied().unwrap_or(0),
            })
        })
        .collect()
}

/// Running totals of tasks created and tasks reaching done, by week.
///
/// Both series count every task in the project, including ones created before
/// the window, so the burn-up starts from where the project actually stood
/// rather than from zero.
fn cumulative(
    snapshot: &ProjectSnapshot,
    first: Date,
    today: Date,
    local_offset_at: impl Fn(OffsetDateTime) -> UtcOffset + Copy,
) -> Vec<Value> {
    let mut weeks = Vec::new();
    let mut cursor = week_start(first);
    while cursor <= today {
        let end = cursor + Duration::days(7);
        let created = snapshot
            .tasks
            .iter()
            .filter(|task| {
                local_date(task.created_at, local_offset_at).is_some_and(|date| date < end)
            })
            .count();
        let completed = snapshot
            .tasks
            .iter()
            .filter(|task| {
                task.status == TaskStatus::Done
                    && local_date(task.updated_at, local_offset_at).is_some_and(|date| date < end)
            })
            .count();
        weeks.push(json!({
            "week": cursor.to_string(),
            "created": created,
            "completedByUpdate": completed,
        }));
        cursor = end;
    }
    weeks
}

/// Monday of the week a date falls in.
fn week_start(date: Date) -> Date {
    let back = i64::from(date.weekday().number_days_from_monday());
    date - Duration::days(back)
}

/// How long tasks take from creation to reaching done.
///
/// Only tasks currently done are counted, and the elapsed time is measured to
/// their last update, so this describes the shape of the project's work rather
/// than a certified duration. Tasks whose update precedes their creation — a
/// clock change, or an imported record — are dropped rather than clamped to
/// zero, because a false pile at "under a day" would read as a real result.
fn lead_time(snapshot: &ProjectSnapshot) -> Value {
    let mut elapsed: Vec<i64> = snapshot
        .tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Done)
        .filter_map(|task| {
            let created = datetime(task.created_at)?;
            let updated = datetime(task.updated_at)?;
            let days = (updated - created).whole_days();
            (updated >= created).then_some(days)
        })
        .collect();
    elapsed.sort_unstable();

    let mut counts = vec![0usize; LEAD_TIME_EDGES.len() + 1];
    for days in &elapsed {
        let slot = LEAD_TIME_EDGES
            .iter()
            .position(|edge| days < edge)
            .unwrap_or(LEAD_TIME_EDGES.len());
        counts[slot] += 1;
    }

    let buckets: Vec<Value> = counts
        .iter()
        .enumerate()
        .map(|(index, count)| json!({ "label": lead_time_label(index), "count": count }))
        .collect();

    json!({
        "buckets": buckets,
        "counted": elapsed.len(),
        "medianDays": median(&elapsed),
    })
}

fn lead_time_label(index: usize) -> String {
    match index {
        0 => "under a day".to_owned(),
        _ if index < LEAD_TIME_EDGES.len() => {
            format!(
                "{}-{} days",
                LEAD_TIME_EDGES[index - 1],
                LEAD_TIME_EDGES[index]
            )
        }
        _ => format!("over {} days", LEAD_TIME_EDGES[LEAD_TIME_EDGES.len() - 1]),
    }
}

/// Median of a sorted slice, `None` when there is nothing to take it from.
fn median(sorted: &[i64]) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        Some(sorted[middle])
    } else {
        Some(i64::midpoint(sorted[middle - 1], sorted[middle]))
    }
}

/// Task progress per plan, newest plan first, archived plans excluded.
fn plans(snapshot: &ProjectSnapshot) -> Vec<Value> {
    snapshot
        .plans
        .iter()
        .map(|plan| {
            let tasks = snapshot
                .tasks
                .iter()
                .filter(|task| task.plan_id == plan.id)
                .collect::<Vec<_>>();
            let done = tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Done)
                .count();
            let blocked = tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Blocked)
                .count();
            json!({
                "id": plan.id,
                "title": plan.title,
                "status": plan.status.as_str(),
                "total": tasks.len(),
                "done": done,
                "blocked": blocked,
            })
        })
        .collect()
}

/// Issues split by severity, plus how long the open ones have been waiting.
fn issues(
    snapshot: &ProjectSnapshot,
    today: Date,
    local_offset_at: impl Fn(OffsetDateTime) -> UtcOffset + Copy,
) -> Value {
    let severities = [
        Severity::Critical,
        Severity::High,
        Severity::Medium,
        Severity::Low,
    ];
    let by_severity: Vec<Value> = severities
        .iter()
        .map(|severity| {
            let open = snapshot
                .issues
                .iter()
                .filter(|issue| issue.severity == *severity && issue.status == IssueStatus::Open)
                .count();
            let closed = snapshot
                .issues
                .iter()
                .filter(|issue| issue.severity == *severity && issue.status == IssueStatus::Closed)
                .count();
            json!({ "severity": severity.as_str(), "open": open, "closed": closed })
        })
        .collect();

    let mut open_ages: Vec<Value> = snapshot
        .issues
        .iter()
        .filter(|issue| issue.status == IssueStatus::Open)
        .filter_map(|issue| {
            let opened = local_date(issue.created_at, local_offset_at)?;
            Some(json!({
                "id": issue.id,
                "severity": issue.severity.as_str(),
                "days": (today - opened).whole_days().max(0),
            }))
        })
        .collect();
    open_ages.sort_by_key(|value| -value["days"].as_i64().unwrap_or_default());

    json!({ "bySeverity": by_severity, "openAges": open_ages })
}

/// Notes and commits laid out by weekday and hour of day.
///
/// Only activity inside the window counts, so the grid describes how the
/// project is being worked now rather than how it was worked a year ago.
fn punchcard(
    snapshot: &ProjectSnapshot,
    first: Date,
    local_offset_at: impl Fn(OffsetDateTime) -> UtcOffset + Copy,
) -> Vec<Value> {
    let mut grid = BTreeMap::<(u8, u8), usize>::new();
    let moments = snapshot
        .notes
        .iter()
        .map(|note| note.created_at)
        .chain(snapshot.commits.iter().map(|commit| commit.created_at));
    for at in moments {
        let Some(local) = local_datetime(at, local_offset_at) else {
            continue;
        };
        if local.date() < first {
            continue;
        }
        let weekday = weekday_index(local.weekday());
        *grid.entry((weekday, local.hour())).or_default() += 1;
    }
    grid.into_iter()
        .map(|((weekday, hour), count)| json!({ "weekday": weekday, "hour": hour, "count": count }))
        .collect()
}

fn weekday_index(weekday: Weekday) -> u8 {
    weekday.number_days_from_monday()
}

/// Whole-project counts, so the page can lead with what the project is.
fn totals(snapshot: &ProjectSnapshot) -> Value {
    let done = snapshot
        .tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Done)
        .count();
    let blocked = snapshot
        .tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Blocked)
        .count();
    let plans_done = snapshot
        .plans
        .iter()
        .filter(|plan| plan.status == PlanStatus::Done)
        .count();
    json!({
        "tasks": snapshot.tasks.len(),
        "tasksDone": done,
        "tasksBlocked": blocked,
        "plans": snapshot.plans.len(),
        "plansDone": plans_done,
        "milestones": snapshot.milestones.len(),
        "notes": snapshot.notes.len(),
        "commits": snapshot.commits.len(),
        "issuesOpen": snapshot
            .issues
            .iter()
            .filter(|issue| issue.status == IssueStatus::Open)
            .count(),
        "issuesClosed": snapshot
            .issues
            .iter()
            .filter(|issue| issue.status == IssueStatus::Closed)
            .count(),
    })
}
