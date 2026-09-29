//! `p-track.exe`: the Windows desktop entry point.
//!
//! `ptrack.exe` is a console program so the CLI and the terminal dashboard
//! work from any shell, but started from the Start menu or Explorer it would
//! open a console window beside the desktop workspace. This launcher is a GUI
//! program that starts `ptrack gui` from its own folder with no console — the
//! Windows counterpart of the macOS bundle's `p-track` launcher script.
//! Running `ptrack` with no subcommand stays the terminal CLI.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::Read;
use std::process::{Command, ExitCode, Stdio};

/// How much of a failed start's stderr is kept for the error dialog.
const MAX_REPORTED_STDERR: usize = 4 * 1024;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            report(&message);
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let launcher = std::env::current_exe()
        .map_err(|error| format!("p-track could not locate its own folder: {error}"))?;
    let program = launcher.with_file_name(if cfg!(windows) {
        "ptrack.exe"
    } else {
        "ptrack"
    });
    let mut command = Command::new(&program);
    command
        .arg("gui")
        .args(std::env::args_os().skip(1))
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

#[cfg(windows)]
fn report(message: &str) {
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

#[cfg(not(windows))]
fn report(message: &str) {
    eprintln!("{message}");
}
