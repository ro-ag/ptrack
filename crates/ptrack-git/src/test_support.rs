use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use crate::runner::{CancellationToken, RepositoryError, Runner, git_environment};

type Responses = BTreeMap<String, VecDeque<Result<Vec<u8>, RepositoryError>>>;

#[derive(Default)]
pub(crate) struct FakeRunner {
    responses: Mutex<Responses>,
    calls: Mutex<Vec<(PathBuf, Vec<OsString>)>>,
}

impl FakeRunner {
    pub(crate) fn output(&self, key: &str, output: impl Into<Vec<u8>>) {
        self.response(key, Ok(output.into()));
    }

    pub(crate) fn error(&self, key: &str, error: RepositoryError) {
        self.response(key, Err(error));
    }

    fn response(&self, key: &str, response: Result<Vec<u8>, RepositoryError>) {
        self.responses
            .lock()
            .expect("fake responses lock poisoned")
            .entry(key.to_owned())
            .or_default()
            .push_back(response);
    }

    pub(crate) fn calls(&self) -> Vec<(PathBuf, Vec<OsString>)> {
        self.calls.lock().expect("fake calls lock poisoned").clone()
    }
}

impl Runner for FakeRunner {
    fn output(
        &self,
        cancellation: &CancellationToken,
        root: &Path,
        args: &[OsString],
    ) -> Result<Vec<u8>, RepositoryError> {
        if cancellation.is_cancelled() {
            return Err(RepositoryError::Cancelled);
        }
        self.calls
            .lock()
            .expect("fake calls lock poisoned")
            .push((root.to_owned(), args.to_vec()));
        let key = command_key(root, args);
        self.responses
            .lock()
            .expect("fake responses lock poisoned")
            .get_mut(&key)
            .and_then(VecDeque::pop_front)
            .unwrap_or(Err(RepositoryError::CommandFailed))
    }
}

pub(crate) fn command_key(root: &Path, args: &[OsString]) -> String {
    let command = args
        .first()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    if command == "for-each-ref" {
        return format!(
            "{}|{}:{}",
            root.display(),
            command,
            args.last().unwrap_or(&OsString::new()).to_string_lossy()
        );
    }
    if command == "log" && args.iter().any(|arg| arg == "--end-of-options") {
        return format!("{}|log:range", root.display());
    }
    format!("{}|{command}", root.display())
}

pub(crate) fn run_git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_clear()
        .envs(git_environment(std::env::vars_os()))
        .stdin(std::process::Stdio::null())
        .status()
        .expect("launch disposable git command");
    assert!(status.success(), "disposable git command failed: {args:?}");
}

/// Canonicalizes a test path into the same form the product emits: on Windows
/// the `\\?\` (and `\\?\UNC\`) verbatim prefixes added by `fs::canonicalize`
/// are stripped, mirroring `worktree::normalize_canonical`.
pub(crate) fn canonical(path: &Path) -> PathBuf {
    normalize_canonical(std::fs::canonicalize(path).expect("canonicalize test path"))
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

/// Applies the same lexical component normalization the product applies to
/// git-reported paths (`Path` components collected back into a `PathBuf`), so
/// expected values are derived per platform instead of hardcoded.
pub(crate) fn native_path(value: &str) -> String {
    Path::new(value)
        .components()
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn sha(character: char) -> String {
    std::iter::repeat_n(character, 40).collect()
}
