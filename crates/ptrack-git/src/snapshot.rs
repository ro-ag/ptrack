use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::model::{Branch, ChangedArea, Commit, Divergence, Remote, RepositoryState, Snapshot};
use crate::runner::{CancellationToken, ExecRunner, RepositoryError, Runner, args, os};
use crate::status::parse_status_records;
use crate::worktree::parse_worktree_list;

const MAX_GIT_COMMANDS: usize = 10;
const MAX_AGGREGATE_GIT_BYTES: usize = 12 * 1024 * 1024;
const MAX_REMOTES: usize = 16;
const MAX_LOCAL_BRANCHES: usize = 100;
const MAX_REMOTE_BRANCHES: usize = 150;
const MAX_RECENT_COMMITS: usize = 40;
const MAX_UNPUSHED_COMMITS: usize = 40;
const MAX_CHANGED_PATHS: usize = 500;
const STALE_BRANCH_AGE_SECONDS: i64 = 90 * 24 * 60 * 60;
const STALE_BRANCH_POLICY: &str =
    "non-current local branch tip older than 90 days; not proof that deletion is safe";
const REF_FORMAT: &str = concat!(
    "--format=%(refname)%00%(objectname)%00%(upstream:short)%00",
    "%(committerdate:unix)%00%(HEAD)%00%(worktreepath)%00"
);
/// Every field is NUL-terminated and starts with a NUL. Commit metadata cannot
/// contain NUL, so records split unambiguously even when a subject or author
/// carries the 0x1e/0x1f bytes an older record format used as separators.
const LOG_FORMAT: &str = "--format=%x00%H%x00%an%x00%ae%x00%at%x00%s%x00%D%x00";
const LOG_FIELDS: usize = 6;

#[derive(Clone)]
pub struct RepositoryService {
    runner: Arc<dyn Runner>,
    now: fn() -> i64,
}

impl Default for RepositoryService {
    fn default() -> Self {
        Self::new()
    }
}

impl RepositoryService {
    #[must_use]
    pub fn new() -> Self {
        Self {
            runner: Arc::new(ExecRunner::default()),
            now: unix_now,
        }
    }

    /// Captures bounded read-only repository intelligence.
    ///
    /// # Errors
    ///
    /// Returns a content-free error when cancellation, a resource bound,
    /// subprocess execution, filesystem inspection, or parsing fails.
    #[allow(clippy::too_many_lines)]
    pub fn capture(
        &self,
        cancellation: &CancellationToken,
        root: impl AsRef<Path>,
    ) -> Result<Snapshot, RepositoryError> {
        let root = root.as_ref();
        let mut session = ExecutionSession::new(self.runner.as_ref(), cancellation);
        let identity = match session.run(
            root,
            &args([
                "rev-parse",
                "--path-format=absolute",
                "--is-inside-work-tree",
                "--show-toplevel",
                "--absolute-git-dir",
                "--git-common-dir",
                "--is-bare-repository",
            ]),
        ) {
            Ok(output) => output,
            Err(RepositoryError::CommandFailed) => {
                if has_git_worktree_marker(root)? {
                    return Err(RepositoryError::CommandFailed);
                }
                return Ok(Snapshot {
                    state: RepositoryState::NotRepository,
                    ..Snapshot::default()
                });
            }
            Err(error) => return Err(error),
        };
        let mut snapshot = parse_repository_identity(&identity)?;

        match session.run(root, &args(["worktree", "list", "--porcelain", "-z"])) {
            Ok(output) => {
                let (worktrees, bounds) = parse_worktree_list(&output)?;
                snapshot.worktrees = Some(worktrees);
                snapshot.worktrees_incomplete = bounds.more > 0;
                snapshot.worktree_bounds = bounds;
            }
            Err(RepositoryError::CommandFailed) => snapshot.worktrees_incomplete = true,
            Err(error) => return Err(error),
        }

        // Ignored paths are counted by a separate, non-fatal query at the end:
        // a large ignored tree must not push the essential status read over
        // its output limit.
        let status_output =
            session.run(root, &args(["status", "--porcelain=v2", "--branch", "-z"]))?;
        let parsed_status = parse_status_records(&status_output)?;
        snapshot.status = parsed_status.status;

        match session.run(
            root,
            &args([
                "config",
                "--null",
                "--get-regexp",
                r"^remote\..*\.(url|pushurl)$",
            ]),
        ) {
            Ok(output) => snapshot.remotes = Some(parse_remotes(&output)),
            Err(RepositoryError::CommandFailed) => {}
            Err(error) => return Err(error),
        }

        let local_output = session.run(
            root,
            &[
                os("for-each-ref"),
                os(format!("--count={MAX_LOCAL_BRANCHES}")),
                os("--sort=-committerdate"),
                os(REF_FORMAT),
                os("refs/heads"),
            ],
        )?;
        let (local, unexpected_remotes) = parse_refs(&local_output, (self.now)())?;
        if !unexpected_remotes.is_empty() {
            return invalid("local Git ref query returned remote refs");
        }

        let remote_output = session.run(
            root,
            &[
                os("for-each-ref"),
                os(format!("--count={MAX_REMOTE_BRANCHES}")),
                os("--sort=-committerdate"),
                os(REF_FORMAT),
                os("refs/remotes"),
            ],
        )?;
        let (unexpected_locals, remote) = parse_refs(&remote_output, (self.now)())?;
        if !unexpected_locals.is_empty() {
            return invalid("remote Git ref query returned local refs");
        }
        snapshot.local_branches = Some(local);
        snapshot.remote_branches = Some(remote);

        match session.run(root, &log_args(MAX_RECENT_COMMITS + 1, None)) {
            Ok(output) => {
                let mut commits = parse_log(&output, MAX_RECENT_COMMITS + 1);
                if commits.len() > MAX_RECENT_COMMITS {
                    commits.truncate(MAX_RECENT_COMMITS);
                    snapshot.recent_commits_truncated = true;
                }
                snapshot.recent_commits = Some(commits);
            }
            Err(RepositoryError::CommandFailed) => {}
            Err(error) => return Err(error),
        }

        if parsed_status.upstream_resolved && !snapshot.status.upstream.is_empty() {
            capture_divergence(&mut session, root, &mut snapshot)?;
        }
        snapshot.status.ignored = count_ignored(&mut session, root)?;
        STALE_BRANCH_POLICY.clone_into(&mut snapshot.stale_branch_policy);
        Ok(snapshot)
    }

    /// Constructs a service sized for tracked-path scans.
    ///
    /// A repository's full `ls-files` listing dwarfs any snapshot command, so
    /// the snapshot's 4 MiB, 3-second bounds would reject or time out on a
    /// large repository long before the 200,000-path cap applied.
    #[must_use]
    pub fn for_stack_scan() -> Self {
        Self {
            runner: Arc::new(ExecRunner::with_limits(
                Duration::from_secs(30),
                32 * 1024 * 1024,
            )),
            now: unix_now,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_runner_and_clock(runner: Arc<dyn Runner>, now: fn() -> i64) -> Self {
        Self { runner, now }
    }

    pub(crate) fn runner(&self) -> &dyn Runner {
        self.runner.as_ref()
    }
}

/// Captures bounded read-only repository intelligence with default bounds.
///
/// # Errors
///
/// Returns a content-free error when cancellation, a resource bound,
/// subprocess execution, filesystem inspection, or parsing fails.
pub fn capture(
    cancellation: &CancellationToken,
    root: impl AsRef<Path>,
) -> Result<Snapshot, RepositoryError> {
    RepositoryService::new().capture(cancellation, root)
}

pub(crate) struct ExecutionSession<'a> {
    runner: &'a dyn Runner,
    cancellation: &'a CancellationToken,
    commands: usize,
    bytes: usize,
}

impl<'a> ExecutionSession<'a> {
    pub(crate) fn new(runner: &'a dyn Runner, cancellation: &'a CancellationToken) -> Self {
        Self {
            runner,
            cancellation,
            commands: 0,
            bytes: 0,
        }
    }

    pub(crate) fn run(
        &mut self,
        root: &Path,
        args: &[OsString],
    ) -> Result<Vec<u8>, RepositoryError> {
        self.commands += 1;
        if self.commands > MAX_GIT_COMMANDS {
            return Err(RepositoryError::AggregateLimit);
        }
        let output = self.runner.output(self.cancellation, root, args)?;
        self.bytes = self
            .bytes
            .checked_add(output.len())
            .ok_or(RepositoryError::AggregateLimit)?;
        if self.bytes > MAX_AGGREGATE_GIT_BYTES {
            return Err(RepositoryError::AggregateLimit);
        }
        Ok(output)
    }
}

/// Reads divergence and unpushed commits against a resolved upstream. A
/// failed range query (the upstream vanished between the status read and
/// this one) leaves both absent rather than failing the snapshot.
fn capture_divergence(
    session: &mut ExecutionSession<'_>,
    root: &Path,
    snapshot: &mut Snapshot,
) -> Result<(), RepositoryError> {
    let range = format!("{}...HEAD", snapshot.status.upstream);
    let divergence_output = match session.run(
        root,
        &[
            os("rev-list"),
            os("--left-right"),
            os("--count"),
            os("--end-of-options"),
            os(range),
        ],
    ) {
        Ok(output) => output,
        Err(RepositoryError::CommandFailed) => return Ok(()),
        Err(error) => return Err(error),
    };
    snapshot.divergence = Some(parse_divergence(
        &snapshot.status.upstream,
        &divergence_output,
    )?);

    let unpushed_range = format!("{}..HEAD", snapshot.status.upstream);
    match session.run(
        root,
        &log_args(MAX_UNPUSHED_COMMITS + 1, Some(&unpushed_range)),
    ) {
        Ok(output) => {
            let mut commits = parse_log(&output, MAX_UNPUSHED_COMMITS + 1);
            if commits.len() > MAX_UNPUSHED_COMMITS {
                commits.truncate(MAX_UNPUSHED_COMMITS);
                snapshot.unpushed_commits_truncated = true;
            }
            snapshot.unpushed_commits = Some(commits);
        }
        Err(RepositoryError::CommandFailed) => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

/// Counts ignored untracked entries, collapsing a wholly ignored directory
/// into one entry. The count is informational: any bound or failure other
/// than cancellation reports zero instead of failing the snapshot.
fn count_ignored(
    session: &mut ExecutionSession<'_>,
    root: &Path,
) -> Result<usize, RepositoryError> {
    match session.run(
        root,
        &args([
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
        ]),
    ) {
        Ok(output) => Ok(output
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
            .count()),
        Err(RepositoryError::Cancelled) => Err(RepositoryError::Cancelled),
        Err(_) => Ok(0),
    }
}

fn log_args(limit: usize, range: Option<&str>) -> Vec<OsString> {
    let mut result = vec![
        os("log"),
        os("-n"),
        os(limit.to_string()),
        os("--date=unix"),
        os(LOG_FORMAT),
        os("--name-only"),
    ];
    if let Some(range) = range {
        result.push(os("--end-of-options"));
        result.push(os(range));
    }
    result
}

fn has_git_worktree_marker(root: &Path) -> Result<bool, RepositoryError> {
    let mut current = absolute_lexical(root)?;
    loop {
        match std::fs::symlink_metadata(current.join(".git")) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(RepositoryError::Filesystem(
                    "inspect repository marker failed",
                ));
            }
        }
        if !current.pop() {
            return Ok(false);
        }
    }
}

fn absolute_lexical(path: &Path) -> Result<PathBuf, RepositoryError> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|_| RepositoryError::Filesystem("resolve absolute repository path failed"))?
            .join(path)
    };
    Ok(absolute.components().collect())
}

fn parse_repository_identity(output: &[u8]) -> Result<Snapshot, RepositoryError> {
    let text = utf8(output, "malformed git repository identity")?;
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() != 5 {
        return invalid("malformed git repository identity");
    }
    if !lines[0]
        .parse::<bool>()
        .map_err(|_| RepositoryError::InvalidData("git root is not a worktree"))?
    {
        return invalid("git root is not a worktree");
    }
    let bare = lines[4]
        .parse::<bool>()
        .map_err(|_| RepositoryError::InvalidData("parse bare repository state"))?;
    let git_dir = clean_path(lines[2]);
    let common_git_dir = clean_path(lines[3]);
    Ok(Snapshot {
        state: RepositoryState::Ready,
        root: clean_path(lines[1]),
        linked_worktree: git_dir != common_git_dir,
        git_dir,
        common_git_dir,
        bare,
        ..Snapshot::default()
    })
}

fn clean_path(value: &str) -> String {
    Path::new(value)
        .components()
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
}

/// Parses `config --null --get-regexp` remote URL entries. Entries that do
/// not have the expected shape are skipped, URLs are decoded lossily, and any
/// `user:password@` or token userinfo is removed before a URL reaches the UI.
fn parse_remotes(output: &[u8]) -> Vec<Remote> {
    #[derive(Default)]
    struct URLs {
        fetch: Vec<String>,
        push: Vec<String>,
    }

    let mut by_name: BTreeMap<String, URLs> = BTreeMap::new();
    for raw in output
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
    {
        let value = String::from_utf8_lossy(raw);
        let Some((key, url)) = value.split_once('\n') else {
            continue;
        };
        let Some(remainder) = key.strip_prefix("remote.") else {
            continue;
        };
        let (name, push) = if let Some(name) = remainder.strip_suffix(".pushurl") {
            (name, true)
        } else if let Some(name) = remainder.strip_suffix(".url") {
            (name, false)
        } else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let entry = by_name.entry(name.to_owned()).or_default();
        let url = redact_url_userinfo(url);
        if push {
            entry.push.push(url);
        } else {
            entry.fetch.push(url);
        }
    }

    by_name
        .into_iter()
        .take(MAX_REMOTES)
        .map(|(name, urls)| Remote {
            name,
            push_urls: if urls.push.is_empty() {
                urls.fetch.clone()
            } else {
                urls.push
            },
            fetch_urls: (!urls.fetch.is_empty()).then_some(urls.fetch),
        })
        .collect()
}

/// Removes credentials from a `scheme://userinfo@host/...` URL.
///
/// An SSH user name without a password (`ssh://git@host/...`) is kept, since
/// it names an account rather than carrying a secret. Every other userinfo is
/// dropped, including a bare token used as the user name
/// (`https://<token>@host/...`). scp-like `user@host:path` remotes carry no
/// secret and are returned unchanged.
pub(crate) fn redact_url_userinfo(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let Some(at) = authority.rfind('@') else {
        return url.to_owned();
    };
    let (userinfo, host) = (&authority[..at], &authority[at + 1..]);
    let ssh = matches!(
        scheme.to_ascii_lowercase().as_str(),
        "ssh" | "git+ssh" | "ssh+git"
    );
    if ssh && !userinfo.contains(':') {
        return url.to_owned();
    }
    format!("{scheme}://{host}{tail}")
}

fn parse_refs(output: &[u8], now: i64) -> Result<(Vec<Branch>, Vec<Branch>), RepositoryError> {
    let fields: Vec<&[u8]> = output.split(|byte| *byte == 0).collect();
    let mut local = Vec::new();
    let mut remote = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let raw_reference = trim_leading_newlines(fields[index]);
        if raw_reference.is_empty() {
            index += 1;
            continue;
        }
        if index + 5 >= fields.len() {
            return invalid("malformed Git ref output");
        }
        let record = &fields[index..index + 6];
        index += 6;
        // A ref name that is not UTF-8 (a collaborator's branch fetched into
        // refs/remotes) or a tip without a commit date (a ref pointing at a
        // non-commit) is skipped rather than failing the whole snapshot.
        let (Ok(reference), Ok(oid)) = (
            std::str::from_utf8(raw_reference),
            std::str::from_utf8(record[1]),
        ) else {
            continue;
        };
        let Some(epoch) = std::str::from_utf8(record[3])
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
        else {
            continue;
        };
        let mut branch = Branch {
            reference: reference.to_owned(),
            oid: oid.to_owned(),
            upstream: String::from_utf8_lossy(record[2]).into_owned(),
            last_commit_at: format_unix_utc(epoch),
            current: record[4] == b"*",
            worktree_path: String::from_utf8_lossy(record[5]).into_owned(),
            ..Branch::default()
        };
        if let Some(name) = reference.strip_prefix("refs/heads/") {
            name.clone_into(&mut branch.name);
            branch.stale = !branch.current
                && i128::from(now) - i128::from(epoch) > i128::from(STALE_BRANCH_AGE_SECONDS);
            if local.len() < MAX_LOCAL_BRANCHES {
                local.push(branch);
            }
        } else if let Some(name) = reference.strip_prefix("refs/remotes/") {
            name.clone_into(&mut branch.name);
            branch.remote = true;
            if !branch.name.ends_with("/HEAD") && remote.len() < MAX_REMOTE_BRANCHES {
                remote.push(branch);
            }
        } else {
            return invalid("unexpected Git ref namespace");
        }
    }
    Ok((local, remote))
}

fn trim_leading_newlines(mut input: &[u8]) -> &[u8] {
    while let [b'\n', rest @ ..] = input {
        input = rest;
    }
    input
}

/// Parses [`LOG_FORMAT`] records with `--name-only` paths.
///
/// Splitting on NUL yields a leading empty token, then per commit the six
/// header fields followed by one token holding that commit's path list.
/// Text fields are decoded lossily, and a record whose hash or date is
/// malformed is skipped, so one unusual commit cannot fail the snapshot.
fn parse_log(output: &[u8], limit: usize) -> Vec<Commit> {
    let tokens: Vec<&[u8]> = output.split(|byte| *byte == 0).collect();
    let mut commits = Vec::with_capacity(limit.min(tokens.len() / (LOG_FIELDS + 1)));
    for record in tokens.get(1..).unwrap_or_default().chunks(LOG_FIELDS + 1) {
        if commits.len() >= limit {
            break;
        }
        if record.len() < LOG_FIELDS {
            break;
        }
        let Some(commit) = parse_log_record(record) else {
            continue;
        };
        commits.push(commit);
    }
    commits
}

fn parse_log_record(record: &[&[u8]]) -> Option<Commit> {
    let sha = std::str::from_utf8(record[0]).ok()?.trim();
    if sha.is_empty() || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let epoch: i64 = std::str::from_utf8(record[3]).ok()?.trim().parse().ok()?;
    let refs = String::from_utf8_lossy(record[5])
        .split(',')
        .map(str::trim)
        .filter(|reference| !reference.is_empty())
        .map(str::to_owned)
        .collect();
    let path_lines: Vec<&[u8]> = record
        .get(LOG_FIELDS)
        .map(|paths| trim_ascii(paths))
        .unwrap_or_default()
        .split(|byte| *byte == b'\n')
        .filter(|path| !path.is_empty())
        .take(MAX_CHANGED_PATHS)
        .collect();
    Some(Commit {
        sha: sha.to_owned(),
        author_name: String::from_utf8_lossy(record[1]).into_owned(),
        author_email: String::from_utf8_lossy(record[2]).into_owned(),
        date: format_unix_utc(epoch),
        subject: String::from_utf8_lossy(record[4]).into_owned(),
        refs,
        files_changed: path_lines.len(),
        changed_areas: changed_areas(&path_lines),
    })
}

fn changed_areas(paths: &[&[u8]]) -> Vec<ChangedArea> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for raw in paths {
        let path = String::from_utf8_lossy(raw);
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        let name = path
            .replace('\\', "/")
            .split_once('/')
            .map_or("(root)".to_owned(), |(area, _)| area.to_owned());
        *counts.entry(name).or_default() += 1;
    }
    let mut result: Vec<ChangedArea> = counts
        .into_iter()
        .map(|(name, files)| ChangedArea { name, files })
        .collect();
    result.sort_by(|left, right| {
        right
            .files
            .cmp(&left.files)
            .then_with(|| left.name.cmp(&right.name))
    });
    result.truncate(6);
    result
}

fn parse_divergence(upstream: &str, output: &[u8]) -> Result<Divergence, RepositoryError> {
    let fields: Vec<&str> = utf8(output, "malformed Git divergence output")?
        .split_whitespace()
        .collect();
    if fields.len() != 2 {
        return invalid("malformed Git divergence output");
    }
    let behind = fields[0]
        .parse()
        .map_err(|_| RepositoryError::InvalidData("parse behind count"))?;
    let ahead = fields[1]
        .parse()
        .map_err(|_| RepositoryError::InvalidData("parse ahead count"))?;
    Ok(Divergence {
        upstream: upstream.to_owned(),
        ahead,
        behind,
    })
}

fn format_unix_utc(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let seconds = epoch.rem_euclid(86_400);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn unix_now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        Err(error) => -i64::try_from(error.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

fn trim_ascii(mut input: &[u8]) -> &[u8] {
    while input.first().is_some_and(u8::is_ascii_whitespace) {
        input = &input[1..];
    }
    while input.last().is_some_and(u8::is_ascii_whitespace) {
        input = &input[..input.len() - 1];
    }
    input
}

fn utf8<'a>(input: &'a [u8], message: &'static str) -> Result<&'a str, RepositoryError> {
    std::str::from_utf8(input).map_err(|_| RepositoryError::InvalidData(message))
}

fn invalid<T>(message: &'static str) -> Result<T, RepositoryError> {
    Err(RepositoryError::InvalidData(message))
}
