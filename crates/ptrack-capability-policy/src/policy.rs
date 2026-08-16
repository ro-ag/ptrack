use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use ptrack_core::{Capability, CapabilityKind, Digest32, GitScope, Timestamp};

use crate::normalize::{
    normalize_endpoint, normalize_git_remote, normalize_http_url, normalize_project_path,
    path_within,
};
use crate::{CapabilityError, normalize};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Denied {
    reason: String,
}

impl Denied {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for Denied {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "capability denied: {}", self.reason)
    }
}

impl std::error::Error for Denied {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitAuthorization {
    pub operation: String,
    pub remote_name: String,
    pub remote_url: String,
    pub branch: String,
    pub refspec: String,
    pub force: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SshOperation {
    Git,
    RemoteCommand,
    Upload,
    Download,
    InteractiveShell,
    LocalForward,
    RemoteForward,
}

impl SshOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::RemoteCommand => "remote-command",
            Self::Upload => "upload",
            Self::Download => "download",
            Self::InteractiveShell => "interactive-shell",
            Self::LocalForward => "local-forward",
            Self::RemoteForward => "remote-forward",
        }
    }
}

/// Revalidates the full common approval envelope immediately before use.
///
/// # Errors
/// Returns a stable deny reason when the stored grant is invalid or inactive.
pub fn authorize(
    capability: &Capability,
    agent_profile: &str,
    now: Timestamp,
) -> Result<Capability, Denied> {
    let preview = normalize(capability).map_err(|_| Denied::new("stored capability is invalid"))?;
    let normalized = preview.capability;
    if capability.scope_digest.is_empty() || capability.scope_digest != preview.scope_digest {
        return Err(Denied::new("approval scope is stale"));
    }
    if !normalized.enabled {
        return Err(Denied::new("capability is disabled"));
    }
    if normalized.agent_profile != agent_profile {
        return Err(Denied::new("agent profile does not match"));
    }
    let Some(approved) = normalized.approved_at.unix_nanoseconds() else {
        return Err(Denied::new("capability has not been approved"));
    };
    let Some(expires) = normalized.expires_at.unix_nanoseconds() else {
        return Err(Denied::new("capability has not been approved"));
    };
    let Some(now) = now.unix_nanoseconds() else {
        return Err(Denied::new("capability approval has expired"));
    };
    if expires <= now {
        return Err(Denied::new("capability approval has expired"));
    }
    let maximum = approved + i128::from(normalized.approval_duration_seconds) * 1_000_000_000;
    if expires > maximum {
        return Err(Denied::new("approval expiry exceeds its duration"));
    }
    Ok(normalized)
}

/// Enables a normalized draft only for the exact confirmed digest.
///
/// # Errors
/// Returns an error when normalization fails, the digest changed, or time overflows.
pub fn approve(
    capability: &Capability,
    expected_digest: Digest32,
    now: Timestamp,
) -> Result<Capability, CapabilityError> {
    let preview = normalize(capability)?;
    if expected_digest.is_empty() || expected_digest != preview.scope_digest {
        return Err(CapabilityError::message(
            "effective scope changed; preview again before enabling",
        ));
    }
    let mut approved = preview.capability;
    approved.enabled = true;
    approved.approved_at = now;
    approved.expires_at = add_seconds(now, approved.approval_duration_seconds)?;
    Ok(approved)
}

#[must_use]
pub fn disable(capability: &Capability) -> Capability {
    let mut disabled = capability.clone();
    disabled.enabled = false;
    disabled.approved_at = Timestamp::Zero;
    disabled.expires_at = Timestamp::Zero;
    disabled
}

/// Authorizes one exact HTTP request without retaining its query data.
///
/// # Errors
/// Returns a stable deny reason when any HTTP dimension is outside the grant.
pub fn authorize_http(
    capability: &Capability,
    agent_profile: &str,
    now: Timestamp,
    method: &str,
    raw_url: &str,
    request_bytes: i64,
) -> Result<(Capability, String), Denied> {
    let normalized = authorize(capability, agent_profile, now)?;
    if normalized.kind != CapabilityKind::Http {
        return Err(Denied::new("capability is not HTTP"));
    }
    let method = method.trim().to_uppercase();
    let Some(scope) = normalized.http.as_ref() else {
        return Err(Denied::new("capability is not HTTP"));
    };
    if !scope.methods.contains(&method) {
        return Err(Denied::new(format!("HTTP method {method} is not approved")));
    }
    if request_bytes < 0 || request_bytes > normalized.limits.max_request_bytes {
        return Err(Denied::new("HTTP request exceeds its byte limit"));
    }
    let request = normalize_http_url(raw_url, true)
        .map_err(|_| Denied::new("HTTP request URL is invalid"))?;
    if !request.fragment.is_empty() {
        return Err(Denied::new("HTTP request URL is invalid"));
    }
    let base = normalize_http_url(&scope.base_url, false)
        .map_err(|_| Denied::new("HTTP request origin is outside the approved scope"))?;
    if request.scheme != base.scheme || request.host != base.host {
        return Err(Denied::new(
            "HTTP request origin is outside the approved scope",
        ));
    }
    if !scope
        .path_prefixes
        .iter()
        .any(|prefix| path_within(prefix, &request.path))
    {
        return Err(Denied::new(
            "HTTP request path is outside the approved scope",
        ));
    }
    Ok((normalized, request.url))
}

/// Authorizes a Git operation against freshly observed remote configuration.
///
/// # Errors
/// Returns a stable deny reason when any Git dimension is outside the grant.
pub fn authorize_git(
    capability: &Capability,
    agent_profile: &str,
    now: Timestamp,
    request: &GitAuthorization,
) -> Result<Capability, Denied> {
    let normalized = authorize(capability, agent_profile, now)?;
    if normalized.kind != CapabilityKind::Git {
        return Err(Denied::new("capability is not Git"));
    }
    let Some(scope) = normalized.git.as_ref() else {
        return Err(Denied::new("capability is not Git"));
    };
    authorize_git_scope(scope, request)?;
    Ok(normalized)
}

pub(crate) fn authorize_git_scope(
    scope: &GitScope,
    request: &GitAuthorization,
) -> Result<(), Denied> {
    let operation = request.operation.trim().to_lowercase();
    if !scope.operations.contains(&operation) {
        return Err(Denied::new(format!("Git {operation} is not approved")));
    }
    let remote = normalize_git_remote(&request.remote_url).ok();
    if request.remote_name != scope.remote_name || remote.as_deref() != Some(&scope.remote_url) {
        return Err(Denied::new(
            "Git remote no longer matches the approved scope",
        ));
    }
    if matches!(operation.as_str(), "fetch" | "pull" | "push")
        && !scope.branches.contains(&request.branch)
    {
        return Err(Denied::new("Git branch is not approved"));
    }
    if !request.refspec.is_empty() && !scope.refspecs.contains(&request.refspec) {
        return Err(Denied::new("Git refspec is not approved"));
    }
    if request.force && !scope.allow_force_push {
        return Err(Denied::new("Git force push is not approved"));
    }
    if request.refspec.starts_with(':') && !scope.allow_delete_refs {
        return Err(Denied::new("Git ref deletion is not approved"));
    }
    if request.refspec.contains("refs/tags/") && !scope.allow_tags {
        return Err(Denied::new("Git tag writes are not approved"));
    }
    Ok(())
}

/// Authorizes one independently granted SSH behavior.
///
/// # Errors
/// Returns a stable deny reason when the SSH behavior or exact value is unapproved.
pub fn authorize_ssh(
    capability: &Capability,
    agent_profile: &str,
    now: Timestamp,
    operation: SshOperation,
    value: &str,
) -> Result<Capability, Denied> {
    let normalized = authorize(capability, agent_profile, now)?;
    if normalized.kind != CapabilityKind::Ssh {
        return Err(Denied::new("capability is not SSH"));
    }
    let Some(scope) = normalized.ssh.as_ref() else {
        return Err(Denied::new("capability is not SSH"));
    };
    let allowed = match operation {
        SshOperation::Git => scope.allow_git,
        SshOperation::RemoteCommand => scope.remote_commands.iter().any(|item| item == value),
        SshOperation::Upload => scope.allow_upload,
        SshOperation::Download => scope.allow_download,
        SshOperation::InteractiveShell => scope.allow_interactive_shell,
        SshOperation::LocalForward => normalize_endpoint(value)
            .is_ok_and(|endpoint| scope.local_forward_targets.contains(&endpoint)),
        SshOperation::RemoteForward => normalize_endpoint(value)
            .is_ok_and(|endpoint| scope.remote_forward_targets.contains(&endpoint)),
    };
    if !allowed {
        return Err(Denied::new(format!(
            "SSH {} is not approved",
            operation.as_str()
        )));
    }
    Ok(normalized)
}

/// Returns policy evidence only. Callers performing transfers must re-open and
/// re-verify filesystem objects with no-follow primitives at the I/O boundary.
///
/// # Errors
/// Returns a stable deny reason when canonicalization or containment fails.
pub fn resolve_project_path(
    project_root: &Path,
    requested: &str,
    approved_roots: &[String],
    must_exist: bool,
) -> Result<PathBuf, Denied> {
    let project = fs::canonicalize(project_root)
        .map_err(|_| Denied::new("project root cannot be canonicalized"))?;
    let relative = normalize_project_path(requested)
        .map_err(|_| Denied::new("path is not project-relative"))?;
    let target = canonicalize_path(&project.join(relative), must_exist)
        .map_err(|_| Denied::new("path escapes the project"))?;
    if !filesystem_within(&project, &target) {
        return Err(Denied::new("path escapes the project"));
    }
    for root in approved_roots {
        let Ok(root) = normalize_project_path(root) else {
            continue;
        };
        let Ok(root) = canonicalize_path(&project.join(root), false) else {
            continue;
        };
        if filesystem_within(&root, &target) {
            return Ok(target);
        }
    }
    Err(Denied::new("path is outside approved roots"))
}

fn add_seconds(timestamp: Timestamp, value: i64) -> Result<Timestamp, CapabilityError> {
    let Timestamp::Fixed {
        seconds,
        nanoseconds,
        offset_seconds,
    } = timestamp
    else {
        return Err(CapabilityError::message("approval time is invalid"));
    };
    Ok(Timestamp::Fixed {
        seconds: seconds
            .checked_add(value)
            .ok_or_else(|| CapabilityError::message("approval expiry is out of range"))?,
        nanoseconds,
        offset_seconds,
    })
}

fn canonicalize_path(value: &Path, must_exist: bool) -> std::io::Result<PathBuf> {
    match fs::canonicalize(value) {
        Ok(path) => return Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if must_exist {
        return fs::canonicalize(value);
    }
    let mut cursor = value;
    let mut missing = Vec::new();
    loop {
        let name = cursor
            .file_name()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
        missing.push(name.to_owned());
        cursor = cursor
            .parent()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
        match fs::canonicalize(cursor) {
            Ok(mut existing) => {
                // The missing suffix can only be re-joined onto a directory.
                // On Windows, canonicalizing a path whose ancestor is a FILE
                // fails with ERROR_PATH_NOT_FOUND, which std maps to
                // `NotFound`, so the loop lands on the file itself here; treat
                // that as an escape. On Unix this check is inert: the loop is
                // only reached when every missing ancestor truly does not
                // exist, so the first existing ancestor is a directory, or
                // canonicalize already failed with ENOTDIR (not `NotFound`).
                // Symlinks are already resolved by `fs::canonicalize`.
                if !fs::metadata(&existing).is_ok_and(|metadata| metadata.is_dir()) {
                    return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
                }
                for component in missing.iter().rev() {
                    existing.push(component);
                }
                return Ok(existing);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
}

fn filesystem_within(parent: &Path, child: &Path) -> bool {
    child == parent || child.strip_prefix(parent).is_ok()
}
