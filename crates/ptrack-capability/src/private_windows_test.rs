use std::fs;
use std::sync::Mutex;

use std::path::Path;

use super::private_windows::{
    SuspendedSpawnApi, active_interface_names, contain_suspended, install_download,
    protect_private_path, rename_buffer_len,
};
use super::test_support::TempDir;

#[test]
fn windows_private_transfer_and_diagnostic_api_surface_typechecks() {
    let protect: fn(&Path) -> Result<(), ()> = protect_private_path;
    let install: fn(&Path, &Path, &Path, i64) -> Result<(), &'static str> = install_download;
    let interfaces: fn() -> Result<Vec<String>, ()> = active_interface_names;
    let _ = (protect, install, interfaces);
}

#[test]
fn one_character_download_destination_uses_complete_rename_header() {
    assert_eq!(
        rename_buffer_len(2).unwrap(),
        std::mem::size_of::<super::private_windows::RenameLayout>()
    );

    let temp = TempDir::new("windows-one-character-download");
    let staged = temp.path().join("staged");
    let destination = temp.path().join("a");
    fs::write(&staged, b"payload").unwrap();

    install_download(temp.path(), &destination, &staged, 7).unwrap();

    assert_eq!(fs::read(destination).unwrap(), b"payload");
}

#[test]
fn suspended_windows_child_is_assigned_before_resume() {
    let api = OrderedApi::default();
    contain_suspended(&api).unwrap();
    assert_eq!(
        *api.steps.lock().unwrap(),
        ["create-kill-job", "assign-suspended", "resume-primary"]
    );
}

#[test]
fn suspended_windows_child_fails_closed_without_resume_after_assign_failure() {
    let api = OrderedApi {
        fail_assign: true,
        ..OrderedApi::default()
    };
    assert!(contain_suspended(&api).is_err());
    assert_eq!(
        *api.steps.lock().unwrap(),
        ["create-kill-job", "assign-suspended"]
    );
}

#[derive(Default)]
struct OrderedApi {
    steps: Mutex<Vec<&'static str>>,
    fail_assign: bool,
}

impl SuspendedSpawnApi for OrderedApi {
    type Job = ();

    fn create_kill_on_close_job(&self) -> Result<Self::Job, ()> {
        self.steps.lock().unwrap().push("create-kill-job");
        Ok(())
    }

    fn assign_suspended_process(&self, _job: &Self::Job) -> Result<(), ()> {
        self.steps.lock().unwrap().push("assign-suspended");
        if self.fail_assign { Err(()) } else { Ok(()) }
    }

    fn resume_primary_thread(&self) -> Result<(), ()> {
        self.steps.lock().unwrap().push("resume-primary");
        Ok(())
    }
}
