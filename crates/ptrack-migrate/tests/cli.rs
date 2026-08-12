use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

#[test]
fn inspect_requires_an_explicit_absolute_bundle_path() {
    for arguments in [
        Vec::<&str>::new(),
        vec!["inspect"],
        vec!["inspect", "--bundle", "relative.bundle"],
        vec!["import", "--bundle", "/tmp/anything"],
        vec![
            "import",
            "--bundle",
            "/tmp/anything",
            "--destination",
            "/tmp/anything.redb",
        ],
        vec![
            "import",
            "--destination",
            "/tmp/anything.redb",
            "--bundle",
            "/tmp/anything",
            "--accept-one-way",
        ],
        vec![
            "import",
            "--bundle",
            "/tmp/anything",
            "--destination",
            "/tmp/anything.redb",
            "--accept-one-way",
            "--force",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ptrack-migrate"))
            .args(arguments)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).starts_with("ptrack-migrate:"));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn import_requires_explicit_one_way_acceptance_and_creates_verified_destination() {
    let directory = TestDirectory::new();
    let bundle = directory.path("global.bundle");
    let destination = directory.path("global.redb");
    fs::write(&bundle, EMPTY_GLOBAL_BUNDLE).unwrap();

    let rejected = Command::new(env!("CARGO_BIN_EXE_ptrack-migrate"))
        .args([
            "import",
            "--bundle",
            bundle.to_str().unwrap(),
            "--destination",
            destination.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(!destination.exists());

    let accepted = Command::new(env!("CARGO_BIN_EXE_ptrack-migrate"))
        .args([
            "import",
            "--bundle",
            bundle.to_str().unwrap(),
            "--destination",
            destination.to_str().unwrap(),
            "--accept-one-way",
        ])
        .output()
        .unwrap();
    assert!(accepted.status.success());
    assert!(accepted.stderr.is_empty());
    let stdout = String::from_utf8(accepted.stdout).unwrap();
    assert!(stdout.starts_with("verified global import: collections=3, records=0"));
    assert!(stdout.contains(destination.to_str().unwrap()));
    assert!(destination.is_file());
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let number = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ptrack-migrate-cli-{}-{number}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const EMPTY_GLOBAL_BUNDLE: &[u8] = &[
    0x50, 0x54, 0x52, 0x4b, 0x4d, 0x49, 0x47, 0x31, 0x00, 0x01, 0x00, 0x28, 0x02, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x42, 0x55, 0x4b, 0x54, 0x00, 0x07, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x62, 0x61, 0x63, 0x6b, 0x75, 0x70, 0x73, 0x42, 0x55, 0x4b, 0x54, 0x00, 0x06, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x63,
    0x6f, 0x6e, 0x66, 0x69, 0x67, 0x42, 0x55, 0x4b, 0x54, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x72, 0x6f,
    0x6a, 0x65, 0x63, 0x74, 0x73, 0x48, 0x41, 0x53, 0x48, 0x00, 0x01, 0x00, 0x20, 0x47, 0x97, 0xb7,
    0x53, 0x3a, 0xd3, 0x05, 0xc3, 0xad, 0x99, 0x22, 0x81, 0xb8, 0x6d, 0x16, 0xaa, 0xad, 0x91, 0x72,
    0xbd, 0xb5, 0x30, 0xcb, 0xb0, 0x26, 0x82, 0xbf, 0x7b, 0xd9, 0x81, 0xb6, 0xf1,
];
