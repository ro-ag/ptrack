//! `p-track.exe`: the Windows desktop entry point.
//!
//! `ptrack.exe` is a console program so the CLI and the terminal dashboard
//! work from any shell, but started from the Start menu or Explorer it would
//! open a console window beside the desktop workspace. This launcher is a GUI
//! program that starts `ptrack gui` from its own folder with no console — the
//! Windows counterpart of the macOS bundle's `p-track` launcher script.
//! Running `ptrack` with no subcommand stays the terminal CLI.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            ptrack_launcher::report(&message);
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
    ptrack_launcher::launch_gui(&program, std::env::args_os().skip(1))
}
