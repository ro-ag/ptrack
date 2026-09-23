//! Admission gates shared by the coordinator and the bound workspace: the
//! resource-admission gate that fences new terminals and agent runs, and the
//! workspace-call gate a workspace drains before it shuts down.

use std::sync::{Arc, Condvar, Mutex};

use super::support::lock;

pub(super) struct ResourceAdmissionState {
    pub(super) fences: usize,
    pub(super) pending: usize,
    pub(super) revision: u64,
}

pub(super) struct ResourceAdmissionGate {
    pub(super) state: Mutex<ResourceAdmissionState>,
}

pub(crate) struct ResourceAdmissionLease(pub(super) Arc<ResourceAdmissionGate>);

impl Drop for ResourceAdmissionLease {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.pending = state.pending.saturating_sub(1);
        state.revision = state.revision.saturating_add(1);
    }
}

pub(super) struct WorkspaceCallState {
    pub(super) closing: bool,
    pub(super) active: usize,
}

pub(super) struct WorkspaceCallGate {
    pub(super) state: Mutex<WorkspaceCallState>,
    pub(super) idle: Condvar,
}

pub(crate) struct WorkspaceCallLease(pub(super) Arc<WorkspaceCallGate>);

impl Drop for WorkspaceCallLease {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.active = state.active.saturating_sub(1);
        if state.active == 0 {
            self.0.idle.notify_all();
        }
    }
}

pub(super) struct ResourceAdmissionFence(pub(super) Arc<ResourceAdmissionGate>);

impl Drop for ResourceAdmissionFence {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.fences = state.fences.saturating_sub(1);
    }
}

/// Opaque coordinator-owned fence that blocks both terminal and `AgentRun`
/// admission for the lifetime of a workspace confirmation.
pub struct DesktopAdmissionFence {
    pub(super) _resource: Option<ResourceAdmissionFence>,
    pub(super) _agent: Option<crate::AgentAdmissionFence>,
}

impl DesktopAdmissionFence {
    pub(super) const fn empty() -> Self {
        Self {
            _resource: None,
            _agent: None,
        }
    }
}
