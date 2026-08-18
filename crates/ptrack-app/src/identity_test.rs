use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ptrack_core::is_identity_id;
use ptrack_store::{ActiveBinding, GlobalStore, StoreKind};

use crate::identity::{load_identity, set_identity_name};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ptrack-identity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        ptrack_store::protect_private_directory(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn test_global_store() -> (Temp, GlobalStore) {
    let temp = Temp::new();
    let database = temp.0.join("global.redb");
    let binding = ActiveBinding {
        generation: 1,
        database_id: "identity-test".to_owned(),
        kind: StoreKind::Global,
        canonical_path: database.clone(),
    };
    let store = GlobalStore::create_new(&database, binding).unwrap();
    (temp, store)
}

#[test]
fn set_user_mints_once_and_renames_in_place() {
    let (_temp, store) = test_global_store();
    assert_eq!(load_identity(&store).unwrap(), None);
    let first = set_identity_name(&store, "Rodrigo").unwrap();
    assert!(is_identity_id(&first.id));
    assert_eq!(first.name, "Rodrigo");
    let renamed = set_identity_name(&store, "Rod").unwrap();
    assert_eq!(renamed.id, first.id, "rename must not re-mint the identity");
    assert_eq!(renamed.name, "Rod");
    assert_eq!(load_identity(&store).unwrap(), Some(renamed));
}

#[test]
fn malformed_stored_identity_fails_closed_on_load_and_repairs_on_set() {
    let (_temp, store) = test_global_store();
    store
        .set_config(crate::identity::IDENTITY_CONFIG_KEY, b"garbage-no-tab")
        .unwrap();
    assert!(load_identity(&store).is_err());
    let repaired = set_identity_name(&store, "Rodrigo").unwrap();
    assert!(is_identity_id(&repaired.id));
    assert_eq!(load_identity(&store).unwrap(), Some(repaired));
}

#[test]
fn invalid_names_are_refused() {
    let (_temp, store) = test_global_store();
    assert!(set_identity_name(&store, "   ").is_err());
    assert!(set_identity_name(&store, "two\nlines").is_err());
    assert!(set_identity_name(&store, &"x".repeat(65)).is_err());
}
