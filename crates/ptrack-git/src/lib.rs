//! Bounded, read-only Git repository and worktree intelligence.
//!
//! Repository content is untrusted. All subprocesses are argument-vector
//! launches with fixed environment and resource bounds; this crate grants no
//! authority to mutate a repository or contact a remote.

mod model;
mod runner;
mod snapshot;
mod status;
mod worktree;

pub use model::{
    Branch, ChangedArea, Commit, Divergence, ExistingWorktree, PathBounds, Remote, RepositoryState,
    Snapshot, Status, WorktreeBounds, WorktreeIdentity,
};
pub use runner::{CancellationToken, RepositoryError};
pub use snapshot::{RepositoryService, capture};
pub use worktree::inspect_worktree;

#[cfg(test)]
mod runner_test;
#[cfg(test)]
mod snapshot_test;
#[cfg(test)]
mod status_test;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod worktree_test;
