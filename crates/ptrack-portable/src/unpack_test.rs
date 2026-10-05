use std::io::Write as _;
use std::path::PathBuf;

use flate2::Compression;
use flate2::write::DeflateEncoder;
use sha2::{Digest as _, Sha256};

use super::{Payload, hex, prune, unpack};

const EXECUTABLE: &[u8] = b"MZ stand-in for ptrack.exe, repeated to compress. MZ stand-in.";

fn compress(data: &[u8]) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn payload<'a>(compressed: &'a [u8], data: &[u8]) -> Payload<'a> {
    Payload {
        compressed,
        sha256: Sha256::digest(data).into(),
        size: data.len() as u64,
    }
}

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "ptrack-portable-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn unpacks_into_a_folder_named_by_the_digest() {
    let root = temp_root("fresh");
    let compressed = compress(EXECUTABLE);
    let payload = payload(&compressed, EXECUTABLE);
    let unpacked = unpack(&root, &payload).unwrap();
    assert_eq!(
        unpacked.program(),
        root.join(hex(&payload.sha256)).join("ptrack.exe")
    );
    assert_eq!(std::fs::read(unpacked.program()).unwrap(), EXECUTABLE);
    // Only the executable remains; the staging file was renamed into place.
    assert_eq!(std::fs::read_dir(unpacked.folder()).unwrap().count(), 1);
    drop(unpacked);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reuses_a_verified_copy_and_replaces_a_changed_one() {
    let root = temp_root("reuse");
    let compressed = compress(EXECUTABLE);
    let payload = payload(&compressed, EXECUTABLE);
    let program = unpack(&root, &payload).unwrap().program().to_path_buf();
    let first = std::fs::metadata(&program).unwrap().modified().unwrap();
    drop(unpack(&root, &payload).unwrap());
    assert_eq!(
        std::fs::metadata(&program).unwrap().modified().unwrap(),
        first
    );

    // Same size, different bytes: only the digest tells them apart.
    let mut tampered = EXECUTABLE.to_vec();
    tampered[3] ^= 0xff;
    std::fs::write(&program, &tampered).unwrap();
    drop(unpack(&root, &payload).unwrap());
    assert_eq!(std::fs::read(&program).unwrap(), EXECUTABLE);

    std::fs::write(&program, b"short").unwrap();
    drop(unpack(&root, &payload).unwrap());
    assert_eq!(std::fs::read(&program).unwrap(), EXECUTABLE);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn refuses_a_payload_that_does_not_match_its_record() {
    let root = temp_root("mismatch");
    let compressed = compress(EXECUTABLE);
    let mut wrong_digest = payload(&compressed, EXECUTABLE);
    wrong_digest.sha256[0] ^= 0xff;
    let error = unpack(&root, &wrong_digest).err().unwrap();
    assert!(error.contains("does not match its digest"), "{error}");

    let mut too_small = payload(&compressed, EXECUTABLE);
    too_small.size -= 1;
    let error = unpack(&root, &too_small).err().unwrap();
    assert!(error.contains("larger than recorded"), "{error}");

    // Neither attempt leaves an executable or a staging file behind.
    for record in [&wrong_digest, &too_small] {
        let folder = root.join(hex(&record.sha256));
        assert_eq!(std::fs::read_dir(folder).unwrap().count(), 0);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn prune_removes_only_other_unpacked_versions() {
    let root = temp_root("prune");
    let compressed = compress(EXECUTABLE);
    let payload = payload(&compressed, EXECUTABLE);
    let older = root.join("ab".repeat(32));
    std::fs::create_dir_all(&older).unwrap();
    std::fs::write(older.join("ptrack.exe"), b"older").unwrap();
    let unrelated = root.join("notes");
    std::fs::create_dir_all(&unrelated).unwrap();

    let unpacked = unpack(&root, &payload).unwrap();
    prune(&root, unpacked.folder());
    assert!(!older.exists());
    assert!(unrelated.exists());
    assert!(unpacked.program().exists());
    drop(unpacked);
    std::fs::remove_dir_all(root).unwrap();
}
