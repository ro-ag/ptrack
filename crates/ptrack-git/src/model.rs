use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum RepositoryState {
    #[default]
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "notRepository")]
    NotRepository,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathBounds {
    pub shown: usize,
    pub total: usize,
    pub more: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub oid: String,
    pub branch: String,
    pub upstream: String,
    pub detached: bool,
    pub initial: bool,
    pub ahead: i64,
    pub behind: i64,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicted: usize,
    pub ignored: usize,
    pub changed_paths: Option<Vec<String>>,
    pub untracked_paths: Option<Vec<String>>,
    pub changed_path_bounds: PathBounds,
    pub untracked_path_bounds: PathBounds,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub name: String,
    pub fetch_urls: Option<Vec<String>>,
    pub push_urls: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub name: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub oid: String,
    pub upstream: String,
    pub last_commit_at: String,
    pub current: bool,
    pub remote: bool,
    pub worktree_path: String,
    pub stale: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedArea {
    pub name: String,
    pub files: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub sha: String,
    pub author_name: String,
    pub author_email: String,
    pub date: String,
    pub subject: String,
    pub refs: Vec<String>,
    pub files_changed: usize,
    pub changed_areas: Vec<ChangedArea>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Divergence {
    pub upstream: String,
    pub ahead: i64,
    pub behind: i64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExistingWorktree {
    pub root: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub branch: String,
    pub head: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeBounds {
    pub shown: usize,
    pub total: usize,
    pub more: usize,
}

/// Content-free, host-observed repository metadata. This does not grant
/// permission to read, write, launch, or run Git in the worktree.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeIdentity {
    pub root: String,
    pub git_dir: String,
    pub common_git_dir: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub branch: String,
    pub head: String,
    pub linked: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct Snapshot {
    pub state: RepositoryState,
    pub root: String,
    pub git_dir: String,
    pub common_git_dir: String,
    pub bare: bool,
    pub linked_worktree: bool,
    pub status: Status,
    pub remotes: Option<Vec<Remote>>,
    pub local_branches: Option<Vec<Branch>>,
    pub remote_branches: Option<Vec<Branch>>,
    pub recent_commits: Option<Vec<Commit>>,
    pub unpushed_commits: Option<Vec<Commit>>,
    pub recent_commits_truncated: bool,
    pub unpushed_commits_truncated: bool,
    pub worktrees: Option<Vec<ExistingWorktree>>,
    pub worktree_bounds: WorktreeBounds,
    pub worktrees_incomplete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub divergence: Option<Divergence>,
    pub stale_branch_policy: String,
}
