use std::fmt::Write as _;

use crate::report::{
    claim_marker, hold_marker, inline_text, issue_line, note_line, note_markdown, task_line,
};
use crate::views::plan_ref;
use crate::{IssueLine, MilestoneRef, NoteLine, PlanRef, ProjectSnapshot, TaskLine};

/// Characters of note body a search result shows around its match.
pub(crate) const SEARCH_SNIPPET_CHARS: usize = 120;
/// Characters of that window placed before the match, so the hit is in view.
const SNIPPET_LEAD_CHARS: usize = 40;

/// Substring matches across milestones, plans, tasks, issues, and notes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchView {
    pub term: String,
    pub milestones: Vec<MilestoneRef>,
    pub plans: Vec<PlanRef>,
    pub tasks: Vec<TaskLine>,
    pub issues: Vec<IssueLine>,
    pub notes: Vec<NoteLine>,
}

/// Matches a case-insensitive substring against the Go report service's exact
/// set of searchable fields. An empty term intentionally matches every item.
#[must_use]
pub fn search(snapshot: &ProjectSnapshot, term: &str) -> SearchView {
    let needle = simple_lowercase(term);
    let has = |value: &str| simple_lowercase(value).contains(&needle);

    SearchView {
        term: term.to_owned(),
        milestones: snapshot
            .milestones
            .iter()
            .filter(|milestone| has(&milestone.title))
            .map(|milestone| MilestoneRef {
                id: milestone.id,
                title: milestone.title.clone(),
                status: milestone.status.as_str().to_owned(),
            })
            .collect(),
        plans: snapshot
            .plans
            .iter()
            .filter(|plan| has(&plan.title))
            .map(|plan| plan_ref(&snapshot.meta, plan))
            .collect(),
        tasks: snapshot
            .tasks
            .iter()
            .filter(|task| has(&task.title))
            .map(task_line)
            .collect(),
        issues: snapshot
            .issues
            .iter()
            .filter(|issue| has(&issue.title) || has(&issue.body))
            .map(issue_line)
            .collect(),
        notes: snapshot
            .notes
            .iter()
            .filter(|note| has(&note.body))
            .map(note_line)
            .collect(),
    }
}

impl SearchView {
    /// Renders the grouped search Markdown.
    ///
    /// No matches renders nothing at all, as the query-surface spec requires,
    /// so a script can test for empty output. Titles render on one line, and a
    /// note shows a bounded single-line snippet around its first match rather
    /// than its whole body.
    #[must_use]
    pub fn markdown(&self) -> String {
        if self.milestones.is_empty()
            && self.plans.is_empty()
            && self.tasks.is_empty()
            && self.issues.is_empty()
            && self.notes.is_empty()
        {
            return String::new();
        }
        let mut output = format!("# Search: {}\n\n", go_quote(&self.term));
        if !self.milestones.is_empty() {
            output.push_str("## Milestones\n");
            for milestone in &self.milestones {
                writeln!(
                    &mut output,
                    "- #{} {} [{}]",
                    milestone.id,
                    inline_text(&milestone.title),
                    milestone.status
                )
                .expect("writing to String cannot fail");
            }
            output.push('\n');
        }
        if !self.plans.is_empty() {
            output.push_str("## Plans\n");
            for plan in &self.plans {
                writeln!(
                    &mut output,
                    "- #{} {} [{}]{}{}",
                    plan.id,
                    inline_text(&plan.title),
                    plan.status,
                    hold_marker(plan.hold_reason.as_deref()),
                    claim_marker(
                        plan.claimed_by_name
                            .as_deref()
                            .or(plan.claimed_by.as_deref())
                    )
                )
                .expect("writing to String cannot fail");
            }
            output.push('\n');
        }
        if !self.tasks.is_empty() {
            output.push_str("## Tasks\n");
            for task in &self.tasks {
                writeln!(
                    &mut output,
                    "- [{}] #{} {} (plan {}){}",
                    task.status,
                    task.id,
                    inline_text(&task.title),
                    task.plan_id,
                    hold_marker(task.hold_reason.as_deref())
                )
                .expect("writing to String cannot fail");
            }
            output.push('\n');
        }
        if !self.issues.is_empty() {
            output.push_str("## Issues\n");
            for issue in &self.issues {
                writeln!(
                    &mut output,
                    "- #{} [{}] {} ({})",
                    issue.id,
                    issue.severity,
                    inline_text(&issue.title),
                    issue.status
                )
                .expect("writing to String cannot fail");
            }
            output.push('\n');
        }
        if !self.notes.is_empty() {
            output.push_str("## Notes\n");
            for note in &self.notes {
                let line = NoteLine {
                    body: snippet(&note.body, &self.term),
                    ..note.clone()
                };
                output.push_str("- ");
                output.push_str(&note_markdown(&line));
                output.push('\n');
            }
        }
        output
    }
}

/// Returns at most [`SEARCH_SNIPPET_CHARS`] characters of `body` around the
/// first case-insensitive match of `term`, on one line, with `…` marking each
/// side that was cut.
///
/// The match is located in the lowercased copy, whose characters correspond
/// one to one with the original's, so the window is cut on the original's
/// character boundaries even where lowercasing changes a character's width.
fn snippet(body: &str, term: &str) -> String {
    let lowered = simple_lowercase(body);
    let hit = lowered
        .find(&simple_lowercase(term))
        .map_or(0, |offset| lowered[..offset].chars().count());
    let total = body.chars().count();
    let mut start = hit.saturating_sub(SNIPPET_LEAD_CHARS);
    let end = (start + SEARCH_SNIPPET_CHARS).min(total);
    start = start.min(end.saturating_sub(SEARCH_SNIPPET_CHARS));
    let mut window = String::new();
    if start > 0 {
        window.push('…');
    }
    window.extend(body.chars().skip(start).take(end - start));
    if end < total {
        window.push('…');
    }
    inline_text(&window).into_owned()
}

fn go_quote(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '\u{0007}' => output.push_str("\\a"),
            '\u{0008}' => output.push_str("\\b"),
            '\u{000C}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{000B}' => output.push_str("\\v"),
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            value if !go_is_print(value) && u32::from(value) < 0x80 => {
                write!(&mut output, "\\x{:02x}", u32::from(value))
                    .expect("writing to String cannot fail");
            }
            value if !go_is_print(value) && u32::from(value) <= 0xffff => {
                write!(&mut output, "\\u{:04x}", u32::from(value))
                    .expect("writing to String cannot fail");
            }
            value if !go_is_print(value) => {
                write!(&mut output, "\\U{:08x}", u32::from(value))
                    .expect("writing to String cannot fail");
            }
            value => output.push(value),
        }
    }
    output.push('"');
    output
}

fn simple_lowercase(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            // Go's strings.ToLower applies unicode.ToLower rune by rune. Rust
            // exposes full lowercase mappings; the only unconditional
            // multi-rune lowercase special case is U+0130 (İ), whose Unicode
            // simple mapping is the first rune, `i`.
            character
                .to_lowercase()
                .next()
                .expect("a lowercase mapping is never empty")
        })
        .collect()
}

fn go_is_print(character: char) -> bool {
    if matches!(character, '"' | '\'' | '\\') {
        return true;
    }
    let mut escaped = character.escape_debug();
    escaped.next() == Some(character) && escaped.next().is_none()
}
