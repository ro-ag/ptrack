//! Repository history for the Insights timeline.
//!
//! The snapshot capture reads the forty most recent commits, which is the right
//! bound for a "what happened lately" panel and far too small to draw a
//! project's life. This module asks git a narrower question over a much longer
//! range: when each commit landed, and when each tag was created. Nothing but
//! timestamps and tag names crosses the boundary, so the output stays small
//! even across thousands of commits.

use std::ffi::OsString;
use std::path::Path;

use crate::runner::{CancellationToken, ExecRunner, RepositoryError, Runner};
use crate::snapshot::ExecutionSession;

/// Most commits the timeline will read. At roughly eleven bytes per record this
/// stays comfortably inside the session's aggregate byte budget, and a project
/// with more commits than this is served just as well by the shape of the most
/// recent eight thousand.
pub const MAX_TIMELINE_COMMITS: usize = 8_000;

/// Most tags the timeline will read.
pub const MAX_TIMELINE_TAGS: usize = 500;

/// A repository's commit and tag history, oldest first.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Timeline {
    /// Commit author timestamps, in seconds since the epoch, ascending.
    pub commits: Vec<i64>,
    /// Tags with the instant they were created, ascending.
    pub tags: Vec<TimelineTag>,
    /// Set when the commit list hit [`MAX_TIMELINE_COMMITS`], so the interface
    /// can say the history is longer than what is drawn instead of implying
    /// the project began at the oldest commit shown.
    pub truncated: bool,
}

/// A tag and when it was created.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelineTag {
    pub name: String,
    pub at: i64,
}

/// Reads commit and tag history for a repository.
///
/// # Errors
///
/// Returns a content-free error when cancellation, a resource bound, subprocess
/// execution, or parsing fails.
pub fn capture_timeline(
    cancellation: &CancellationToken,
    root: impl AsRef<Path>,
) -> Result<Timeline, RepositoryError> {
    capture_timeline_with(&ExecRunner::default(), cancellation, root.as_ref())
}

pub(crate) fn capture_timeline_with(
    runner: &dyn Runner,
    cancellation: &CancellationToken,
    root: &Path,
) -> Result<Timeline, RepositoryError> {
    let mut session = ExecutionSession::new(runner, cancellation);

    let log = session.run(root, &commit_args())?;
    let mut commits = parse_commits(&log);
    let truncated = commits.len() > MAX_TIMELINE_COMMITS;
    commits.truncate(MAX_TIMELINE_COMMITS);
    // git log reports newest first; a timeline reads the other way.
    commits.reverse();

    let tags = match session.run(root, &tag_args()) {
        Ok(output) => parse_tags(&output),
        // A repository with no tags is ordinary, and so is one where the tag
        // read fails on its own; the commit history is still worth drawing.
        Err(RepositoryError::AggregateLimit) => return Err(RepositoryError::AggregateLimit),
        Err(_) => Vec::new(),
    };

    Ok(Timeline {
        commits,
        tags,
        truncated,
    })
}

fn commit_args() -> Vec<OsString> {
    vec![
        OsString::from("log"),
        OsString::from("-n"),
        OsString::from((MAX_TIMELINE_COMMITS + 1).to_string()),
        OsString::from("--date=unix"),
        OsString::from("--format=%at"),
    ]
}

fn tag_args() -> Vec<OsString> {
    vec![
        OsString::from("tag"),
        OsString::from("--list"),
        OsString::from("--sort=creatordate"),
        OsString::from(format!("--count={MAX_TIMELINE_TAGS}")),
        OsString::from("--format=%(creatordate:unix)\u{1f}%(refname:short)"),
    ]
}

/// Parses one epoch second per line, dropping anything that is not a number.
///
/// A malformed line is skipped rather than failing the read: a single unusual
/// record in a long history should not cost the whole timeline.
fn parse_commits(output: &[u8]) -> Vec<i64> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| line.trim().parse::<i64>().ok())
        .collect()
}

/// Parses `<epoch seconds>\u{1f}<tag name>` per line, oldest first.
fn parse_tags(output: &[u8]) -> Vec<TimelineTag> {
    let mut tags: Vec<TimelineTag> = String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let (at, name) = line.trim().split_once('\u{1f}')?;
            let at = at.trim().parse::<i64>().ok()?;
            let name = name.trim();
            (!name.is_empty()).then(|| TimelineTag {
                name: name.to_owned(),
                at,
            })
        })
        .collect();
    tags.sort_by_key(|tag| tag.at);
    tags.truncate(MAX_TIMELINE_TAGS);
    tags
}
