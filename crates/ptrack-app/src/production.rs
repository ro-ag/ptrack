//! Production authority: the attested process runtime and everything the CLI
//! and the desktop build on it.
//!
//! - `runtime` is [`ActiveRuntime`], the retained active-generation marker.
//! - `routed` is [`RoutedApplication`], the CLI's lazily bound application
//!   with bootstrap and relocation.
//! - `recent` is [`ProductionRecentProjects`], the recent-projects registry.
//! - `factory` builds production desktop workspaces.
//! - `authority` is [`ProductionDesktopAuthority`], the replaceable desktop
//!   authority, with first-run initialization as its state machine.
//! - `startup` decides what a desktop launch opens.
//! - `guide` and `pinned_guide` preview, validate, and publish project guides.
//! - `bootstrap` and `journal` own the durable bootstrap plan and the desktop
//!   initialization journal.

mod authority;
mod bootstrap;
mod factory;
mod guide;
mod journal;
#[cfg(unix)]
mod pinned_guide;
mod recent;
mod routed;
mod runtime;
mod startup;
#[cfg(test)]
mod test_support;

use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ptrack_store::sha256_digest;

use crate::{AppError, AppResult};

pub use authority::{
    ProductionDesktopAuthority, production_desktop_runtime, production_desktop_runtime_for_startup,
};
pub use factory::ProductionDesktopWorkspaceFactory;
pub(crate) use guide::install_project_guide_pinned;
#[cfg(test)]
pub(crate) use journal::{stale_guide_skip_allowed, validate_desktop_initialization_transition};
pub use recent::ProductionRecentProjects;
pub use routed::RoutedApplication;
pub use runtime::{ActiveRuntime, RuntimeBindingState, resolve_global_home};
pub use startup::{StartupProjectV1, resolved_startup_project, startup_project};
#[cfg(test)]
pub(crate) use test_support::{
    set_guide_before_commit_hook, set_guide_before_publish_hook,
    set_initialization_after_bootstrap_plan_hook, set_initialization_after_started_hook,
    set_initialization_before_commit_hook, set_startup_initialization_inference_hook,
};

const RECOVERY_REQUIRED: &str = "runtime recovery is required";
const BOOTSTRAP_PLAN: &str = "bootstrap.json";
/// Refusal shared by both initializers when the other one holds the lock.
const INITIALIZATION_IN_PROGRESS: &str =
    "another p-track initialization is in progress; retry once it finishes";
const BOOTSTRAP_LIMIT: u64 = 1024 * 1024;
const DESKTOP_INITIALIZATION: &str = "desktop-initialization.json";
const DESKTOP_INITIALIZATION_LOCK: &str = "desktop-initialization.lock";
const DESKTOP_INITIALIZATION_LIMIT: u64 = 64 * 1024;
const DESKTOP_INITIALIZATION_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const GUIDE_FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];
const GUIDE_FILE_LIMIT: u64 = 32 * 1024;
const GUIDE_OUTPUT_LIMIT: usize = 64 * 1024;
const GUIDE_DIFF_LIMIT: usize = 64 * 1024;
const GUIDE_DIFF_LINE_LIMIT: usize = 4_096;
const GUIDE_PREVIEW_LIMIT: usize = 8;
const GUIDE_PREVIEW_STALE: &str = "project-guide-preview-stale";
const GUIDE_PARTIALLY_APPLIED: &str = "project-guide-partially-applied";
const RECENT_CONFIRMATION_LIMIT: usize = 64;
const RECENT_CONFIRMATION_TTL: Duration = Duration::from_secs(120);
const RECENT_LISTING_TTL: Duration = Duration::from_secs(600);
const RECENT_ID_BYTES: usize = 43;
const RECENT_PATH_LIMIT: usize = 16 * 1024;
#[cfg(not(unix))]
const GUIDE_UNAVAILABLE: &str = "Project guidance is not available on this platform yet";

fn recovery(error: impl std::fmt::Display) -> AppError {
    AppError::Message(format!("{RECOVERY_REQUIRED}: {error}"))
}

fn uninitialized() -> AppError {
    AppError::Message("p-track runtime is not initialized (run 'ptrack init')".to_owned())
}

fn path_is_present(path: &Path) -> AppResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn content_digest(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(sha256_digest(bytes))
}

fn random_id() -> AppResult<String> {
    let mut raw = [0_u8; 16];
    getrandom::fill(&mut raw)
        .map_err(|_| AppError::Message("runtime identity could not be created".to_owned()))?;
    Ok(URL_SAFE_NO_PAD.encode(raw))
}

fn random_operation_id() -> AppResult<String> {
    let mut raw = [0_u8; 32];
    getrandom::fill(&mut raw).map_err(|_| {
        AppError::Message("initialization identity could not be created".to_owned())
    })?;
    Ok(URL_SAFE_NO_PAD.encode(raw))
}

fn validate_operation_id(value: &str) -> AppResult<()> {
    if value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err(AppError::Message(
            "initialization operation is invalid".to_owned(),
        ))
    }
}

fn project_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .to_owned()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
