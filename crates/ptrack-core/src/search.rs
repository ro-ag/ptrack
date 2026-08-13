use std::fmt::Write as _;

use crate::report::{issue_line, note_line, notes_markdown, task_line};
use crate::views::plan_ref;
use crate::{IssueLine, MilestoneRef, NoteLine, PlanRef, ProjectSnapshot, TaskLine};

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
            .map(plan_ref)
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
    /// Renders the exact Go-compatible grouped search Markdown.
    #[must_use]
    pub fn markdown(&self) -> String {
        let mut output = format!("# Search: {}\n\n", go_quote(&self.term));
        if self.milestones.is_empty()
            && self.plans.is_empty()
            && self.tasks.is_empty()
            && self.issues.is_empty()
            && self.notes.is_empty()
        {
            output.push_str("_no matches_\n");
            return output;
        }
        if !self.milestones.is_empty() {
            output.push_str("## Milestones\n");
            for milestone in &self.milestones {
                writeln!(
                    &mut output,
                    "- #{} {} [{}]",
                    milestone.id, milestone.title, milestone.status
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
                    "- #{} {} [{}]",
                    plan.id, plan.title, plan.status
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
                    "- [{}] #{} {} (plan {})",
                    task.status, task.id, task.title, task.plan_id
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
                    issue.id, issue.severity, issue.title, issue.status
                )
                .expect("writing to String cannot fail");
            }
            output.push('\n');
        }
        if !self.notes.is_empty() {
            output.push_str("## Notes\n");
            output.push_str(&notes_markdown(&self.notes));
        }
        output
    }
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
