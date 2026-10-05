//! Compresses the `ptrack.exe` named by `PTRACK_PORTABLE_PAYLOAD` into the
//! portable executable, with its SHA-256 and size, and embeds the p-track
//! icon and version details.
//!
//! Without `PTRACK_PORTABLE_PAYLOAD` the build carries no payload, so
//! workspace builds, tests, and lints need no release binary; such a stub
//! refuses to run, and the release contract rejects it by size.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::DeflateEncoder;
use sha2::{Digest as _, Sha256};

/// Opens the digest record; `build/windows/package.ps1` searches for it.
const PAYLOAD_MARKER: &[u8] = b"ptrack-portable-sha256:";

fn main() {
    println!("cargo:rerun-if-changed=../../src-tauri/icons/icon.ico");
    println!("cargo:rerun-if-env-changed=PTRACK_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=PTRACK_PORTABLE_PAYLOAD");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));

    let (compressed, digest, size) =
        match std::env::var_os("PTRACK_PORTABLE_PAYLOAD").filter(|path| !path.is_empty()) {
            Some(path) => {
                let path = PathBuf::from(path);
                println!("cargo:rerun-if-changed={}", path.display());
                let raw = std::fs::read(&path).unwrap_or_else(|error| {
                    panic!("read the portable payload {}: {error}", path.display())
                });
                check_machine(&path, &raw);
                let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
                let compressed = encoder
                    .write_all(&raw)
                    .and_then(|()| encoder.finish())
                    .unwrap_or_else(|error| panic!("compress the portable payload: {error}"));
                (compressed, Sha256::digest(&raw).into(), raw.len())
            }
            None => (Vec::new(), [0_u8; 32], 0),
        };
    std::fs::write(out.join("payload.deflate"), &compressed)
        .expect("write the compressed portable payload");
    let mut record = PAYLOAD_MARKER.to_vec();
    record.extend_from_slice(&digest);
    std::fs::write(
        out.join("payload.rs"),
        format!(
            "/// Size in bytes of the unpacked `ptrack.exe`.\n\
             const PAYLOAD_SIZE: u64 = {size};\n\
             /// Length of the marker that opens `PAYLOAD_RECORD`.\n\
             const PAYLOAD_MARKER_BYTES: usize = {marker};\n\
             /// The marker, then the SHA-256 of the unpacked `ptrack.exe`, kept\n\
             /// intact in the binary so packaging can tell which build it carries.\n\
             #[used]\n\
             static PAYLOAD_RECORD: [u8; {length}] = {record:?};\n",
            marker = PAYLOAD_MARKER.len(),
            length = record.len(),
        ),
    )
    .expect("write the portable payload digest");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resources();
    }
}

/// Refuses a payload built for another CPU than the portable itself.
fn check_machine(path: &Path, raw: &[u8]) {
    let wanted: u16 = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => 0x8664,
        Ok("aarch64") => 0xAA64,
        _ => return,
    };
    let machine = raw
        .get(60..64)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_le_bytes)
        .and_then(|offset| usize::try_from(offset).ok())
        .filter(|&offset| raw.get(offset..offset + 4) == Some(b"PE\0\0"))
        .and_then(|offset| raw.get(offset + 4..offset + 6))
        .and_then(|bytes| bytes.try_into().ok())
        .map(u16::from_le_bytes);
    assert!(
        machine == Some(wanted),
        "the portable payload {} is not a Windows executable for this target",
        path.display()
    );
}

fn embed_resources() {
    let mut resource = tauri_winres::WindowsResource::new();
    resource
        .set_icon("../../src-tauri/icons/icon.ico")
        .set("ProductName", "p-track")
        .set("FileDescription", "p-track")
        .set("InternalName", "p-track-portable")
        .set("OriginalFilename", "p-track-portable.exe");
    if let Ok(version) = std::env::var("PTRACK_BUILD_VERSION")
        && !version.is_empty()
    {
        resource
            .set("ProductVersion", &version)
            .set("FileVersion", &version);
    }
    if let Err(error) = resource.compile() {
        panic!("embed the p-track portable resources: {error}");
    }
}
