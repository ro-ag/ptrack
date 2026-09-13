//! Disposable summaries written by explicit sync. Reading never opens project databases.
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use ptrack_core::{IssueStatus, PlanStatus, ProjectRef, ProjectSnapshot, TaskStatus, Timestamp};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{AppError, AppResult};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverviewCounts {
    pub active_plans: usize,
    pub open_tasks: usize,
    pub done_tasks: usize,
    pub open_issues: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverviewActivity {
    pub kind: String,
    pub id: u64,
    pub title: String,
    pub status: String,
    /// Last record update, never an inferred completion time.
    pub updated_at: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub version: u8,
    pub root: String,
    pub database_id: String,
    pub synced_at: i64,
    pub counts: OverviewCounts,
    pub activity: Vec<OverviewActivity>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalOverviewV1 {
    pub tracked_projects: usize,
    pub summarized_projects: usize,
    pub counts: OverviewCounts,
    pub projects: Vec<ProjectSummary>,
}

/// Outcome of an explicit native refresh; failed projects retain their cache.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshGlobalOverviewV1 {
    pub overview: GlobalOverviewV1,
    pub refreshed_projects: usize,
    pub skipped_projects: usize,
}

fn cache_path(home: &Path, root: &str) -> PathBuf {
    use std::fmt::Write;
    let digest = ptrack_store::sha256_digest(root.as_bytes());
    let name = digest
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            write!(text, "{byte:02x}").expect("writing to a string cannot fail");
            text
        });
    home.join("overview").join(format!("{name}.json"))
}

fn seconds(timestamp: Timestamp) -> Option<i64> {
    match timestamp {
        Timestamp::Fixed { seconds, .. } => Some(seconds),
        Timestamp::Zero => None,
    }
}

fn summary(root: &Path, database_id: &str, snapshot: &ProjectSnapshot) -> ProjectSummary {
    let mut activity = Vec::new();
    for task in &snapshot.tasks {
        if let Some(updated_at) = seconds(task.updated_at) {
            activity.push(OverviewActivity {
                kind: "task".into(),
                id: task.id,
                title: task.title.chars().take(512).collect(),
                status: task.status.as_str().into(),
                updated_at,
            });
        }
    }
    for issue in &snapshot.issues {
        if let Some(updated_at) = seconds(issue.updated_at) {
            activity.push(OverviewActivity {
                kind: "issue".into(),
                id: issue.id,
                title: issue.title.chars().take(512).collect(),
                status: issue.status.as_str().into(),
                updated_at,
            });
        }
    }
    for milestone in &snapshot.milestones {
        if let Some(updated_at) = seconds(milestone.updated_at) {
            activity.push(OverviewActivity {
                kind: "milestone".into(),
                id: milestone.id,
                title: milestone.title.chars().take(512).collect(),
                status: milestone.status.as_str().into(),
                updated_at,
            });
        }
    }
    activity.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then(a.kind.cmp(&b.kind))
            .then(a.id.cmp(&b.id))
    });
    activity.truncate(50);
    ProjectSummary {
        version: 1,
        root: root.to_string_lossy().into_owned(),
        database_id: database_id.to_owned(),
        synced_at: OffsetDateTime::now_utc().unix_timestamp(),
        counts: OverviewCounts {
            active_plans: snapshot
                .plans
                .iter()
                .filter(|p| p.status == PlanStatus::Active)
                .count(),
            open_tasks: snapshot
                .tasks
                .iter()
                .filter(|t| t.status != TaskStatus::Done)
                .count(),
            done_tasks: snapshot
                .tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Done)
                .count(),
            open_issues: snapshot
                .issues
                .iter()
                .filter(|i| i.status == IssueStatus::Open)
                .count(),
        },
        activity,
    }
}

/// Refresh a rebuildable cache without modifying either database.
///
/// # Errors
/// Returns an error when the cache cannot be serialized or atomically replaced.
pub fn write_project_summary(
    home: &Path,
    root: &Path,
    database_id: &str,
    snapshot: &ProjectSnapshot,
) -> AppResult<()> {
    let summary = summary(root, database_id, snapshot);
    let destination = cache_path(home, &summary.root);
    let directory = home.join("overview");
    match fs::create_dir(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(AppError::Io(error)),
    }
    ptrack_store::protect_private_directory(&directory).map_err(cache_error)?;
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).map_err(|e| AppError::Message(e.to_string()))?;
    let temporary = directory.join(format!(".sync-{:032x}.tmp", u128::from_le_bytes(nonce)));
    let bytes = serde_json::to_vec(&summary).map_err(|e| AppError::Message(e.to_string()))?;
    let result = (|| -> AppResult<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        ptrack_store::protect_private_file(&temporary).map_err(cache_error)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        ptrack_store::replace_private_file(&temporary, &destination).map_err(cache_error)?;
        ptrack_store::sync_private_directory(&directory).map_err(cache_error)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Aggregate only currently registered projects; unavailable or invalid caches
/// remain uncovered rather than being reported as zero-work projects.
#[must_use]
pub fn read_global_overview(
    home: &Path,
    registered: &[ProjectRef],
    expected: &[ptrack_store::ActiveGenerationProject],
) -> GlobalOverviewV1 {
    let mut overview = GlobalOverviewV1 {
        tracked_projects: registered.len(),
        ..GlobalOverviewV1::default()
    };
    if ptrack_store::verify_private_path(&home.join("overview"), true).is_err() {
        return overview;
    }
    for project in registered {
        let path = cache_path(home, &project.path);
        let Some(summary) = read_summary(&path).filter(|s| {
            s.version == 1
                && s.root == project.path
                && s.activity.len() <= 50
                && OffsetDateTime::from_unix_timestamp(s.synced_at).is_ok()
                && s.activity
                    .iter()
                    .all(|item| OffsetDateTime::from_unix_timestamp(item.updated_at).is_ok())
                && expected
                    .iter()
                    .any(|binding| binding.root == s.root && binding.database_id == s.database_id)
        }) else {
            continue;
        };
        overview.counts.active_plans = overview
            .counts
            .active_plans
            .saturating_add(summary.counts.active_plans);
        overview.counts.open_tasks = overview
            .counts
            .open_tasks
            .saturating_add(summary.counts.open_tasks);
        overview.counts.done_tasks = overview
            .counts
            .done_tasks
            .saturating_add(summary.counts.done_tasks);
        overview.counts.open_issues = overview
            .counts
            .open_issues
            .saturating_add(summary.counts.open_issues);
        overview.projects.push(summary);
    }
    overview.summarized_projects = overview.projects.len();
    overview
}

fn read_summary(path: &Path) -> Option<ProjectSummary> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return None;
    }
    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags, open};
        fs::File::from(
            open(
                path,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .ok()?,
        )
    };
    #[cfg(not(unix))]
    let file = ptrack_store::open_private_path(path, false, false).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    ptrack_store::verify_private_open_handle(&file).ok()?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 1024 * 1024 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn cache_error(error: impl std::fmt::Display) -> AppError {
    AppError::Message(error.to_string())
}
