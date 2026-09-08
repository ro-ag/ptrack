//! Tracked-path collection for deterministic stack discovery.
//!
//! `git ls-files` is the single source of truth for project membership:
//! ignored, untracked, and vendored-but-ignored files never appear, so
//! `.gitignore` decides what counts without this crate reimplementing it.
//!
//! Only collection lives here. Turning paths into languages is the resolver's
//! job in `ptrack-core`, which keeps this crate free of model dependencies.

use std::path::Path;
use std::str;

use crate::runner::{CancellationToken, RepositoryError, args};
use crate::snapshot::RepositoryService;

/// The most tracked paths one scan reads before reporting truncation.
pub const MAX_TRACKED_PATHS: usize = 200_000;

/// One tracked path and, when the line pass ran, its line count.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackedPath {
    pub path: String,
    pub lines: u32,
}

/// A bounded listing of a repository's tracked paths.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackedPaths {
    /// Repository-relative paths, sorted, at most [`MAX_TRACKED_PATHS`].
    pub paths: Vec<TrackedPath>,
    /// The listing hit the cap and was truncated.
    pub incomplete: bool,
    /// Line counts were collected. False when the repository has no commit yet
    /// or the count could not run, in which case every count is zero.
    pub lines_counted: bool,
}

impl RepositoryService {
    /// Lists the repository's tracked paths, sorted and bounded.
    ///
    /// # Errors
    ///
    /// Returns a content-free error when cancellation, a resource bound,
    /// subprocess execution, or decoding fails.
    pub fn capture_tracked_paths(
        &self,
        cancellation: &CancellationToken,
        root: impl AsRef<Path>,
    ) -> Result<TrackedPaths, RepositoryError> {
        let output = self.runner().output(
            cancellation,
            root.as_ref(),
            &args(["ls-files", "-z", "--deduplicate"]),
        )?;
        let mut paths = parse_tracked_paths(&output)?;
        paths.sort();
        let incomplete = paths.len() > MAX_TRACKED_PATHS;
        paths.truncate(MAX_TRACKED_PATHS);

        // Counting lines is a second, independently failing pass: a repository
        // with no commit yet has nothing to count, and a listing that defeats
        // the byte ceiling still yields a usable file-count profile.
        let counts = self.capture_line_counts(cancellation, root.as_ref()).ok();
        let lines_counted = counts.is_some();
        let counts = counts.unwrap_or_default();
        let paths = paths
            .into_iter()
            .map(|path| {
                let lines = counts
                    .binary_search_by(|(candidate, _)| candidate.as_str().cmp(path.as_str()))
                    .map_or(0, |index| counts[index].1);
                TrackedPath { path, lines }
            })
            .collect();

        Ok(TrackedPaths {
            paths,
            incomplete,
            lines_counted,
        })
    }

    /// Counts lines per tracked text file at HEAD.
    ///
    /// `git grep -I` skips binary files, so no heuristic is needed here, and
    /// counting at HEAD rather than on disk keeps the answer reproducible from
    /// the same commit. Files with no lines simply do not appear.
    fn capture_line_counts(
        &self,
        cancellation: &CancellationToken,
        root: &Path,
    ) -> Result<Vec<(String, u32)>, RepositoryError> {
        let output = self.runner().output(
            cancellation,
            root,
            // "^" matches every line and is portable; an empty pattern is
            // rejected outright by some git versions.
            &args(["grep", "-I", "-c", "-z", "-e", "^", "HEAD"]),
        )?;
        let mut counts = parse_line_counts(&output);
        counts.sort();
        counts.dedup_by(|left, right| left.0 == right.0);
        Ok(counts)
    }
}

/// Parses `git grep -c -z` records shaped `HEAD:<path>\0<count>`.
///
/// A record that does not parse is skipped rather than failing the scan: the
/// file then reports no lines, which the profile already models.
fn parse_line_counts(output: &[u8]) -> Vec<(String, u32)> {
    output
        .split(|byte| *byte == b'\n')
        .filter_map(|record| {
            let record = str::from_utf8(record).ok()?;
            let (path, count) = record.split_once('\0')?;
            let path = path.strip_prefix("HEAD:")?;
            Some((path.to_owned(), count.trim().parse::<u32>().ok()?))
        })
        .collect()
}

/// Splits a NUL-delimited `ls-files` listing into repository-relative paths.
fn parse_tracked_paths(output: &[u8]) -> Result<Vec<String>, RepositoryError> {
    output
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            str::from_utf8(entry)
                .map(str::to_owned)
                .map_err(|_| RepositoryError::InvalidData("tracked path is not UTF-8"))
        })
        .collect()
}
