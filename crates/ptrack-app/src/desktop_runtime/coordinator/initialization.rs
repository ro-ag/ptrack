//! First-run project initialization through the coordinator: target
//! validation, guide previews, the fenced initialization transaction, and
//! binding a workspace once an initialization completes.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;

use super::super::command::InitializationCommand;
use super::super::support::{lock, value};
use super::super::wire::{
    DesktopEvent, InitializationCheckpointV1, InitializationOutcomeV1, InitializationStatusV1,
    InitializeProjectRequestV1, InitializeProjectResultV1, ProjectGuideChoiceV1, WorkspaceStatus,
};
use super::super::{FIRST_RUN_GOAL_MAX_BYTES, WORKSPACE_OPERATION_DRAIN_TIMEOUT};
use super::{CompletedInitializationReplay, DesktopRuntime, shutting_down, state_view};
use crate::{AppError, AppResult};

impl DesktopRuntime {
    pub(super) fn initialization_command(
        self: &Arc<Self>,
        command: InitializationCommand<'_>,
    ) -> AppResult<Value> {
        match command {
            InitializationCommand::Status { operation_id } => {
                self.initialization_status(operation_id)
            }
            InitializationCommand::Pending => self.pending_initialization(),
            InitializationCommand::Initialize { request } => {
                value(self.initialize_project(request)?)
            }
            InitializationCommand::PreviewGuide { request } => {
                let _lease = self.begin_native_action()?;
                value(self.initialization.preview_guide(&request)?)
            }
            InitializationCommand::ValidateTarget { selected } => {
                let _lease = self.begin_native_action()?;
                value(self.initialization.validate_target(&selected)?)
            }
        }
    }

    #[allow(clippy::too_many_lines)] // One fenced authority transaction owns every recovery edge.
    fn initialize_project(
        self: &Arc<Self>,
        mut request: InitializeProjectRequestV1,
    ) -> AppResult<InitializeProjectResultV1> {
        request.goal = request.goal.trim().to_owned();
        if request.goal.is_empty() || request.goal.len() > FIRST_RUN_GOAL_MAX_BYTES {
            return Err(AppError::Message(format!(
                "project goal must contain 1 to {FIRST_RUN_GOAL_MAX_BYTES} UTF-8 bytes"
            )));
        }
        match request.guide_choice {
            ProjectGuideChoiceV1::Skip if !request.guide_preview_token.is_empty() => {
                return Err(AppError::Message(
                    "skipping project guidance requires an empty preview token".to_owned(),
                ));
            }
            ProjectGuideChoiceV1::Skip | ProjectGuideChoiceV1::Install => {}
        }
        if let Some(replayed) = self.replay_completed_initialization(&request)? {
            return Ok(replayed);
        }
        {
            let mut state = lock(&self.state);
            if state.shutting_down {
                return Err(shutting_down());
            }
            if state.authority_changing {
                return Err(AppError::Message(
                    "runtime authority is changing".to_owned(),
                ));
            }
            if state.workspace.is_some() {
                if let Some(replay) = &state.completed_initialization
                    && replay.request == request
                    && replay.result.state == state_view(&state, &self.version)
                {
                    return Ok(replay.result.clone());
                }
                return Err(AppError::Message(
                    "project initialization requires no open workspace".to_owned(),
                ));
            }
            state.authority_changing = true;
            state.status = WorkspaceStatus::Loading;
            state.error.clear();
            let deadline = Instant::now() + WORKSPACE_OPERATION_DRAIN_TIMEOUT;
            while state.active_calls != 0 {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let (next, _) = self
                    .calls_changed
                    .wait_timeout(state, remaining)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state = next;
            }
            let refused = (state.active_calls != 0).then(|| {
                state.authority_changing = false;
                state.status = WorkspaceStatus::Error;
                "runtime calls did not finish before initialization".clone_into(&mut state.error);
                state.error.clone()
            });
            drop(state);
            if let Some(error) = refused {
                return Err(AppError::Message(error));
            }
        }
        let _transition = lock(&self.transition);

        let initialized = (|| -> AppResult<InitializeProjectResultV1> {
            // The drain above waited for calls admitted before the fence, and
            // an open admitted then may have published a workspace since the
            // check. Building over it would orphan its terminals and agent
            // server, so initialization refuses instead.
            if lock(&self.state).workspace.is_some() {
                return Err(AppError::Message(
                    "project initialization requires no open workspace".to_owned(),
                ));
            }
            let status = self.initialization.initialize(&request)?;
            if status.outcome == InitializationOutcomeV1::RecoveryRequired {
                let mut state = lock(&self.state);
                state.status = WorkspaceStatus::Error;
                state.error.clone_from(&status.error_kind);
                let view = state_view(&state, &self.version);
                drop(state);
                return Ok(InitializeProjectResultV1 {
                    initialization: status,
                    state: view,
                });
            }
            if !matches!(
                status.checkpoint,
                InitializationCheckpointV1::GuideApplied | InitializationCheckpointV1::DesktopBound
            ) {
                return Err(AppError::Message(
                    "project initialization did not commit the project".to_owned(),
                ));
            }
            let root = PathBuf::from(&status.canonical_root);
            let next_generation = lock(&self.state)
                .generation
                .checked_add(1)
                .ok_or_else(|| AppError::Message("workspace generation overflow".to_owned()))?;
            let workspace = self.factory.build(&root, next_generation)?;
            let project = workspace.project();
            let initialization = match self
                .initialization
                .mark_desktop_bound(&request.operation_id)
            {
                Ok(initialization) => initialization,
                Err(error) => {
                    let _ = workspace.shutdown();
                    return Err(error);
                }
            };
            {
                let mut state = lock(&self.state);
                state.workspace = Some(Arc::clone(&workspace));
                state.generation = next_generation;
                state.status = WorkspaceStatus::Open;
                state.error.clear();
            }
            self.start_workspace_watcher(
                next_generation,
                PathBuf::from(project.db_path),
                workspace,
            );
            let result = InitializeProjectResultV1 {
                initialization,
                state: self.workspace_state(),
            };
            lock(&self.state).completed_initialization = Some(CompletedInitializationReplay {
                request: request.clone(),
                result: result.clone(),
            });
            Ok(result)
        })();

        let mut state = lock(&self.state);
        state.authority_changing = false;
        if let Err(error) = &initialized {
            if state.workspace.is_none() {
                state.status = WorkspaceStatus::Error;
                state.error = error.to_string();
            } else {
                // A workspace published during the drain stays the open one.
                state.status = WorkspaceStatus::Open;
            }
        }
        drop(state);
        self.calls_changed.notify_all();
        if let Ok(result) = &initialized {
            self.emit(DesktopEvent::WorkspaceDataChanged(result.state.generation));
        }
        initialized
    }

    fn replay_completed_initialization(
        self: &Arc<Self>,
        request: &InitializeProjectRequestV1,
    ) -> AppResult<Option<InitializeProjectResultV1>> {
        if lock(&self.state).workspace.is_none() {
            return Ok(None);
        }
        let _lease = self.begin_native_action()?;
        let _transition = lock(&self.transition);
        {
            let state = lock(&self.state);
            if state.workspace.is_none() {
                return Ok(None);
            }
            if let Some(replay) = &state.completed_initialization
                && replay.request == *request
                && replay.result.state == state_view(&state, &self.version)
            {
                return Ok(Some(replay.result.clone()));
            }
        }
        let initialization = self.initialization.initialize(request)?;
        let mut state = lock(&self.state);
        if state.shutting_down {
            return Err(shutting_down());
        }
        if state.authority_changing {
            return Err(AppError::Message(
                "runtime authority is changing".to_owned(),
            ));
        }
        let current = state_view(&state, &self.version);
        if initialization.checkpoint != InitializationCheckpointV1::DesktopBound
            || initialization.outcome != InitializationOutcomeV1::Complete
            || initialization.canonical_root != request.root
            || current.status != WorkspaceStatus::Open
            || current
                .project
                .as_ref()
                .is_none_or(|project| project.root != request.root)
        {
            return Err(AppError::Message(
                "project initialization requires no open workspace".to_owned(),
            ));
        }
        let result = InitializeProjectResultV1 {
            initialization,
            state: current,
        };
        state.completed_initialization = Some(CompletedInitializationReplay {
            request: request.clone(),
            result: result.clone(),
        });
        drop(state);
        Ok(Some(result))
    }

    fn pending_initialization(self: &Arc<Self>) -> AppResult<Value> {
        let _lease = self.begin_native_action()?;
        let _transition = lock(&self.transition);
        // Startup discovery must not reopen a project from an old completed
        // initialization journal. Explicit operation-status polling below
        // still binds a just-completed initialization during recovery.
        value(self.initialization.pending()?)
    }

    fn initialization_status(self: &Arc<Self>, operation_id: &str) -> AppResult<Value> {
        let _lease = self.begin_native_action()?;
        let _transition = lock(&self.transition);
        let status = self.initialization.status(operation_id)?;
        self.bind_completed_initialization_locked(&status)?;
        value(status)
    }

    fn bind_completed_initialization_locked(
        self: &Arc<Self>,
        status: &InitializationStatusV1,
    ) -> AppResult<()> {
        if status.checkpoint != InitializationCheckpointV1::DesktopBound
            || status.outcome != InitializationOutcomeV1::Complete
        {
            return Ok(());
        }
        self.require_not_shutting_down()?;
        let next_generation = {
            let mut state = lock(&self.state);
            if state.workspace.is_some() {
                return Ok(());
            }
            let next_generation = state
                .generation
                .checked_add(1)
                .ok_or_else(|| AppError::Message("workspace generation overflow".to_owned()))?;
            state.status = WorkspaceStatus::Loading;
            state.error.clear();
            next_generation
        };
        let root = PathBuf::from(&status.canonical_root);
        let workspace = match self.factory.build(&root, next_generation) {
            Ok(workspace) => workspace,
            Err(error) => {
                let mut state = lock(&self.state);
                state.status = WorkspaceStatus::Error;
                state.error = error.to_string();
                drop(state);
                return Err(error);
            }
        };
        let project = workspace.project();
        {
            let mut state = lock(&self.state);
            state.workspace = Some(Arc::clone(&workspace));
            state.generation = next_generation;
            state.status = WorkspaceStatus::Open;
            state.error.clear();
        }
        self.start_workspace_watcher(next_generation, PathBuf::from(project.db_path), workspace);
        self.emit(DesktopEvent::WorkspaceDataChanged(next_generation));
        Ok(())
    }
}
