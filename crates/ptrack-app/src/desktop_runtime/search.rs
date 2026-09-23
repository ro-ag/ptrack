//! Workspace search and the activity heatmap.

use std::collections::BTreeMap;

use ptrack_core::{MemoryKind, Note, NoteTarget, ProjectSnapshot};
use serde_json::{Value, json};
use time::{OffsetDateTime, UtcOffset};

use super::support::timestamp_datetime;
use super::{SEARCH_RESULT_LIMIT, SEARCH_SNIPPET_SPAN};

pub(crate) fn search(snapshot: &ProjectSnapshot, query: &str) -> Vec<Value> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut results = Vec::new();
    for plan in &snapshot.plans {
        if plan.title.to_lowercase().contains(&needle) {
            results.push(json!({
                "kind": "plan",
                "id": plan.id,
                "planId": plan.id,
                "title": plan.title,
                "snippet": "",
                "status": plan.status.as_str()
            }));
        }
        if results.len() == SEARCH_RESULT_LIMIT {
            return results;
        }
    }
    for task in &snapshot.tasks {
        if task.title.to_lowercase().contains(&needle) {
            results.push(json!({
                "kind": "task",
                "id": task.id,
                "planId": task.plan_id,
                "title": task.title,
                "snippet": "",
                "status": task.status.as_str()
            }));
        }
        if results.len() == SEARCH_RESULT_LIMIT {
            return results;
        }
    }
    for issue in &snapshot.issues {
        if issue.title.to_lowercase().contains(&needle)
            || issue.body.to_lowercase().contains(&needle)
        {
            results.push(json!({"kind": "issue", "id": issue.id,
                "planId": snapshot.task(issue.task_id).map_or(0, |task| task.plan_id),
                "title": issue.title, "snippet": "", "status": issue.status.as_str()}));
        }
        if results.len() == SEARCH_RESULT_LIMIT {
            return results;
        }
    }
    for note in &snapshot.notes {
        if let Some((start, end)) = find_case_insensitive(&note.body, &needle) {
            results.push(json!({
                "kind": "note",
                "id": note.id,
                "planId": if note.target == NoteTarget::Plan { note.target_id } else { 0 },
                "title": note_title(note),
                "snippet": snippet(&note.body, start, end)
            }));
        }
        if results.len() == SEARCH_RESULT_LIMIT {
            break;
        }
    }
    results
}

pub(super) fn note_title(note: &Note) -> String {
    let prefix = if note.kind == MemoryKind::Legacy {
        String::new()
    } else {
        let mut chars = note.kind.as_str().chars();
        chars.next().map_or_else(String::new, |first| {
            format!("{}{} · ", first.to_uppercase(), chars.as_str())
        })
    };
    format!(
        "{prefix}{} note",
        match note.target {
            NoteTarget::Project => "Project",
            NoteTarget::Plan => "Plan",
            NoteTarget::Task => "Task",
        }
    )
}

/// Finds `needle` — already lowercased — in `haystack` ignoring case, and
/// returns the byte range of the match in `haystack` itself.
///
/// Offsets found in `haystack.to_lowercase()` do not address the original:
/// lowercasing changes byte lengths (`İ` grows, the Kelvin sign `K` and `ẞ`
/// shrink), so they land on the wrong text or inside a character. Matching the
/// lowercase expansion of each original character keeps every offset on a
/// boundary of the string it indexes.
pub(crate) fn find_case_insensitive(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    haystack.char_indices().find_map(|(start, _)| {
        lowercase_match_len(&haystack[start..], needle).map(|length| (start, start + length))
    })
}

/// The byte length of the shortest prefix of `text` whose lowercase form
/// begins with `needle`, if any. A needle that ends inside one character's
/// expansion takes the whole character.
pub(super) fn lowercase_match_len(text: &str, needle: &str) -> Option<usize> {
    let mut wanted = needle.chars().peekable();
    for (offset, character) in text.char_indices() {
        for lower in character.to_lowercase() {
            match wanted.next() {
                Some(expected) if expected == lower => {}
                Some(_) => return None,
                None => break,
            }
        }
        if wanted.peek().is_none() {
            return Some(offset + character.len_utf8());
        }
    }
    None
}

/// The note text around one match, widened to character boundaries of `body`.
pub(super) fn snippet(body: &str, match_start: usize, match_end: usize) -> String {
    let mut start = match_start.saturating_sub(SEARCH_SNIPPET_SPAN / 2);
    while !body.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = match_end
        .saturating_add(SEARCH_SNIPPET_SPAN / 2)
        .min(body.len());
    while !body.is_char_boundary(end) {
        end += 1;
    }
    let mut result = body[start..end].to_owned();
    if start != 0 {
        result.insert(0, '…');
    }
    if end != body.len() {
        result.push('…');
    }
    result
}

pub(super) fn heatmap(snapshot: &ProjectSnapshot, requested_weeks: i64) -> Vec<Value> {
    heatmap_at(
        snapshot,
        requested_weeks,
        OffsetDateTime::now_utc(),
        |timestamp| UtcOffset::local_offset_at(timestamp).unwrap_or(UtcOffset::UTC),
    )
}

pub(crate) fn heatmap_at(
    snapshot: &ProjectSnapshot,
    requested_weeks: i64,
    now: OffsetDateTime,
    local_offset_at: impl Fn(OffsetDateTime) -> UtcOffset,
) -> Vec<Value> {
    let weeks = if requested_weeks <= 0 {
        16
    } else {
        requested_weeks.min(52)
    };
    let days = usize::try_from(weeks * 7).unwrap_or(112);
    let today = now.to_offset(local_offset_at(now)).date();
    let mut counts = BTreeMap::<String, usize>::new();
    for at in snapshot
        .notes
        .iter()
        .map(|note| note.created_at)
        .chain(snapshot.commits.iter().map(|commit| commit.created_at))
    {
        if let Some(timestamp) = timestamp_datetime(at) {
            let date = timestamp.to_offset(local_offset_at(timestamp)).date();
            *counts.entry(date.to_string()).or_default() += 1;
        }
    }
    (0..days)
        .map(|offset| {
            let ago = i64::try_from(days - 1 - offset).unwrap_or_default();
            let date = today - time::Duration::days(ago);
            let key = date.to_string();
            json!({ "date": key, "count": counts.get(&key).copied().unwrap_or(0) })
        })
        .collect()
}
