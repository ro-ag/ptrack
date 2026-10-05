//! Unpacks the embedded `ptrack.exe` into a folder named by its digest and
//! hands back a verified copy that stays open against changes.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use flate2::read::DeflateDecoder;
use sha2::{Digest as _, Sha256};

const BUFFER_BYTES: usize = 64 * 1024;

/// The compressed executable and what it must unpack to.
pub(crate) struct Payload<'a> {
    pub compressed: &'a [u8],
    pub sha256: [u8; 32],
    pub size: u64,
}

/// A verified unpacked `ptrack.exe`, held open so it cannot be rewritten or
/// deleted while it is in use.
pub(crate) struct Unpacked {
    program: PathBuf,
    _held: File,
}

impl Unpacked {
    pub(crate) fn program(&self) -> &Path {
        &self.program
    }

    pub(crate) fn folder(&self) -> &Path {
        self.program.parent().unwrap_or(&self.program)
    }
}

/// Returns the verified copy under `root`, unpacking it first when it is
/// missing, damaged, or replaced.
///
/// # Errors
/// Returns the message to show when no verified copy can be produced.
pub(crate) fn unpack(root: &Path, payload: &Payload<'_>) -> Result<Unpacked, String> {
    let program = root.join(hex(&payload.sha256)).join("ptrack.exe");
    if let Some(held) = open_verified(&program, payload) {
        return Ok(Unpacked {
            program,
            _held: held,
        });
    }
    let unpacked = extract(&program, payload);
    // A start running at the same moment may have unpacked the same copy
    // first and already be running it, which makes the replace fail.
    match open_verified(&program, payload) {
        Some(held) => Ok(Unpacked {
            program,
            _held: held,
        }),
        None => Err(unpacked.err().unwrap_or_else(|| {
            format!(
                "p-track unpacked {} but the copy failed verification",
                program.display()
            )
        })),
    }
}

/// Removes copies unpacked by other versions. A copy that is still running
/// cannot be deleted on Windows and stays until a later start.
pub(crate) fn prune(root: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_copy = entry.file_name().to_str().is_some_and(|name| {
            name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
        if is_copy && path != keep && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

/// Opens `program` and returns it only when its size and digest match.
fn open_verified(program: &Path, payload: &Payload<'_>) -> Option<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        // FILE_SHARE_READ alone: while this handle is open nothing can write,
        // rename, or delete the copy, yet the loader can still map it.
        options.share_mode(0x0000_0001);
    }
    let mut file = options.open(program).ok()?;
    if file.metadata().ok()?.len() != payload.size {
        return None;
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    loop {
        let count = file.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    (hasher.finalize().as_slice() == payload.sha256).then_some(file)
}

fn extract(program: &Path, payload: &Payload<'_>) -> Result<(), String> {
    let folder = program.parent().unwrap_or(program);
    fs::create_dir_all(folder)
        .map_err(|error| format!("p-track could not create {}: {error}", folder.display()))?;
    let staging = folder.join(format!("ptrack.exe.{}.partial", std::process::id()));
    let _ = fs::remove_file(&staging);
    let result = write_payload(&staging, payload)
        .and_then(|()| fs::rename(&staging, program))
        .map_err(|error| format!("p-track could not unpack {}: {error}", program.display()));
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
}

/// Inflates the payload into a new file, refusing any output that is larger
/// than recorded or does not match the recorded digest.
fn write_payload(staging: &Path, payload: &Payload<'_>) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staging)?;
    let mut decoder = DeflateDecoder::new(payload.compressed);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    let mut written = 0_u64;
    loop {
        let count = decoder.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        written += count as u64;
        if written > payload.size {
            return Err(invalid("the embedded ptrack.exe is larger than recorded"));
        }
        hasher.update(&buffer[..count]);
        file.write_all(&buffer[..count])?;
    }
    if written != payload.size || hasher.finalize().as_slice() != payload.sha256 {
        return Err(invalid("the embedded ptrack.exe does not match its digest"));
    }
    file.sync_all()
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn hex(digest: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    digest
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

#[cfg(test)]
#[path = "unpack_test.rs"]
mod tests;
