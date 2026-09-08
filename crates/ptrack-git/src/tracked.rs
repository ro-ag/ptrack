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

/// A bounded listing of a repository's tracked paths.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackedPaths {
    /// Repository-relative paths, sorted, at most [`MAX_TRACKED_PATHS`].
    pub paths: Vec<String>,
    /// The listing hit the cap and was truncated.
    pub incomplete: bool,
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
        Ok(TrackedPaths { paths, incomplete })
    }
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
