use std::fs;

use ptrack_store::{Store, StoreKind};

use super::stage_test::empty_global_stage;
use super::workflow::{
    import_stage, import_stage_with_after_incomplete, import_stage_with_before_create,
};

#[test]
fn imports_to_an_absent_root_and_writes_receipt_last() {
    let (source_root, manifest) = empty_global_stage();
    let destination = source_root.with_extension("candidates");

    let refused = import_stage(&manifest, &destination, false).unwrap_err();
    assert!(refused.to_string().contains("--accept-all"));
    assert!(!destination.exists());

    let receipt = import_stage(&manifest, &destination, true).expect("candidate import");
    assert_eq!(receipt.candidate_count, 1);
    assert!(destination.join("receipt.json").is_file());
    assert!(!destination.join("incomplete.json").exists());
    let candidate = destination.join("0000-global.redb");
    let store = Store::open_existing(candidate, StoreKind::Global).expect("reopen candidate");
    assert_eq!(
        store
            .json_stage_provenance()
            .expect("provenance read")
            .expect("JSON provenance")
            .batch_manifest_sha256,
        receipt.report.manifest_sha256
    );

    drop(store);
    fs::remove_dir_all(source_root).expect("remove temporary stage");
    fs::remove_dir_all(destination).expect("remove temporary destination");
}

#[cfg(unix)]
#[test]
fn refuses_a_replaced_destination_root() {
    use std::os::unix::fs::PermissionsExt;

    let (source_root, manifest) = empty_global_stage();
    let destination = source_root.with_extension("swap-candidates");
    let moved = source_root.with_extension("original-candidates");
    let result = import_stage_with_after_incomplete(&manifest, &destination, |root| {
        fs::rename(root, &moved).expect("move pinned root");
        fs::create_dir(root).expect("replacement root");
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).expect("private replacement");
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("destination root identity changed")
    );
    assert!(!destination.join("receipt.json").exists());
    assert!(moved.join("incomplete.json").exists());

    fs::remove_dir_all(source_root).expect("remove temporary stage");
    fs::remove_dir_all(destination).expect("remove replacement destination");
    fs::remove_dir_all(moved).expect("remove moved destination");
}

#[cfg(unix)]
#[test]
fn refuses_a_replaced_destination_parent_before_root_creation() {
    let (source_root, manifest) = empty_global_stage();
    let parent = source_root.with_extension("destination-parent");
    let moved = source_root.with_extension("original-parent");
    fs::create_dir(&parent).expect("destination parent");
    let destination = parent.join("candidates");
    let result = import_stage_with_before_create(&manifest, &destination, |captured_parent| {
        assert_eq!(captured_parent, parent);
        fs::rename(&parent, &moved).expect("move pinned parent");
        fs::create_dir(&parent).expect("replacement parent");
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("destination parent identity changed")
    );
    assert!(!destination.join("receipt.json").exists());
    assert!(!moved.join("candidates/receipt.json").exists());

    fs::remove_dir_all(source_root).expect("remove temporary stage");
    fs::remove_dir_all(parent).expect("remove replacement parent");
    fs::remove_dir_all(moved).expect("remove moved parent");
}
