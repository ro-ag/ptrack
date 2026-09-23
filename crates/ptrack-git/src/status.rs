use std::collections::BTreeSet;
use std::path::{Component, Path};

use crate::model::{PathBounds, Status};
use crate::runner::RepositoryError;

const MAX_STATUS_PATHS: usize = 500;

/// A parsed porcelain v2 status plus facts the snapshot needs but the wire
/// DTO does not carry.
pub(crate) struct ParsedStatus {
    pub(crate) status: Status,
    /// Git printed a `branch.ab` header, which it does only when the
    /// configured upstream ref exists. An upstream that was pruned, or that
    /// was never fetched (a fresh clone of an empty repository), has no
    /// divergence to compute.
    pub(crate) upstream_resolved: bool,
}

#[cfg(test)]
pub(crate) fn parse_porcelain_v2_status(input: &[u8]) -> Result<Status, RepositoryError> {
    parse_status_records(input).map(|parsed| parsed.status)
}

/// Parses `git status --porcelain=v2 --branch -z` output.
///
/// A path that is not UTF-8 or carries control characters is counted in the
/// totals but left out of the path lists (it shows as one of the "more"
/// paths), so one unusual file cannot blank the whole snapshot.
pub(crate) fn parse_status_records(input: &[u8]) -> Result<ParsedStatus, RepositoryError> {
    let records: Vec<&[u8]> = input.split(|byte| *byte == 0).collect();
    let mut status = Status::default();
    let mut upstream_resolved = false;
    let mut changed = PathSet::default();
    let mut untracked = PathSet::default();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        if record.is_empty() {
            index += 1;
            continue;
        }
        if let Some(header) = record.strip_prefix(b"# ") {
            let header = String::from_utf8_lossy(header);
            upstream_resolved |= header.starts_with("branch.ab ");
            parse_status_header(&mut status, &header)?;
        } else if record.starts_with(b"1 ") {
            let fields = splitn_spaces(record, 9);
            if fields.len() != 9 || fields[1].len() != 2 {
                return invalid("malformed ordinary status record");
            }
            count_xy(&mut status, fields[1]);
            changed.insert(fields[8])?;
        } else if record.starts_with(b"2 ") {
            let fields = splitn_spaces(record, 10);
            if fields.len() != 10
                || fields[1].len() != 2
                || index + 1 >= records.len()
                || records[index + 1].is_empty()
            {
                return invalid("malformed renamed status record");
            }
            count_xy(&mut status, fields[1]);
            changed.insert(fields[9])?;
            changed.insert(records[index + 1])?;
            index += 1;
        } else if record.starts_with(b"u ") {
            let fields = splitn_spaces(record, 11);
            if fields.len() != 11 || fields[1].len() != 2 {
                return invalid("malformed unmerged status record");
            }
            status.conflicted += 1;
            changed.insert(fields[10])?;
        } else if let Some(path) = record.strip_prefix(b"? ") {
            if path.is_empty() {
                return invalid("malformed untracked status record");
            }
            status.untracked += 1;
            untracked.insert(path)?;
        } else if let Some(path) = record.strip_prefix(b"! ") {
            if path.is_empty() {
                return invalid("malformed ignored status record");
            }
            status.ignored += 1;
        } else {
            return invalid("unknown porcelain v2 record");
        }
        index += 1;
    }
    let (changed_paths, changed_bounds) = changed.bounded();
    status.changed_paths = Some(changed_paths);
    status.changed_path_bounds = changed_bounds;
    let (untracked_paths, untracked_bounds) = untracked.bounded();
    status.untracked_paths = Some(untracked_paths);
    status.untracked_path_bounds = untracked_bounds;
    Ok(ParsedStatus {
        status,
        upstream_resolved,
    })
}

/// Distinct displayable paths plus a count of paths that cannot be shown.
#[derive(Default)]
struct PathSet {
    paths: BTreeSet<String>,
    unrepresentable: BTreeSet<Vec<u8>>,
}

impl PathSet {
    fn insert(&mut self, raw: &[u8]) -> Result<(), RepositoryError> {
        match std::str::from_utf8(raw) {
            Ok(value) if !value.chars().any(char::is_control) => {
                self.paths.insert(status_path(value)?);
            }
            _ => {
                if raw.is_empty() {
                    return invalid("invalid repository-relative status path");
                }
                self.unrepresentable.insert(raw.to_vec());
            }
        }
        Ok(())
    }

    fn bounded(self) -> (Vec<String>, PathBounds) {
        let total = self.paths.len() + self.unrepresentable.len();
        let items: Vec<String> = self.paths.into_iter().take(MAX_STATUS_PATHS).collect();
        let shown = items.len();
        (
            items,
            PathBounds {
                shown,
                total,
                more: total - shown,
            },
        )
    }
}

fn splitn_spaces(input: &[u8], count: usize) -> Vec<&[u8]> {
    input.splitn(count, |byte| *byte == b' ').collect()
}

fn status_path(value: &str) -> Result<String, RepositoryError> {
    if value.is_empty() || Path::new(value).is_absolute() || has_platform_volume(value) {
        return invalid("invalid repository-relative status path");
    }

    let mut clean = Vec::new();
    for component in Path::new(value).components() {
        match component {
            Component::Normal(part) => clean.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir if clean.pop().is_some() => {}
            Component::ParentDir => return invalid("status path escapes repository root"),
            Component::RootDir | Component::Prefix(_) => {
                return invalid("invalid repository-relative status path");
            }
        }
    }
    if clean.is_empty() {
        return invalid("invalid repository-relative status path");
    }
    Ok(clean.join("/"))
}

#[cfg(windows)]
fn has_platform_volume(value: &str) -> bool {
    let bytes = value.as_bytes();
    (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || value.starts_with("\\\\")
}

#[cfg(not(windows))]
fn has_platform_volume(_value: &str) -> bool {
    false
}

fn parse_status_header(status: &mut Status, header: &str) -> Result<(), RepositoryError> {
    let Some((key, value)) = header.split_once(' ') else {
        return invalid("malformed status header");
    };
    match key {
        "branch.oid" => value.clone_into(&mut status.oid),
        "branch.head" => {
            value.clone_into(&mut status.branch);
            status.detached = value == "(detached)";
            status.initial = value == "(initial)";
        }
        "branch.upstream" => value.clone_into(&mut status.upstream),
        "branch.ab" => {
            let fields: Vec<&str> = value.split_whitespace().collect();
            if fields.len() != 2 || !fields[0].starts_with('+') || !fields[1].starts_with('-') {
                return invalid("malformed branch divergence header");
            }
            status.ahead = fields[0][1..]
                .parse()
                .map_err(|_| RepositoryError::InvalidData("parse ahead count"))?;
            status.behind = fields[1][1..]
                .parse()
                .map_err(|_| RepositoryError::InvalidData("parse behind count"))?;
        }
        _ => {}
    }
    Ok(())
}

fn count_xy(status: &mut Status, xy: &[u8]) {
    if xy[0] != b'.' {
        status.staged += 1;
    }
    if xy[1] != b'.' {
        status.unstaged += 1;
    }
}

fn invalid<T>(message: &'static str) -> Result<T, RepositoryError> {
    Err(RepositoryError::InvalidData(message))
}
