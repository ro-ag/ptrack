use std::collections::{BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::sync::{Mutex, MutexGuard};

use ptrack_app::{
    DesktopNotificationEventV1, DesktopNotificationKindV1, DesktopNotificationSnapshotV1,
    DesktopRuntime,
};
use serde_json::{Value, json};
use tauri::plugin::PermissionState;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_notification::NotificationExt as _;

const SEEN_EVENT_LIMIT: usize = 4_096;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct NotificationOptIns {
    pub handoff_arrival: bool,
    pub run_failure_or_drift: bool,
    pub run_completion: bool,
}

impl NotificationOptIns {
    pub(crate) fn from_preferences(value: &Value) -> Self {
        let section = &value["notifications"];
        Self {
            handoff_arrival: section["handoffArrival"].as_bool().unwrap_or(false),
            run_failure_or_drift: section["runFailureOrDrift"].as_bool().unwrap_or(false),
            run_completion: section["runCompletion"].as_bool().unwrap_or(false),
        }
    }

    const fn any(self) -> bool {
        self.handoff_arrival || self.run_failure_or_drift || self.run_completion
    }

    const fn enabled(self, kind: DesktopNotificationKindV1) -> bool {
        match kind {
            DesktopNotificationKindV1::HandoffArrival => self.handoff_arrival,
            DesktopNotificationKindV1::RunFailure | DesktopNotificationKindV1::RunDrift => {
                self.run_failure_or_drift
            }
            DesktopNotificationKindV1::RunCompletion => self.run_completion,
        }
    }
}

#[derive(Default)]
struct SeenEvents {
    ids: BTreeSet<String>,
    order: VecDeque<String>,
}

impl SeenEvents {
    fn insert(&mut self, id: &str) -> bool {
        if !self.ids.insert(id.to_owned()) {
            return false;
        }
        self.order.push_back(id.to_owned());
        while self.order.len() > SEEN_EVENT_LIMIT {
            if let Some(expired) = self.order.pop_front() {
                self.ids.remove(&expired);
            }
        }
        true
    }

    fn clear(&mut self) {
        self.ids.clear();
        self.order.clear();
    }
}

#[derive(Default)]
pub(crate) struct NotificationPolicy {
    opt_ins: NotificationOptIns,
    permission_granted: bool,
    generation: Option<u64>,
    baselines: BTreeSet<DesktopNotificationKindV1>,
    seen: SeenEvents,
}

impl NotificationPolicy {
    pub(crate) fn configure(&mut self, opt_ins: NotificationOptIns, permission_granted: bool) {
        if !self.opt_ins.handoff_arrival && opt_ins.handoff_arrival {
            self.baselines
                .insert(DesktopNotificationKindV1::HandoffArrival);
        }
        if !self.opt_ins.run_failure_or_drift && opt_ins.run_failure_or_drift {
            self.baselines.insert(DesktopNotificationKindV1::RunFailure);
            self.baselines.insert(DesktopNotificationKindV1::RunDrift);
        }
        if !self.opt_ins.run_completion && opt_ins.run_completion {
            self.baselines
                .insert(DesktopNotificationKindV1::RunCompletion);
        }
        self.opt_ins = opt_ins;
        self.permission_granted = permission_granted;
    }

    pub(crate) fn observe(
        &mut self,
        snapshot: &DesktopNotificationSnapshotV1,
        foreground: bool,
    ) -> Vec<DesktopNotificationEventV1> {
        if self.generation != Some(snapshot.generation) {
            self.generation = Some(snapshot.generation);
            self.seen.clear();
            for event in &snapshot.events {
                self.seen.insert(&event.id);
            }
            self.baselines.clear();
            return Vec::new();
        }

        let mut alerts = Vec::new();
        for event in &snapshot.events {
            let baseline = self.baselines.contains(&event.kind);
            let is_new = self.seen.insert(&event.id);
            if is_new
                && !baseline
                && self.opt_ins.enabled(event.kind)
                && self.permission_granted
                && !foreground
            {
                alerts.push(event.clone());
            }
        }
        self.baselines.clear();
        alerts
    }
}

#[derive(Default)]
struct NotificationState {
    policy: NotificationPolicy,
    focused_windows: BTreeSet<String>,
}

#[derive(Default)]
pub(crate) struct NativeNotificationController {
    state: Mutex<NotificationState>,
    configuration: Mutex<()>,
}

impl NativeNotificationController {
    pub(crate) fn with_configuration<T>(&self, action: impl FnOnce() -> T) -> T {
        let _guard = lock(&self.configuration);
        action()
    }

    pub(crate) fn initialize_focus<R: Runtime>(&self, app: &AppHandle<R>) {
        let mut state = lock(&self.state);
        state.focused_windows.clear();
        for (label, window) in app.webview_windows() {
            if window.is_focused().unwrap_or(false) {
                state.focused_windows.insert(label);
            }
        }
    }

    pub(crate) fn set_window_focus(&self, label: &str, focused: bool) {
        let mut state = lock(&self.state);
        if focused {
            state.focused_windows.insert(label.to_owned());
        } else {
            state.focused_windows.remove(label);
        }
    }

    pub(crate) fn remove_window(&self, label: &str) {
        lock(&self.state).focused_windows.remove(label);
    }

    pub(crate) fn foreground(&self) -> bool {
        !lock(&self.state).focused_windows.is_empty()
    }

    /// Applies stored opt-ins and returns `true` when an enabled preference
    /// has no usable OS permission. Only an explicit UI enable requests it.
    pub(crate) fn configure<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        preferences: &Value,
        request_on_enable: bool,
    ) -> bool {
        let opt_ins = NotificationOptIns::from_preferences(preferences);
        let previous = lock(&self.state).policy.opt_ins;
        let newly_enabled = (!previous.handoff_arrival && opt_ins.handoff_arrival)
            || (!previous.run_failure_or_drift && opt_ins.run_failure_or_drift)
            || (!previous.run_completion && opt_ins.run_completion);
        let permission = if !opt_ins.any() {
            false
        } else if request_on_enable && newly_enabled {
            app.notification()
                .request_permission()
                .is_ok_and(|state| state == PermissionState::Granted)
        } else {
            app.notification()
                .permission_state()
                .is_ok_and(|state| state == PermissionState::Granted)
        };
        lock(&self.state).policy.configure(opt_ins, permission);
        opt_ins.any() && !permission
    }

    pub(crate) fn refresh<R: Runtime>(&self, app: &AppHandle<R>) {
        let Some(runtime) = app.try_state::<std::sync::Arc<DesktopRuntime>>() else {
            return;
        };
        let Ok(snapshot) = runtime.notification_snapshot() else {
            return;
        };
        let foreground = self.foreground();
        let alerts = {
            let mut state = lock(&self.state);
            state.policy.observe(&snapshot, foreground)
        };
        for event in alerts {
            let may_deliver = {
                let state = lock(&self.state);
                state.focused_windows.is_empty()
                    && state.policy.permission_granted
                    && state.policy.opt_ins.enabled(event.kind)
            };
            if !may_deliver {
                continue;
            }
            let _ = app
                .notification()
                .builder()
                .title("p-track")
                .body(notification_body(&event))
                .show();
        }
    }
}

pub(crate) fn disabled_notification_patch() -> Value {
    json!({
        "notifications": {
            "handoffArrival": false,
            "runFailureOrDrift": false,
            "runCompletion": false
        }
    })
}

pub(crate) fn notification_body(event: &DesktopNotificationEventV1) -> String {
    let label = match event.kind {
        DesktopNotificationKindV1::HandoffArrival => "Handoff arrived",
        DesktopNotificationKindV1::RunFailure => "Run failed",
        DesktopNotificationKindV1::RunDrift => "Possible task drift",
        DesktopNotificationKindV1::RunCompletion => "Run completed",
    };
    let mut body = format!("{label} · agent {}", short_id(&event.run_id));
    if event.plan_id != 0 {
        let _ = write!(body, " · plan #{}", event.plan_id);
    }
    if event.task_id != 0 {
        let _ = write!(body, " · task #{}", event.task_id);
    }
    body
}

fn short_id(value: &str) -> String {
    value.chars().take(8).collect()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
