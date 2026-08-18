use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{CutoverLockMode, acquire_cutover_lock, protect_private_directory};
#[cfg(unix)]
use crate::{acquire_legacy_read_lease, verify_legacy_source_identity};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ptrack-cutover-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        protect_private_directory(&path).unwrap();
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn shared_runtime_leases_exclude_activation_until_all_are_dropped() {
    let temp = Temp::new();
    let first = acquire_cutover_lock(&temp.0, CutoverLockMode::Shared).unwrap();
    let second = acquire_cutover_lock(&temp.0, CutoverLockMode::Shared).unwrap();
    let error = acquire_cutover_lock(&temp.0, CutoverLockMode::Exclusive).unwrap_err();
    assert!(error.to_string().contains("cutover lock is unavailable"));
    drop(first);
    drop(second);
    let exclusive = acquire_cutover_lock(&temp.0, CutoverLockMode::Exclusive).unwrap();
    assert_eq!(exclusive.path(), temp.0.join("runtime/cutover.lock"));
}

#[cfg(unix)]
#[test]
fn leaked_global_home_permissions_are_healed_on_use() {
    use std::os::unix::fs::PermissionsExt;

    // A home directory that leaked group/other bits — a restore, a sync, a
    // copy under a default umask — is tightened to owner-only, never refused:
    // removing access cannot leak anything, while failing closed locked the
    // whole runtime out (v0.24.x field reports, file and directory alike).
    let temp = Temp::new();
    fs::set_permissions(&temp.0, fs::Permissions::from_mode(0o755)).unwrap();
    drop(acquire_cutover_lock(&temp.0, CutoverLockMode::Shared).unwrap());
    assert_eq!(
        fs::metadata(&temp.0).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[cfg(unix)]
#[test]
fn legacy_source_lock_pins_identity_without_requiring_new_private_policy() {
    use std::os::unix::fs::PermissionsExt;

    let temp = Temp::new();
    let legacy = temp.0.join("legacy.db");
    fs::write(&legacy, b"legacy").unwrap();
    fs::set_permissions(&legacy, fs::Permissions::from_mode(0o644)).unwrap();
    let lease = acquire_legacy_read_lease(&legacy).unwrap();
    assert_eq!(
        lease.identity(),
        verify_legacy_source_identity(&legacy).unwrap()
    );
}
