//! `p-track-portable.exe`: p-track for Windows in a single file.
//!
//! Released as `p-track_<version>_windows_<arch>_portable.exe`. It carries the
//! release `ptrack.exe` deflate-compressed inside itself, unpacks it once into
//! `%LOCALAPPDATA%\p-track\portable\<sha256>\`, and starts `ptrack gui` from
//! there with no console, as `p-track.exe` does from the portable folder.
//! Every start re-verifies the unpacked copy against the digest built into
//! this file, so a damaged or replaced copy is unpacked again, never run.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod unpack;

use std::path::PathBuf;
use std::process::ExitCode;

include!(concat!(env!("OUT_DIR"), "/payload.rs"));

/// The release `ptrack.exe`, deflate-compressed by `build.rs`.
static COMPRESSED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.deflate"));

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
    if COMPRESSED.is_empty() {
        return Err(
            "this p-track build carries no ptrack.exe; build it with PTRACK_PORTABLE_PAYLOAD set"
                .to_owned(),
        );
    }
    let root = unpack_root()?;
    let mut sha256 = [0_u8; 32];
    sha256.copy_from_slice(&PAYLOAD_RECORD[PAYLOAD_MARKER_BYTES..]);
    let payload = unpack::Payload {
        compressed: COMPRESSED,
        sha256,
        size: PAYLOAD_SIZE,
    };
    // Held until the desktop process ends, so the verified copy cannot be
    // changed between its check and its start.
    let unpacked = unpack::unpack(&root, &payload)?;
    unpack::prune(&root, unpacked.folder());
    ptrack_launcher::launch_gui(unpacked.program(), std::env::args_os().skip(1))
}

/// `%LOCALAPPDATA%\p-track\portable`: per-user and never roamed.
fn unpack_root() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|path| path.join("p-track").join("portable"))
        .ok_or_else(|| {
            "p-track could not find your local application data folder (LOCALAPPDATA)".to_owned()
        })
}
