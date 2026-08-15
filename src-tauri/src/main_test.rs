use std::path::PathBuf;

use super::{
    ProjectPickerPurpose, WindowStateCapture, project_picker_result, validate_external_url,
};

#[test]
fn project_picker_purpose_is_strict_and_owns_native_titles() {
    let initialize = ProjectPickerPurpose::parse("initialize").unwrap();
    let locate = ProjectPickerPurpose::parse("locate-recent-project").unwrap();
    let open = ProjectPickerPurpose::parse("open").unwrap();

    assert_eq!(initialize.title(), "Initialize p-track Project");
    assert_eq!(locate.title(), "Locate p-track Project");
    assert_eq!(open.title(), "Open p-track Project");
    assert!(ProjectPickerPurpose::parse("").is_err());
    assert!(ProjectPickerPurpose::parse("Open").is_err());
    assert!(ProjectPickerPurpose::parse("other").is_err());
}

#[test]
fn project_picker_cancellation_is_an_exact_no_selection_result() {
    assert_eq!(project_picker_result(None).unwrap(), "");
    assert_eq!(
        project_picker_result(Some(tauri_plugin_dialog::FilePath::Path(PathBuf::from(
            "selected-project"
        ),)))
        .unwrap(),
        "selected-project"
    );
}

/// Non-terminal captures run on their own thread, so one can wake up after the
/// exit flush has already written the rect the window really closed at. That
/// late write must be dropped, not ordered behind the good one.
#[test]
fn a_capture_that_wakes_after_the_exit_flush_never_writes() {
    let capture = WindowStateCapture::new();

    // Ordinary trailing captures write and leave the gate open.
    assert!(capture.guarded(false, || {}));
    assert!(capture.guarded(false, || {}));
    // The exit flush writes and seals.
    assert!(capture.guarded(true, || {}));
    // Anything arriving afterwards, exit flush included, is refused.
    assert!(!capture.guarded(false, || {}));
    assert!(!capture.guarded(true, || {}));
}

#[test]
fn external_url_gate_remains_available_to_the_native_shell_tests() {
    assert!(validate_external_url("https://example.com/help").is_ok());
    assert!(validate_external_url("file:///tmp/help").is_err());
}
