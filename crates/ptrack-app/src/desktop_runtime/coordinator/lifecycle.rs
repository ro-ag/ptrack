//! Opening, closing, and switching the project workspace, including the
//! confirmation a change needs while terminals or agent runs are live.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::thread;
use std::time::Instant;

use serde_json::{Value, json};

use super::super::admission::DesktopAdmissionFence;
use super::super::command::LifecycleCommand;
use super::super::support::{lock, random_token, value};
use super::super::wire::{
    ActiveResourceSummary, DesktopEvent, WorkspaceChangeResult, WorkspaceStatus,
};
use super::application::record_last_project_in;
use super::{Confirmation, ConfirmationAction, DesktopRuntime, shutting_down, state_view};
use crate::{AppError, AppResult};

impl DesktopRuntime {
    pub(super) fn lifecycle_command(
        self: &Arc<Self>,
        command: LifecycleCommand<'_>,
    ) -> AppResult<Value> {
        match command {
            LifecycleCommand::GetState => value(self.workspace_state()),
            LifecycleCommand::Open { root, token } => {
                let _lease = self.begin_native_action()?;
                value(self.open_project(&root, token)?)
            }
            LifecycleCommand::Close { token } => {
                let _lease = self.begin_native_action()?;
                value(self.close_project(token)?)
            }
            LifecycleCommand::CancelChange { token } => {
                let _lease = self.begin_native_action()?;
                self.cancel_workspace_change(token)?;
                Ok(Value::Null)
            }
        }
    }

    pub(super) fn open_project(
        self: &Arc<Self>,
        root: &Path,
        token: &str,
    ) -> AppResult<WorkspaceChangeResult> {
        let _transition = lock(&self.transition);
        self.require_not_shutting_down()?;
        // An initialization fences the authority before it drains, and an open
        // admitted just before that fence must not publish a workspace the
        // initialization is about to replace.
        if lock(&self.state).authority_changing {
            return Err(AppError::Message(
                "runtime authority is changing".to_owned(),
            ));
        }
        let canonical = fs::canonicalize(root).map_err(AppError::Io)?;
        if !canonical.is_dir() {
            return Err(AppError::Message(
                "selected project path is not a directory".to_owned(),
            ));
        }
        let (old, generation) = {
            let state = lock(&self.state);
            (state.workspace.clone(), state.generation)
        };
        let admission = old
            .as_ref()
            .map(|host| host.fence_resource_admission())
            .transpose()?
            .unwrap_or_else(DesktopAdmissionFence::empty);
        let active = old
            .as_ref()
            .map_or(Ok(ActiveResourceSummary::default()), |host| {
                host.active_resources()
            })?;
        if active.requires_confirmation()
            && !self.confirmed(ConfirmationAction::Open, &canonical, token, active)?
        {
            return self.challenge(ConfirmationAction::Open, canonical, active, admission);
        }
        let next_generation = generation
            .checked_add(1)
            .ok_or_else(|| AppError::Message("workspace generation overflow".to_owned()))?;
        {
            let mut state = lock(&self.state);
            state.status = WorkspaceStatus::Loading;
            state.error.clear();
        }
        let candidate = match self.factory.build(&canonical, next_generation) {
            Ok(candidate) => candidate,
            Err(error) => {
                let mut state = lock(&self.state);
                state.status = if old.is_some() {
                    WorkspaceStatus::Open
                } else {
                    WorkspaceStatus::Error
                };
                if old.is_none() {
                    state.error = error.to_string();
                    drop(state);
                }
                return Err(error);
            }
        };
        let candidate_project = candidate.project();
        let watcher_workspace = candidate.clone();
        {
            let mut state = lock(&self.state);
            state.workspace = Some(candidate);
            state.generation = next_generation;
            state.status = WorkspaceStatus::Open;
            state.error.clear();
            state.confirmation = None;
        }
        self.start_workspace_watcher(
            next_generation,
            PathBuf::from(candidate_project.db_path),
            watcher_workspace,
        );
        let warning = old
            .and_then(|workspace| workspace.shutdown().err())
            .map_or_else(String::new, |error| {
                format!("previous project cleanup incomplete: {error}")
            });
        self.emit(DesktopEvent::WorkspaceDataChanged(next_generation));
        self.record_last_project(&json!(canonical.to_str()));
        Ok(WorkspaceChangeResult {
            state: self.workspace_state(),
            requires_confirmation: false,
            confirmation_token: String::new(),
            active_resources: ActiveResourceSummary::default(),
            warning,
        })
    }

    /// Records, or with a null root clears, the project startup may reopen.
    /// Best effort, because a global store that will not open must never fail
    /// the project change the user actually asked for.
    fn record_last_project(&self, root: &Value) {
        if let Ok(store) = self.global_store() {
            record_last_project_in(&store, root);
        }
    }

    /// The root of the project open right now, if any.
    pub(super) fn open_project_root(&self) -> Option<String> {
        lock(&self.state)
            .workspace
            .as_ref()
            .map(|workspace| workspace.project().root)
    }

    fn close_project(self: &Arc<Self>, token: &str) -> AppResult<WorkspaceChangeResult> {
        let _transition = lock(&self.transition);
        self.require_not_shutting_down()?;
        let workspace = {
            let state = lock(&self.state);
            state.workspace.clone()
        };
        let Some(workspace) = workspace else {
            return Ok(WorkspaceChangeResult {
                state: self.workspace_state(),
                requires_confirmation: false,
                confirmation_token: String::new(),
                active_resources: ActiveResourceSummary::default(),
                warning: String::new(),
            });
        };
        let admission = workspace.fence_resource_admission()?;
        let active = workspace.active_resources()?;
        if active.requires_confirmation()
            && !self.confirmed(ConfirmationAction::Close, Path::new(""), token, active)?
        {
            return self.challenge(ConfirmationAction::Close, PathBuf::new(), active, admission);
        }
        {
            let mut state = lock(&self.state);
            state.status = WorkspaceStatus::Loading;
            state.confirmation = None;
            state.workspace = None;
        }
        self.stop_workspace_watcher();
        let warning = workspace
            .shutdown()
            .err()
            .map_or_else(String::new, |error| {
                format!("project cleanup incomplete: {error}")
            });
        let closed_state = {
            let mut state = lock(&self.state);
            state.status = WorkspaceStatus::Closed;
            let view = state_view(&state, &self.version);
            drop(state);
            view
        };
        let result = WorkspaceChangeResult {
            state: closed_state,
            requires_confirmation: false,
            confirmation_token: String::new(),
            active_resources: ActiveResourceSummary::default(),
            warning,
        };
        lock(&self.state).status = WorkspaceStatus::Welcome;
        // An explicit close is the user saying they do not want this project
        // back on the next launch.
        self.record_last_project(&Value::Null);
        Ok(result)
    }

    fn challenge(
        self: &Arc<Self>,
        action: ConfirmationAction,
        path: PathBuf,
        active: ActiveResourceSummary,
        admission: DesktopAdmissionFence,
    ) -> AppResult<WorkspaceChangeResult> {
        let token = random_token()?;
        let (expiry_cancel, expiry_cancellation) = channel();
        {
            let mut state = lock(&self.state);
            let generation = state.generation;
            state.confirmation = Some(Confirmation {
                token: token.clone(),
                action,
                path,
                generation,
                resource_revision: active.resource_revision,
                resources: active,
                expires_at: Instant::now() + self.confirmation_ttl,
                _expiry_cancellation: expiry_cancel,
                _admission: admission,
            });
        }
        let weak = Arc::downgrade(self);
        let expiry_token = token.clone();
        let confirmation_ttl = self.confirmation_ttl;
        let _ = thread::Builder::new()
            .name("ptrack-workspace-confirmation".to_owned())
            .spawn(move || {
                if expiry_cancellation.recv_timeout(confirmation_ttl).is_ok() {
                    return;
                }
                let Some(runtime) = weak.upgrade() else {
                    return;
                };
                let mut state = lock(&runtime.state);
                if state
                    .confirmation
                    .as_ref()
                    .is_some_and(|confirmation| confirmation.token == expiry_token)
                {
                    state.confirmation = None;
                }
            });
        Ok(WorkspaceChangeResult {
            state: self.workspace_state(),
            requires_confirmation: true,
            confirmation_token: token,
            active_resources: active,
            warning: String::new(),
        })
    }

    fn confirmed(
        &self,
        action: ConfirmationAction,
        path: &Path,
        token: &str,
        active: ActiveResourceSummary,
    ) -> AppResult<bool> {
        if token.is_empty() {
            return Ok(false);
        }
        let mut state = lock(&self.state);
        let valid = state.confirmation.as_ref().is_some_and(|confirmation| {
            confirmation.token == token
                && confirmation.action == action
                && confirmation.path == path
                && confirmation.generation == state.generation
                && confirmation.resource_revision == active.resource_revision
                && confirmation.resources == active
                && Instant::now() <= confirmation.expires_at
        });
        state.confirmation = None;
        drop(state);
        if valid {
            Ok(true)
        } else {
            Err(AppError::Message(
                "invalid or expired workspace confirmation".to_owned(),
            ))
        }
    }

    fn cancel_workspace_change(&self, token: &str) -> AppResult<()> {
        let mut state = lock(&self.state);
        if state.shutting_down {
            return Err(shutting_down());
        }
        let valid = state
            .confirmation
            .as_ref()
            .is_some_and(|confirmation| confirmation.token == token);
        state.confirmation = None;
        drop(state);
        if valid {
            Ok(())
        } else {
            Err(AppError::Message(
                "invalid or expired workspace confirmation".to_owned(),
            ))
        }
    }

    pub(super) fn cancel_workspace_change_if_exact(&self, token: &str) {
        let mut state = lock(&self.state);
        if state
            .confirmation
            .as_ref()
            .is_some_and(|confirmation| confirmation.token == token)
        {
            state.confirmation = None;
        }
    }
}
