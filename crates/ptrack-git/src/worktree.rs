use std::path::{Path, PathBuf};

use crate::model::{ExistingWorktree, WorktreeBounds, WorktreeIdentity};
use crate::runner::{CancellationToken, RepositoryError, args};
use crate::snapshot::{ExecutionSession, RepositoryService};

const MAX_WORKTREES: usize = 64;

impl RepositoryService {
    /// Validates a selected path as a registered worktree of the project.
    ///
    /// # Errors
    ///
    /// Returns a content-free error when identity, containment, membership,
    /// cancellation, subprocess, resource-bound, or filesystem checks fail.
    pub fn inspect_worktree(
        &self,
        cancellation: &CancellationToken,
        project_root: impl AsRef<Path>,
        candidate: impl AsRef<Path>,
    ) -> Result<WorktreeIdentity, RepositoryError> {
        let project_root = project_root.as_ref();
        let candidate = candidate.as_ref();
        let canonical_candidate =
            canonical_existing_path(candidate, "canonicalize selected worktree path failed")?;
        let mut session = ExecutionSession::new(self.runner(), cancellation);
        let project = inspect_worktree_identity(&mut session, project_root)?;
        let selected = inspect_worktree_identity(&mut session, candidate)?;
        if project.common_git_dir != selected.common_git_dir {
            return invalid("selected worktree belongs to a different repository");
        }
        if !path_within_root(Path::new(&selected.root), &canonical_candidate) {
            return invalid("selected path is outside the inspected worktree");
        }
        let listed_output = session.run(
            project_root,
            &args(["worktree", "list", "--porcelain", "-z"]),
        )?;
        let (listed, _) = parse_worktree_list(&listed_output)?;
        if !listed.iter().any(|worktree| worktree.root == selected.root) {
            return invalid("selected worktree is not registered with the project repository");
        }
        Ok(selected)
    }
}

/// Validates a selected path with a default repository service.
///
/// # Errors
///
/// Returns a content-free error when identity, containment, membership,
/// cancellation, subprocess, resource-bound, or filesystem checks fail.
pub fn inspect_worktree(
    cancellation: &CancellationToken,
    project_root: impl AsRef<Path>,
    candidate: impl AsRef<Path>,
) -> Result<WorktreeIdentity, RepositoryError> {
    RepositoryService::new().inspect_worktree(cancellation, project_root, candidate)
}

fn inspect_worktree_identity(
    session: &mut ExecutionSession<'_>,
    root: &Path,
) -> Result<WorktreeIdentity, RepositoryError> {
    let output = session.run(
        root,
        &args([
            "rev-parse",
            "--path-format=absolute",
            "--is-inside-work-tree",
            "--show-toplevel",
            "--absolute-git-dir",
            "--git-common-dir",
            "--is-bare-repository",
            "--verify",
            "HEAD",
        ]),
    )?;
    let text = std::str::from_utf8(&output)
        .map_err(|_| RepositoryError::InvalidData("malformed worktree identity"))?;
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() != 6 {
        return invalid("malformed worktree identity");
    }
    let inside = lines[0]
        .parse::<bool>()
        .map_err(|_| RepositoryError::InvalidData("selected path is not a non-bare worktree"))?;
    let bare = lines[4]
        .parse::<bool>()
        .map_err(|_| RepositoryError::InvalidData("selected path is not a non-bare worktree"))?;
    if !inside || bare {
        return invalid("selected path is not a non-bare worktree");
    }

    let canonical_root =
        canonical_existing_path(Path::new(lines[1]), "canonicalize worktree identity failed")?;
    let canonical_git_dir =
        canonical_existing_path(Path::new(lines[2]), "canonicalize worktree identity failed")?;
    let canonical_common_git_dir =
        canonical_existing_path(Path::new(lines[3]), "canonicalize worktree identity failed")?;
    for path in [
        &canonical_root,
        &canonical_git_dir,
        &canonical_common_git_dir,
    ] {
        if !std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
            return invalid("worktree identity path is not a directory");
        }
    }

    let branch = match session.run(
        &canonical_root,
        &args(["symbolic-ref", "--quiet", "--short", "HEAD"]),
    ) {
        Ok(output) => std::str::from_utf8(&output)
            .map_err(|_| RepositoryError::InvalidData("malformed worktree branch"))?
            .trim()
            .to_owned(),
        Err(RepositoryError::CommandFailed) => String::new(),
        Err(error) => return Err(error),
    };
    if branch.contains(['\0', '\r', '\n']) || branch.len() > 512 {
        return invalid("malformed worktree branch");
    }
    let head = lines[5].trim();
    if !valid_object_id(head) {
        return invalid("malformed worktree HEAD");
    }
    let git_dir = path_string(&canonical_git_dir)?;
    let common_git_dir = path_string(&canonical_common_git_dir)?;
    Ok(WorktreeIdentity {
        root: path_string(&canonical_root)?,
        linked: git_dir != common_git_dir,
        git_dir,
        common_git_dir,
        branch,
        head: head.to_ascii_lowercase(),
    })
}

fn path_within_root(root: &Path, candidate: &Path) -> bool {
    candidate.starts_with(root)
}

fn canonical_existing_path(path: &Path, message: &'static str) -> Result<PathBuf, RepositoryError> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|_| RepositoryError::Filesystem(message))?
            .join(path)
    };
    let canonical =
        std::fs::canonicalize(absolute).map_err(|_| RepositoryError::Filesystem(message))?;
    std::fs::metadata(&canonical).map_err(|_| RepositoryError::Filesystem(message))?;
    Ok(normalize_canonical(canonical))
}

#[cfg(windows)]
fn normalize_canonical(path: PathBuf) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    if let Some(local) = value.strip_prefix(r"\\?\") {
        return PathBuf::from(local);
    }
    path
}

#[cfg(not(windows))]
fn normalize_canonical(path: PathBuf) -> PathBuf {
    path
}

fn path_string(path: &Path) -> Result<String, RepositoryError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or(RepositoryError::InvalidData(
            "worktree identity path is not valid UTF-8",
        ))
}

pub(crate) fn parse_worktree_list(
    output: &[u8],
) -> Result<(Vec<ExistingWorktree>, WorktreeBounds), RepositoryError> {
    let mut worktrees = Vec::new();
    let mut total = 0;
    let mut fields = Vec::new();
    for field in output.split(|byte| *byte == 0) {
        if field.is_empty() {
            if !fields.is_empty() {
                parse_worktree_record(&fields, &mut worktrees, &mut total)?;
                fields.clear();
            }
        } else {
            fields.push(field);
        }
    }
    if !fields.is_empty() {
        parse_worktree_record(&fields, &mut worktrees, &mut total)?;
    }
    Ok((
        worktrees,
        WorktreeBounds {
            shown: total.min(MAX_WORKTREES),
            total,
            more: total.saturating_sub(MAX_WORKTREES),
        },
    ))
}

fn parse_worktree_record(
    fields: &[&[u8]],
    worktrees: &mut Vec<ExistingWorktree>,
    total: &mut usize,
) -> Result<(), RepositoryError> {
    let mut candidate = ExistingWorktree::default();
    let mut skip = false;
    for raw in fields {
        let (key, value) = raw
            .iter()
            .position(|byte| *byte == b' ')
            .map_or((*raw, &[][..]), |position| {
                (&raw[..position], &raw[position + 1..])
            });
        match key {
            b"worktree" => {
                utf8(value, "malformed Git worktree list")?.clone_into(&mut candidate.root);
            }
            b"HEAD" => {
                candidate.head = utf8(value, "malformed Git worktree list")?.to_ascii_lowercase();
            }
            b"branch" => {
                let branch = utf8(value, "malformed Git worktree list")?;
                branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch)
                    .clone_into(&mut candidate.branch);
            }
            b"bare" | b"prunable" => skip = true,
            _ => {}
        }
    }
    if skip {
        return Ok(());
    }
    if candidate.root.is_empty()
        || !Path::new(&candidate.root).is_absolute()
        || !valid_object_id(&candidate.head)
        || candidate.branch.contains(['\0', '\r', '\n'])
        || candidate.branch.len() > 512
    {
        return invalid("malformed Git worktree list");
    }
    let Ok(canonical_root) = canonical_existing_path(
        Path::new(&candidate.root),
        "canonicalize worktree identity failed",
    ) else {
        return Ok(());
    };
    if !std::fs::metadata(&canonical_root).is_ok_and(|metadata| metadata.is_dir()) {
        return Ok(());
    }
    candidate.root = path_string(&canonical_root)?;
    *total += 1;
    if worktrees.len() < MAX_WORKTREES {
        worktrees.push(candidate);
    }
    Ok(())
}

fn valid_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn utf8<'a>(input: &'a [u8], message: &'static str) -> Result<&'a str, RepositoryError> {
    std::str::from_utf8(input).map_err(|_| RepositoryError::InvalidData(message))
}

fn invalid<T>(message: &'static str) -> Result<T, RepositoryError> {
    Err(RepositoryError::InvalidData(message))
}
