//! The first-run initialization service: readying an operation, binding its
//! guide choice, quiescing the authority for the exclusive commit, and the
//! durable status every step records.

use std::path::Path;
use std::sync::Arc;

use ptrack_store::ProjectStore;

use super::super::bootstrap::{
    selected_project_directory_present, selected_project_storage_present,
};
use super::super::guide::{DesktopGuideManifest, validate_guide_before_commit};
use super::super::journal::{
    DesktopInitializationJournal, initialization_error_kind, publish_desktop_initialization,
    publish_desktop_initialization_locked, read_desktop_initialization,
    validate_desktop_initialization_transition, with_desktop_initialization_lock,
};
use super::super::runtime::ActiveRuntime;
use super::super::{
    BOOTSTRAP_PLAN, DESKTOP_INITIALIZATION, GUIDE_PREVIEW_STALE, lock, path_is_present, recovery,
    validate_operation_id,
};
use super::ProductionDesktopAuthority;
use crate::{
    AppError, AppResult, DesktopInitializationService, InitializationCheckpointV1,
    InitializationOutcomeV1, InitializationStatusV1, InitializeProjectRequestV1,
    PendingInitializationV1, ProjectGuideChoiceV1, ProjectGuidePreviewRequestV1,
    ProjectGuidePreviewV1, ProjectTargetKindV1, ProjectTargetValidationV1,
    UnavailableUpdateService,
};

impl DesktopInitializationService for ProductionDesktopAuthority {
    fn validate_target(&self, selected: &Path) -> AppResult<ProjectTargetValidationV1> {
        let validation = self.validate_target_inner(selected)?;
        if validation.kind == ProjectTargetKindV1::New {
            let mut state = lock(&self.state);
            if state
                .initialization
                .as_ref()
                .is_none_or(|status| status.operation_id != validation.operation_id)
            {
                state.initialization = Some(InitializationStatusV1 {
                    operation_id: validation.operation_id.clone(),
                    canonical_root: validation.canonical_root.clone(),
                    checkpoint: InitializationCheckpointV1::None,
                    outcome: InitializationOutcomeV1::Ready,
                    error_kind: String::new(),
                });
                state.initialization_goal = None;
                // The guide manifest is bound to one operation. Leaving the
                // previous project's manifest in place made the next
                // initialization refuse its own guide choice as stale.
                state.initialization_guide = None;
            }
        }
        Ok(validation)
    }

    fn preview_guide(
        &self,
        request: &ProjectGuidePreviewRequestV1,
    ) -> AppResult<ProjectGuidePreviewV1> {
        #[cfg(unix)]
        {
            self.preview_guide_inner(request)
        }
        #[cfg(not(unix))]
        {
            let _ = request;
            Ok(Self::guide_unavailable())
        }
    }

    fn initialize(
        &self,
        request: &InitializeProjectRequestV1,
    ) -> AppResult<InitializationStatusV1> {
        validate_operation_id(&request.operation_id)?;
        let (ready, bound_guide) = self.bound_initialization(request)?;
        if ready.checkpoint == InitializationCheckpointV1::DesktopBound {
            return replay_bound_initialization(request, ready, bound_guide);
        }
        if ready.checkpoint != InitializationCheckpointV1::GuideApplied {
            validate_guide_request(request)?;
        }
        let validation = self.validated_initialization_target(request, &ready)?;
        let guide = self.initialization_guide(request, &ready)?;
        lock(&self.state).initialization_guide = Some(guide);
        self.quiesce_authority(request, &ready, &validation)?;

        let initialized = (|| -> AppResult<InitializationStatusV1> {
            #[cfg(test)]
            super::super::test_support::run_initialization_before_commit_hook();
            let status = self.commit_initialization(request, &ready)?;
            self.install_reloaded_authority(status.clone())?;
            Ok(status)
        })();
        if let Err(error) = &initialized {
            self.recover_failed_initialization(request, error);
        }
        initialized
    }

    fn status(&self, operation_id: &str) -> AppResult<InitializationStatusV1> {
        validate_operation_id(operation_id)?;
        self.refresh_durable_initialization()
            .map_err(|_| AppError::Message("initialization status is unavailable".to_owned()))?;
        if let Some(status) = lock(&self.state)
            .initialization
            .clone()
            .filter(|status| status.operation_id == operation_id)
        {
            return Ok(status);
        }
        Err(AppError::Message(
            "initialization operation is unknown".to_owned(),
        ))
    }

    fn pending(&self) -> AppResult<PendingInitializationV1> {
        self.refresh_durable_initialization()
            .map_err(|_| AppError::Message("initialization status is unavailable".to_owned()))?;
        let status = lock(&self.state).initialization.clone();
        let Some(status) = status.filter(|status| {
            status.outcome != InitializationOutcomeV1::Complete
                && status.checkpoint != InitializationCheckpointV1::None
        }) else {
            return Ok(PendingInitializationV1 {
                pending: false,
                initialization: None,
                validation: None,
            });
        };
        let validation = self
            .validate_target_inner(Path::new(&status.canonical_root))
            .unwrap_or_else(|_| {
                Self::recovery_validation(
                    &status.canonical_root,
                    "the interrupted initialization target is unavailable",
                )
            });
        Ok(PendingInitializationV1 {
            pending: true,
            initialization: Some(status),
            validation: Some(validation),
        })
    }

    fn completed_initialization(&self) -> AppResult<Option<InitializationStatusV1>> {
        self.refresh_durable_initialization()
            .map_err(|_| AppError::Message("initialization status is unavailable".to_owned()))?;
        Ok(lock(&self.state)
            .initialization
            .clone()
            .filter(|status| status.outcome == InitializationOutcomeV1::Complete))
    }

    fn mark_desktop_bound(&self, operation_id: &str) -> AppResult<InitializationStatusV1> {
        validate_operation_id(operation_id)?;
        let (status, goal, guide) = {
            let state = lock(&self.state);
            let status = state
                .initialization
                .as_ref()
                .filter(|status| status.operation_id == operation_id)
                .ok_or_else(|| {
                    AppError::Message("initialization operation is unknown".to_owned())
                })?;
            if status.checkpoint == InitializationCheckpointV1::DesktopBound {
                return Ok(status.clone());
            }
            if status.checkpoint != InitializationCheckpointV1::GuideApplied {
                return Err(recovery(
                    "desktop binding requires the guide decision to be applied",
                ));
            }
            let status = InitializationStatusV1 {
                checkpoint: InitializationCheckpointV1::DesktopBound,
                outcome: InitializationOutcomeV1::Complete,
                error_kind: String::new(),
                ..status.clone()
            };
            let goal = state.initialization_goal.clone().ok_or_else(|| {
                AppError::Message("initialization operation goal is unavailable".to_owned())
            })?;
            (status, goal, state.initialization_guide.clone())
        };
        publish_desktop_initialization(&self.global_home, &status, &goal, guide.as_ref())?;
        lock(&self.state).initialization = Some(status.clone());
        Ok(status)
    }
}

impl ProductionDesktopAuthority {
    /// The operation `validate_target` readied for this request, with the
    /// guide manifest a previous attempt bound to it.
    fn bound_initialization(
        &self,
        request: &InitializeProjectRequestV1,
    ) -> AppResult<(InitializationStatusV1, Option<DesktopGuideManifest>)> {
        let (ready, bound_goal, bound_guide) = {
            let state = lock(&self.state);
            let ready = state
                .initialization
                .clone()
                .filter(|status| status.operation_id == request.operation_id)
                .ok_or_else(|| {
                    AppError::Message("initialization operation is unknown".to_owned())
                })?;
            (
                ready,
                state.initialization_goal.clone(),
                state.initialization_guide.clone(),
            )
        };
        if bound_goal
            .as_deref()
            .is_some_and(|goal| goal != request.goal)
        {
            return Err(AppError::Message(
                "initialization operation goal does not match its durable request".to_owned(),
            ));
        }
        Ok((ready, bound_guide))
    }

    /// Re-validates the request's target, recording a failure against the
    /// operation, and forgets a readied operation whose target went stale.
    fn validated_initialization_target(
        &self,
        request: &InitializeProjectRequestV1,
        ready: &InitializationStatusV1,
    ) -> AppResult<ProjectTargetValidationV1> {
        let validation = match self.validate_target_inner(Path::new(&request.root)) {
            Ok(validation) => validation,
            Err(error) => {
                let status = InitializationStatusV1 {
                    operation_id: request.operation_id.clone(),
                    canonical_root: request.root.clone(),
                    checkpoint: ready.checkpoint,
                    outcome: if ready.checkpoint == InitializationCheckpointV1::None {
                        InitializationOutcomeV1::Ready
                    } else {
                        InitializationOutcomeV1::RecoveryRequired
                    },
                    error_kind: initialization_error_kind(&error).to_owned(),
                };
                if ready.checkpoint == InitializationCheckpointV1::None {
                    let mut state = lock(&self.state);
                    if state
                        .initialization
                        .as_ref()
                        .is_some_and(|current| current.operation_id == request.operation_id)
                    {
                        state.initialization = Some(status.clone());
                    }
                } else {
                    self.record_initialization_status(status.clone(), &request.goal)?;
                }
                return Err(AppError::Message(status.error_kind));
            }
        };
        if validation.kind != ProjectTargetKindV1::New
            || validation.canonical_root != request.root
            || ready.canonical_root != request.root
        {
            if ready.checkpoint == InitializationCheckpointV1::None {
                let mut state = lock(&self.state);
                if state
                    .initialization
                    .as_ref()
                    .is_some_and(|status| status.operation_id == request.operation_id)
                {
                    state.initialization = None;
                    state.initialization_goal = None;
                }
            }
            return Err(AppError::Message(
                "project initialization request is stale or unsafe".to_owned(),
            ));
        }
        Ok(validation)
    }

    /// The guide manifest this attempt commits: the applied one when the
    /// guide already landed, else the request's choice bound and re-checked
    /// against the files it will replace.
    fn initialization_guide(
        &self,
        request: &InitializeProjectRequestV1,
        ready: &InitializationStatusV1,
    ) -> AppResult<DesktopGuideManifest> {
        let guide = if ready.checkpoint == InitializationCheckpointV1::GuideApplied {
            lock(&self.state)
                .initialization_guide
                .clone()
                .ok_or_else(|| recovery("applied project guide manifest is missing"))?
        } else {
            self.bind_guide_manifest(request, ready)?
        };
        if ready.checkpoint != InitializationCheckpointV1::GuideApplied
            && let Err(error) = validate_guide_before_commit(&self.global_home, &guide)
        {
            let status = InitializationStatusV1 {
                operation_id: request.operation_id.clone(),
                canonical_root: request.root.clone(),
                checkpoint: ready.checkpoint,
                outcome: if ready.checkpoint == InitializationCheckpointV1::None {
                    InitializationOutcomeV1::Ready
                } else {
                    InitializationOutcomeV1::RecoveryRequired
                },
                error_kind: GUIDE_PREVIEW_STALE.to_owned(),
            };
            let durable = path_is_present(
                &self
                    .global_home
                    .join("runtime")
                    .join(DESKTOP_INITIALIZATION),
            )?;
            if durable {
                self.record_initialization_status(status, &request.goal)?;
            } else {
                lock(&self.state).initialization = Some(status);
            }
            return Err(error);
        }
        Ok(guide)
    }

    /// Takes the runtime, workspace factory, recents, and updater out of the
    /// authority so every shared cutover lease is gone before the exclusive
    /// commit, restoring them when the updater refuses to stop.
    fn quiesce_authority(
        &self,
        request: &InitializeProjectRequestV1,
        ready: &InitializationStatusV1,
        validation: &ProjectTargetValidationV1,
    ) -> AppResult<()> {
        let started = InitializationStatusV1 {
            operation_id: request.operation_id.clone(),
            canonical_root: validation.canonical_root.clone(),
            checkpoint: ready.checkpoint,
            outcome: InitializationOutcomeV1::InProgress,
            error_kind: String::new(),
        };
        let (old_runtime, old_factory, old_recents, old_updates) = {
            let mut state = lock(&self.state);
            state.initialization = Some(started);
            state.initialization_goal = Some(request.goal.clone());
            (
                state.runtime.take(),
                state.factory.take(),
                state.recents.take(),
                std::mem::replace(
                    &mut state.updates,
                    UnavailableUpdateService::new(&self.writer_version),
                ),
            )
        };
        if old_updates.shutdown().is_err() {
            let status = InitializationStatusV1 {
                operation_id: request.operation_id.clone(),
                canonical_root: request.root.clone(),
                checkpoint: ready.checkpoint,
                outcome: if ready.checkpoint == InitializationCheckpointV1::None {
                    InitializationOutcomeV1::Ready
                } else {
                    InitializationOutcomeV1::RecoveryRequired
                },
                error_kind: "authority-shutdown-failed".to_owned(),
            };
            let mut state = lock(&self.state);
            state.runtime = old_runtime;
            state.factory = old_factory;
            state.recents = old_recents;
            state.updates = Arc::clone(&old_updates);
            drop(state);
            let (status, goal, guide) = self.reconcile_recovery_status(status, &request.goal);
            let mut state = lock(&self.state);
            state.initialization = Some(status);
            state.initialization_goal = Some(goal);
            state.initialization_guide = guide;
            drop(state);
            return Err(AppError::Message(
                "desktop runtime authority could not be quiesced".to_owned(),
            ));
        }
        drop(old_updates);
        drop(old_recents);
        drop(old_factory);
        drop(old_runtime);
        Ok(())
    }

    /// Records where a failed commit stopped and reinstalls whatever
    /// authority the durable state still supports.
    fn recover_failed_initialization(
        &self,
        request: &InitializeProjectRequestV1,
        error: &AppError,
    ) {
        let checkpoint =
            self.derive_initialization_checkpoint(Path::new(&request.root), &request.goal);
        let status = InitializationStatusV1 {
            operation_id: request.operation_id.clone(),
            canonical_root: request.root.clone(),
            checkpoint,
            outcome: if checkpoint == InitializationCheckpointV1::None {
                InitializationOutcomeV1::Ready
            } else {
                InitializationOutcomeV1::RecoveryRequired
            },
            error_kind: initialization_error_kind(error).to_owned(),
        };
        let (status, goal, guide) = self.reconcile_recovery_status(status, &request.goal);
        let reload_failed = self.install_reloaded_authority(status.clone()).is_err();
        let mut state = lock(&self.state);
        if reload_failed {
            state.initialization = Some(status);
        }
        state.initialization_goal = Some(goal);
        state.initialization_guide = guide;
    }

    pub(super) fn record_initialization_status(
        &self,
        status: InitializationStatusV1,
        goal: &str,
    ) -> AppResult<InitializationStatusV1> {
        let guide = lock(&self.state).initialization_guide.clone();
        publish_desktop_initialization(&self.global_home, &status, goal, guide.as_ref())?;
        let mut state = lock(&self.state);
        state.initialization = Some(status.clone());
        state.initialization_goal = Some(goal.to_owned());
        drop(state);
        Ok(status)
    }

    pub(super) fn record_recovery_if_owned(
        &self,
        status: &InitializationStatusV1,
        goal: &str,
    ) -> AppResult<Option<DesktopInitializationJournal>> {
        if !path_is_present(&self.global_home.join("runtime"))? {
            return Ok(None);
        }
        with_desktop_initialization_lock(&self.global_home, || {
            let Some(journal) = read_desktop_initialization(&self.global_home)? else {
                return Ok(None);
            };
            if journal.status.operation_id != status.operation_id || journal.goal != goal {
                return Ok(None);
            }
            validate_desktop_initialization_transition(&journal.status, status)?;
            publish_desktop_initialization_locked(
                &self.global_home,
                status,
                goal,
                journal.guide.as_ref(),
            )?;
            Ok(Some(DesktopInitializationJournal {
                format: journal.format,
                version: journal.version,
                status: status.clone(),
                goal: goal.to_owned(),
                guide: journal.guide,
            }))
        })
    }

    pub(super) fn reconcile_recovery_status(
        &self,
        proposed: InitializationStatusV1,
        goal: &str,
    ) -> (InitializationStatusV1, String, Option<DesktopGuideManifest>) {
        if let Ok(Some(journal)) = self.record_recovery_if_owned(&proposed, goal) {
            return (journal.status, journal.goal, journal.guide);
        }
        let local_guide = lock(&self.state).initialization_guide.clone();
        match read_desktop_initialization(&self.global_home) {
            Ok(Some(journal)) => (journal.status, journal.goal, journal.guide),
            Ok(None) | Err(_) => (proposed, goal.to_owned(), local_guide),
        }
    }

    pub(super) fn derive_initialization_checkpoint(
        &self,
        root: &Path,
        goal: &str,
    ) -> InitializationCheckpointV1 {
        if let Ok(Some(runtime)) = ActiveRuntime::load(&self.global_home, &self.writer_version) {
            if let Ok(bindings) = runtime.bindings_for_exact_root(root)
                && let Some(endpoint) = bindings.project
            {
                let committed = ProjectStore::open_existing(
                    &endpoint.database,
                    &endpoint.binding,
                    &self.writer_version,
                )
                .and_then(|store| store.meta())
                .is_ok_and(|meta| meta.goal == goal);
                return if committed {
                    InitializationCheckpointV1::ProjectCommitted
                } else {
                    InitializationCheckpointV1::RuntimeCommitted
                };
            }
            return if selected_project_storage_present(root) {
                InitializationCheckpointV1::RuntimeCommitted
            } else if selected_project_directory_present(root) {
                InitializationCheckpointV1::Prepared
            } else {
                InitializationCheckpointV1::None
            };
        }
        if path_is_present(&self.global_home.join("runtime").join(BOOTSTRAP_PLAN)).unwrap_or(false)
        {
            InitializationCheckpointV1::Prepared
        } else if selected_project_storage_present(root) {
            InitializationCheckpointV1::RuntimeCommitted
        } else if selected_project_directory_present(root) {
            InitializationCheckpointV1::Prepared
        } else {
            InitializationCheckpointV1::None
        }
    }

    pub(super) fn refresh_durable_initialization(&self) -> AppResult<()> {
        let durable_required = lock(&self.state)
            .initialization
            .as_ref()
            .is_some_and(|status| {
                status.checkpoint != InitializationCheckpointV1::None
                    || status.outcome == InitializationOutcomeV1::Complete
            });
        let journal_path = self
            .global_home
            .join("runtime")
            .join(DESKTOP_INITIALIZATION);
        if !self.global_home.exists() || !path_is_present(&journal_path)? {
            return if durable_required {
                Err(recovery("desktop initialization status disappeared"))
            } else {
                Ok(())
            };
        }
        let journal = with_desktop_initialization_lock(&self.global_home, || {
            read_desktop_initialization(&self.global_home)
        })?;
        let journal =
            journal.ok_or_else(|| recovery("desktop initialization status disappeared"))?;
        let reload_components = journal.status.outcome == InitializationOutcomeV1::Complete
            && lock(&self.state).factory.is_none();
        if reload_components {
            self.install_reloaded_authority(journal.status.clone())?;
        }
        let mut state = lock(&self.state);
        state.initialization = Some(journal.status);
        state.initialization_goal = Some(journal.goal);
        state.initialization_guide = journal.guide;
        drop(state);
        Ok(())
    }
}

/// A request replayed after its initialization completed must match the
/// durable request exactly.
fn replay_bound_initialization(
    request: &InitializeProjectRequestV1,
    ready: InitializationStatusV1,
    bound_guide: Option<DesktopGuideManifest>,
) -> AppResult<InitializationStatusV1> {
    let guide = bound_guide
        .ok_or_else(|| recovery("completed initialization guide manifest is missing"))?;
    if ready.outcome != InitializationOutcomeV1::Complete
        || ready.canonical_root != request.root
        || guide.operation_id != request.operation_id
        || guide.canonical_root != request.root
        || guide.choice != request.guide_choice
        || guide.preview_token != request.guide_preview_token
    {
        return Err(AppError::Message(
            "initialization operation request does not match its durable request".to_owned(),
        ));
    }
    Ok(ready)
}

/// The guide choice and its preview token must agree before any target work.
fn validate_guide_request(request: &InitializeProjectRequestV1) -> AppResult<()> {
    match request.guide_choice {
        ProjectGuideChoiceV1::Skip if !request.guide_preview_token.is_empty() => {
            return Err(AppError::Message(
                "skipping project guidance requires an empty preview token".to_owned(),
            ));
        }
        ProjectGuideChoiceV1::Install
            if validate_operation_id(&request.guide_preview_token).is_err() =>
        {
            return Err(AppError::Message(
                "installing project guidance requires a valid preview token".to_owned(),
            ));
        }
        ProjectGuideChoiceV1::Skip | ProjectGuideChoiceV1::Install => {}
    }
    #[cfg(not(unix))]
    if request.guide_choice == ProjectGuideChoiceV1::Install {
        return Err(AppError::Message("project-guide-unavailable".to_owned()));
    }
    Ok(())
}
