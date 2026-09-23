//! The deterministic stack profile and the rule deciding when to rescan it.

use ptrack_core::StackProfile;
use serde::Serialize;

use super::support::timestamp;

/// The deterministic stack profile served to the Overview and the Repository
/// panel.
///
/// Languages are discovered from tracked manifests and counted in tracked
/// files. Sizes and line counts are deliberately absent: one vendored
/// directory or generated bundle outweighs the code that defines a project.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StackProfileView {
    /// `scanning` is never served by this synchronous command; the frontend
    /// shows it while the call is in flight.
    pub(super) state: &'static str,
    pub(super) scanned_head: String,
    pub(super) scanned_at: String,
    pub(super) tracked_files: u32,
    /// Lines across counted tracked files; zero when `lines_counted` is false.
    pub(super) lines: u32,
    pub(super) lines_counted: bool,
    pub(super) incomplete: bool,
    pub(super) projects: Vec<StackProjectView>,
}

/// One discovered project and the manifests that prove it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StackProjectView {
    pub(super) root: String,
    pub(super) language: String,
    pub(super) files: u32,
    pub(super) lines: u32,
    pub(super) evidence: Vec<String>,
}

impl StackProfileView {
    pub(super) fn unavailable() -> Self {
        Self::empty("unavailable")
    }

    pub(super) fn failed() -> Self {
        Self::empty("failed")
    }

    fn empty(state: &'static str) -> Self {
        Self {
            state,
            scanned_head: String::new(),
            scanned_at: String::new(),
            tracked_files: 0,
            lines: 0,
            lines_counted: false,
            incomplete: false,
            projects: Vec::new(),
        }
    }

    pub(super) fn ready(profile: &StackProfile) -> Self {
        Self {
            state: "ready",
            scanned_head: profile.scanned_head.clone(),
            scanned_at: timestamp(profile.scanned_at),
            tracked_files: profile.tracked_files,
            lines: profile.lines,
            lines_counted: profile.lines_counted,
            incomplete: profile.incomplete,
            projects: profile
                .projects
                .iter()
                .map(|project| StackProjectView {
                    root: project.root.clone(),
                    language: project.language.as_str().to_owned(),
                    files: project.files,
                    lines: project.lines,
                    evidence: project.evidence.clone(),
                })
                .collect(),
        }
    }
}

/// What one scan attempt does to durable state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StackScanOutcome {
    /// Persist this profile and its registry summary, then serve it.
    Store(StackProfile),
    /// Serve the stored profile; nothing is written.
    Serve(StackProfile),
    /// The scan failed. Nothing is written; the stored profile stands.
    Failed(Option<StackProfile>),
    /// There is no HEAD to scan against.
    Unavailable,
}

/// Reports whether a tracked-path scan is due.
///
/// A truncated profile is exempt from HEAD-driven rescans: its listing was
/// already over the cap, so re-reading it on every commit costs far more than
/// the staleness it removes. Project open and explicit rescan still scan it.
pub(crate) fn stack_scan_due(stored: Option<&StackProfile>, head: &str) -> bool {
    match stored {
        None => true,
        Some(profile) if profile.incomplete => false,
        Some(profile) => profile.scanned_head != head,
    }
}

/// Decides the outcome of one scan attempt.
///
/// A failure never writes, so a transient git error cannot clear counts that a
/// successful scan established.
pub(crate) fn stack_scan_outcome(
    stored: Option<StackProfile>,
    head: Option<&str>,
    force: bool,
    scan: impl FnOnce() -> Option<StackProfile>,
) -> StackScanOutcome {
    let Some(head) = head else {
        return StackScanOutcome::Unavailable;
    };
    if !force && !stack_scan_due(stored.as_ref(), head) {
        return stored.map_or(StackScanOutcome::Unavailable, StackScanOutcome::Serve);
    }
    scan().map_or(StackScanOutcome::Failed(stored), StackScanOutcome::Store)
}
