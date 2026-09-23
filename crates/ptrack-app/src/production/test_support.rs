//! Test-only interleaving hooks. Production call sites run each hook at one
//! exact point of a transaction so a test can race it deterministically; this
//! module is compiled only into the test build.

std::thread_local! {
    static GUIDE_BEFORE_PUBLISH_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static GUIDE_BEFORE_COMMIT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static INITIALIZATION_BEFORE_COMMIT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static INITIALIZATION_AFTER_STARTED_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static STARTUP_INITIALIZATION_INFERENCE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static INITIALIZATION_AFTER_BOOTSTRAP_PLAN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

pub(crate) fn set_guide_before_publish_hook(hook: impl FnOnce() + 'static) {
    GUIDE_BEFORE_PUBLISH_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

pub(super) fn run_guide_before_publish_hook() {
    GUIDE_BEFORE_PUBLISH_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

pub(crate) fn set_guide_before_commit_hook(hook: impl FnOnce() + 'static) {
    GUIDE_BEFORE_COMMIT_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

pub(super) fn run_guide_before_commit_hook() {
    GUIDE_BEFORE_COMMIT_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

pub(crate) fn set_initialization_before_commit_hook(hook: impl FnOnce() + 'static) {
    INITIALIZATION_BEFORE_COMMIT_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

pub(super) fn run_initialization_before_commit_hook() {
    INITIALIZATION_BEFORE_COMMIT_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

pub(crate) fn set_initialization_after_started_hook(hook: impl FnOnce() + 'static) {
    INITIALIZATION_AFTER_STARTED_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

pub(super) fn run_initialization_after_started_hook() {
    INITIALIZATION_AFTER_STARTED_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

pub(crate) fn set_startup_initialization_inference_hook(hook: impl FnOnce() + 'static) {
    STARTUP_INITIALIZATION_INFERENCE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

pub(super) fn run_startup_initialization_inference_hook() {
    STARTUP_INITIALIZATION_INFERENCE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

pub(crate) fn set_initialization_after_bootstrap_plan_hook(hook: impl FnOnce() + 'static) {
    INITIALIZATION_AFTER_BOOTSTRAP_PLAN_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

pub(super) fn run_initialization_after_bootstrap_plan_hook() {
    INITIALIZATION_AFTER_BOOTSTRAP_PLAN_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}
