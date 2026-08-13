use std::sync::Mutex;

use super::private_windows::{SuspendedSpawnApi, contain_suspended};

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
