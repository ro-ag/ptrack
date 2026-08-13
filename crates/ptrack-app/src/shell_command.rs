#[cfg(not(unix))]
use std::fs::OpenOptions;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use serde::Serialize;

pub(crate) const SHELL_PATH_MARKER_BEGIN: &str = "# >>> ptrack cli >>>";
pub(crate) const SHELL_PATH_MARKER_END: &str = "# <<< ptrack cli <<<";
const MAX_DIALOG_BYTES: usize = 4_096;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellCommandInstallResult {
    pub message: String,
}

/// Appends the idempotent p-track PATH block to the user's zsh profile and
/// returns the exact native-dialog message. File errors are deliberately
/// represented as the dialog message, preserving the no-error GUI contract.
#[must_use]
pub fn install_shell_command() -> ShellCommandInstallResult {
    let message = std::env::current_exe()
        .map_err(|error| format!("cannot locate the ptrack binary: {error}"))
        .and_then(|executable| {
            let home = user_home().ok_or_else(|| {
                "cannot locate your home directory: home is unavailable".to_owned()
            })?;
            install_shell_command_from(&executable, &home)
        })
        .unwrap_or_else(|error| error);
    ShellCommandInstallResult {
        message: bound_message(message),
    }
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn install_shell_command_from(executable: &Path, home: &Path) -> Result<String, String> {
    let bin_dir = executable
        .parent()
        .ok_or_else(|| "cannot locate the ptrack binary: executable has no directory".to_owned())?;
    let bin_text = bin_dir
        .to_str()
        .ok_or_else(|| "cannot locate the ptrack binary: path is not UTF-8".to_owned())?;
    if bin_text
        .chars()
        .any(|character| matches!(character, '"' | '$' | '`' | '\\' | '\n' | '\r'))
    {
        return Err("cannot update shell PATH: executable directory is unsafe".to_owned());
    }
    let profile = home.join(".zprofile");
    match ensure_shell_path(&profile, bin_dir) {
        Ok(true) => Ok(format!(
            "Added to PATH in {}:\n\n{}\n\nOpen a new terminal window, then run `ptrack`.",
            profile.display(),
            bin_dir.display()
        )),
        Ok(false) => Ok(format!(
            "Already on PATH via {}:\n\n{}",
            profile.display(),
            bin_dir.display()
        )),
        Err(error) => Err(bound_message(error)),
    }
}

pub(crate) fn ensure_shell_path(profile: &Path, bin_dir: &Path) -> Result<bool, String> {
    let mut read_file = open_profile_read(profile)
        .map_err(|error| format!("cannot update {}: {error}", profile.display()))?;
    if !read_file
        .metadata()
        .map_err(|error| format!("cannot read {}: {error}", profile.display()))?
        .is_file()
    {
        return Err(format!(
            "cannot read {}: not a regular file",
            profile.display()
        ));
    }
    let mut data = Vec::new();
    read_file
        .read_to_end(&mut data)
        .map_err(|error| format!("cannot read {}: {error}", profile.display()))?;
    if data
        .windows(SHELL_PATH_MARKER_BEGIN.len())
        .any(|window| window == SHELL_PATH_MARKER_BEGIN.as_bytes())
    {
        return Ok(false);
    }
    let mut block = Vec::new();
    if !data.is_empty() && !data.ends_with(b"\n") {
        block.push(b'\n');
    }
    block.extend_from_slice(SHELL_PATH_MARKER_BEGIN.as_bytes());
    block.extend_from_slice(
        b"\n# Added by p-track: makes the `ptrack` CLI available in new terminal sessions.\n",
    );
    writeln!(block, "export PATH=\"$PATH:{}\"", bin_dir.display())
        .map_err(|error| format!("cannot update {}: {error}", profile.display()))?;
    block.extend_from_slice(SHELL_PATH_MARKER_END.as_bytes());
    block.push(b'\n');

    let mut write_file = open_profile_append(profile)
        .map_err(|error| format!("cannot update {}: {error}", profile.display()))?;
    if !same_profile(&read_file, &write_file)
        .map_err(|error| format!("cannot update {}: {error}", profile.display()))?
    {
        return Err(format!(
            "cannot update {}: profile changed while it was being read",
            profile.display()
        ));
    }
    write_file
        .write_all(&block)
        .map_err(|error| format!("cannot update {}: {error}", profile.display()))?;
    Ok(true)
}

#[cfg(unix)]
fn open_profile_read(profile: &Path) -> std::io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags};

    let descriptor = rustix::fs::open(
        profile,
        OFlags::RDONLY | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR | Mode::RGRP | Mode::ROTH,
    )?;
    Ok(std::fs::File::from(descriptor))
}

#[cfg(not(unix))]
fn open_profile_read(profile: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).open(profile)
}

#[cfg(unix)]
fn open_profile_append(profile: &Path) -> std::io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags};

    let descriptor = rustix::fs::open(
        profile,
        OFlags::WRONLY | OFlags::APPEND | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?;
    Ok(std::fs::File::from(descriptor))
}

#[cfg(not(unix))]
fn open_profile_append(profile: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.append(true).open(profile)
}

#[cfg(unix)]
fn same_profile(left: &std::fs::File, right: &std::fs::File) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt as _;

    let left = left.metadata()?;
    let right = right.metadata()?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(not(unix))]
fn same_profile(_left: &std::fs::File, _right: &std::fs::File) -> std::io::Result<bool> {
    Ok(true)
}

fn bound_message(mut message: String) -> String {
    if message.len() <= MAX_DIALOG_BYTES {
        return message;
    }
    let mut end = MAX_DIALOG_BYTES.saturating_sub(3);
    while !message.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    message.truncate(end);
    message.push_str("...");
    message
}
