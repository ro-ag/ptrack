//! Starts `ptrack gui` without a console and reports a failed start in a
//! dialog. Shared by `p-track.exe`, which runs the `ptrack.exe` beside it,
//! and the single-file portable, which runs the copy it unpacked.

use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

/// How much of a failed start's stderr is kept for the error dialog.
const MAX_REPORTED_STDERR: usize = 4 * 1024;

/// Runs `program gui <args>` with no console and waits for it to end.
///
/// # Errors
/// Returns the message to show when the desktop process cannot start, or
/// when it fails and explains why on stderr.
pub fn launch_gui(program: &Path, args: impl IntoIterator<Item = OsString>) -> Result<(), String> {
    let mut command = Command::new(program);
    command
        .arg("gui")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // CREATE_NO_WINDOW: the desktop process must not get a console.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("p-track could not start {}: {error}", program.display()))?;
    // Drained to the end so the desktop process never blocks on a full pipe;
    // only the head is kept for the dialog.
    let mut stderr = Vec::new();
    if let Some(pipe) = child.stderr.take() {
        drain_bounded(pipe, &mut stderr);
    }
    let status = child
        .wait()
        .map_err(|error| format!("p-track lost track of the desktop process: {error}"))?;
    // Only a failure ptrack explained is shown: a start-up refusal or a panic
    // writes to stderr, while a process ended from outside (Task Manager,
    // sign-out) has nothing to say and must not raise a dialog.
    let detail = String::from_utf8_lossy(&stderr).trim().to_owned();
    if status.success() || detail.is_empty() {
        return Ok(());
    }
    Err(detail)
}

fn drain_bounded(mut pipe: impl Read, kept: &mut Vec<u8>) {
    let mut buffer = [0_u8; 4096];
    while let Ok(count) = pipe.read(&mut buffer) {
        if count == 0 {
            break;
        }
        let room = MAX_REPORTED_STDERR.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..count.min(room)]);
    }
}

/// Shows `message` in an error dialog; a GUI program has no console to print to.
#[cfg(windows)]
pub fn report(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

    let wide = |text: &str| {
        text.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    };
    let text = wide(message);
    let caption = wide("p-track");
    // SAFETY: both buffers are NUL-terminated UTF-16 that outlive the
    // synchronous call; a null owner window is permitted.
    #[allow(unsafe_code)]
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// Prints `message` to stderr where there is no dialog to show it in.
#[cfg(not(windows))]
pub fn report(message: &str) {
    eprintln!("{message}");
}
