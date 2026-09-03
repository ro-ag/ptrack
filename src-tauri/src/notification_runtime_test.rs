use ptrack_app::{
    DesktopNotificationEventV1, DesktopNotificationKindV1, DesktopNotificationSnapshotV1,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use super::notification_runtime::{NativeNotificationController, disabled_notification_patch};
use super::notification_runtime::{NotificationOptIns, NotificationPolicy, notification_body};

fn event(id: &str, kind: DesktopNotificationKindV1) -> DesktopNotificationEventV1 {
    DesktopNotificationEventV1 {
        id: id.to_owned(),
        kind,
        run_id: "12345678-secret-suffix".to_owned(),
        plan_id: 26,
        task_id: 212,
    }
}

fn snapshot(
    generation: u64,
    events: Vec<DesktopNotificationEventV1>,
) -> DesktopNotificationSnapshotV1 {
    DesktopNotificationSnapshotV1 { generation, events }
}

#[test]
fn startup_generation_and_new_opt_in_baseline_retained_events() {
    let mut policy = NotificationPolicy::default();
    policy.configure(
        NotificationOptIns {
            handoff_arrival: true,
            ..NotificationOptIns::default()
        },
        true,
    );
    let retained = event("handoff:old", DesktopNotificationKindV1::HandoffArrival);
    assert!(
        policy
            .observe(&snapshot(1, vec![retained.clone()]), false)
            .is_empty()
    );
    assert!(
        policy
            .observe(&snapshot(1, vec![retained]), false)
            .is_empty()
    );

    policy.configure(
        NotificationOptIns {
            handoff_arrival: true,
            run_completion: true,
            ..NotificationOptIns::default()
        },
        true,
    );
    let completed = event("run:old", DesktopNotificationKindV1::RunCompletion);
    assert!(
        policy
            .observe(&snapshot(1, vec![completed.clone()]), false)
            .is_empty()
    );
    assert!(
        policy
            .observe(&snapshot(1, vec![completed]), false)
            .is_empty()
    );
}

#[test]
fn background_delivery_is_opt_in_permission_and_stable_id_gated() {
    let mut policy = NotificationPolicy::default();
    let opt_ins = NotificationOptIns {
        handoff_arrival: true,
        run_failure_or_drift: true,
        run_completion: true,
    };
    policy.configure(opt_ins, true);
    assert!(policy.observe(&snapshot(7, Vec::new()), false).is_empty());

    let failure = event("run:failure:1", DesktopNotificationKindV1::RunFailure);
    assert!(
        policy
            .observe(&snapshot(7, vec![failure.clone()]), true)
            .is_empty()
    );
    assert!(
        policy
            .observe(&snapshot(7, vec![failure]), false)
            .is_empty()
    );

    let drift = event("run:drift:1", DesktopNotificationKindV1::RunDrift);
    assert_eq!(
        policy.observe(&snapshot(7, vec![drift.clone()]), false),
        vec![drift.clone()]
    );
    assert!(policy.observe(&snapshot(7, vec![drift]), false).is_empty());

    policy.configure(opt_ins, false);
    let completion = event("run:completion:1", DesktopNotificationKindV1::RunCompletion);
    assert!(
        policy
            .observe(&snapshot(7, vec![completion]), false)
            .is_empty()
    );
}

#[test]
fn project_generation_change_is_never_replayed() {
    let mut policy = NotificationPolicy::default();
    policy.configure(
        NotificationOptIns {
            run_completion: true,
            ..NotificationOptIns::default()
        },
        true,
    );
    assert!(policy.observe(&snapshot(1, Vec::new()), false).is_empty());
    let completed = event("run:complete", DesktopNotificationKindV1::RunCompletion);
    assert_eq!(
        policy.observe(&snapshot(1, vec![completed.clone()]), false),
        vec![completed.clone()]
    );
    assert!(
        policy
            .observe(&snapshot(2, vec![completed]), false)
            .is_empty()
    );
}

#[test]
fn copy_contains_only_bounded_identifiers() {
    let handoff = event("opaque-event-id", DesktopNotificationKindV1::HandoffArrival);
    let body = notification_body(&handoff);
    assert_eq!(
        body,
        "Handoff arrived · agent 12345678 · plan #26 · task #212"
    );
    assert!(!body.contains("secret-suffix"));
    assert!(!body.contains("opaque-event-id"));
}

#[test]
fn any_focused_ptrack_window_suppresses_delivery() {
    let controller = NativeNotificationController::default();
    assert!(!controller.foreground());
    controller.set_window_focus("main", true);
    controller.set_window_focus("terminal-1", false);
    assert!(controller.foreground());
    controller.remove_window("main");
    assert!(!controller.foreground());
    controller.set_window_focus("terminal-1", true);
    assert!(controller.foreground());
    controller.set_window_focus("terminal-1", false);
    assert!(!controller.foreground());
}

#[test]
fn permission_denial_patch_disables_exactly_all_three_categories() {
    assert_eq!(
        disabled_notification_patch(),
        serde_json::json!({
            "notifications": {
                "handoffArrival": false,
                "runFailureOrDrift": false,
                "runCompletion": false
            }
        })
    );
}

#[test]
fn preference_permission_transactions_are_serialized() {
    let controller = Arc::new(NativeNotificationController::default());
    let barrier = Arc::new(Barrier::new(3));
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));

    std::thread::scope(|scope| {
        for _ in 0..2 {
            let controller = Arc::clone(&controller);
            let barrier = Arc::clone(&barrier);
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            scope.spawn(move || {
                barrier.wait();
                controller.with_configuration(|| {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(current, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(10));
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            });
        }
        barrier.wait();
    });

    assert_eq!(maximum.load(Ordering::SeqCst), 1);
}
