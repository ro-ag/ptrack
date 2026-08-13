use std::collections::BTreeSet;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use crate::intelligence::current_run_events;
use crate::privacy::{
    contains_credential_like, contains_reasoning_marker, contains_rejected_summary_credential,
    redact_summary, valid_text,
};
use crate::{Event, EventKind, IntelligenceConfidence, Run, derive_run_intelligence};

const MAX_HANDOFF_EVENTS: usize = 8;
const MAX_HANDOFF_BYTES: usize = 2 * 1024;
const MAX_HANDOFF_SCALAR_BYTES: usize = 512;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffPreview {
    pub text: String,
    pub included_event_ids: Vec<String>,
    pub considered_events: usize,
    pub truncated: bool,
}

#[must_use]
pub fn build_handoff_preview(run: &Run, events: &[Event]) -> HandoffPreview {
    let relevant = current_handoff_events(run, events);
    let intelligence = derive_run_intelligence(run, &relevant);
    let confidence = match intelligence.confidence {
        IntelligenceConfidence::Unset => IntelligenceConfidence::Low,
        value => value,
    };
    let mut lines = vec![format!(
        "Agent run state: {} ({} confidence).",
        intelligence.state.as_str(),
        confidence.as_str()
    )];
    lines.push(match run.association.as_ref() {
        Some(value) if value.target.task_id != 0 => format!(
            "Context: plan #{}, task #{}.",
            value.target.plan_id, value.target.task_id
        ),
        Some(value) if value.target.plan_id != 0 => {
            format!("Context: plan #{}.", value.target.plan_id)
        }
        Some(_) => "Context: project.".to_owned(),
        None => "Context: project (no current plan or task association).".to_owned(),
    });
    let mut preview = HandoffPreview {
        text: String::new(),
        included_event_ids: Vec::new(),
        considered_events: relevant.len(),
        truncated: false,
    };
    let mut seen_lines = BTreeSet::new();
    for event in relevant.iter().rev() {
        let line = handoff_event_line(event);
        if line.is_empty() || !seen_lines.insert(line.clone()) {
            continue;
        }
        if preview.included_event_ids.len() == MAX_HANDOFF_EVENTS {
            preview.truncated = true;
            break;
        }
        lines.push(format!("- {line}"));
        preview.included_event_ids.push(event.id.clone());
    }
    if preview.included_event_ids.is_empty() {
        lines
            .push("No retained structured work-product events for the current context.".to_owned());
    }
    (preview.text, preview.truncated) = bounded_handoff_text(&lines.join("\n"), preview.truncated);
    preview
}

fn current_handoff_events(run: &Run, events: &[Event]) -> Vec<Event> {
    current_run_events(run, events)
        .into_iter()
        .filter(|event| {
            let correlation = &event.correlation;
            if correlation.project_root != run.project_root
                || correlation.terminal_id != run.terminal_id
            {
                return false;
            }
            match run.association.as_ref() {
                None => {
                    correlation.plan_id == 0
                        && correlation.task_id == 0
                        && correlation.generation == 0
                        && correlation.association_revision == 0
                }
                Some(current) => {
                    correlation.plan_id == current.target.plan_id
                        && correlation.task_id == current.target.task_id
                        && correlation.generation == current.generation
                        && correlation.association_revision == current.revision
                }
            }
        })
        .collect()
}

fn handoff_event_line(event: &Event) -> String {
    let phase = event.phase.as_str();
    let subject = safe_handoff_scalar(&event.subject);
    match event.kind {
        EventKind::Lifecycle => format!("Lifecycle {phase}."),
        EventKind::Tool => handoff_subject_line("Tool", phase, &subject),
        EventKind::Command => handoff_subject_line("Command", phase, &subject),
        EventKind::File => {
            let paths = safe_handoff_paths(&event.paths);
            if paths.is_empty() {
                format!("File activity {phase}.")
            } else {
                format!("File activity {phase}: {paths}.")
            }
        }
        EventKind::Test => handoff_subject_line("Test", phase, &subject),
        EventKind::Commit => {
            let sha = safe_handoff_scalar(&event.commit_sha);
            let sha = valid_prefix(&sha, 12);
            handoff_subject_line("Commit", phase, sha)
        }
        EventKind::Error => {
            handoff_subject_line("Error", phase, &safe_handoff_scalar(&event.error_class))
        }
        EventKind::Summary => safe_handoff_summary(&event.summary),
        EventKind::Unset => String::new(),
    }
}

fn safe_handoff_summary(value: &str) -> String {
    if contains_reasoning_marker(value) || contains_rejected_summary_credential(value) {
        return String::new();
    }
    let summary = redact_summary(&value.split_whitespace().collect::<Vec<_>>().join(" "));
    if summary.is_empty() || !valid_text(&summary, true) {
        String::new()
    } else {
        format!("Agent-provided summary: {summary}")
    }
}

fn handoff_subject_line(kind: &str, phase: &str, subject: &str) -> String {
    if subject.is_empty() {
        format!("{kind} {phase}.")
    } else {
        format!("{kind} {phase}: {subject}.")
    }
}

fn safe_handoff_scalar(value: &str) -> String {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_HANDOFF_SCALAR_BYTES
        || !valid_text(value, false)
        || contains_credential_like(value)
        || contains_reasoning_marker(value)
    {
        String::new()
    } else {
        value.to_owned()
    }
}

fn safe_handoff_paths(paths: &[String]) -> String {
    paths
        .iter()
        .filter_map(|value| {
            if safe_handoff_scalar(value).is_empty() || Path::new(value).is_absolute() {
                return None;
            }
            let mut clean = Vec::new();
            for component in Path::new(value).components() {
                match component {
                    Component::Normal(component) => {
                        clean.push(component.to_string_lossy().into_owned());
                    }
                    Component::CurDir => {}
                    Component::ParentDir => {
                        clean.pop()?;
                    }
                    Component::RootDir | Component::Prefix(_) => return None,
                }
            }
            (!clean.is_empty()).then(|| clean.join("/"))
        })
        .take(3)
        .collect::<Vec<_>>()
        .join(", ")
}

fn bounded_handoff_text(value: &str, already_truncated: bool) -> (String, bool) {
    if value.len() <= MAX_HANDOFF_BYTES {
        return (value.to_owned(), already_truncated);
    }
    let prefix = valid_prefix(value, MAX_HANDOFF_BYTES - "\n…".len()).trim_end();
    (format!("{prefix}\n…"), true)
}

fn valid_prefix(value: &str, byte_limit: usize) -> &str {
    let mut end = 0;
    for (index, character) in value.char_indices() {
        if index + character.len_utf8() > byte_limit {
            break;
        }
        end = index + character.len_utf8();
    }
    &value[..end]
}
