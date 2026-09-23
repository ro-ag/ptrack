//! Classifies a folder the first-run dialog selected: a new project, an
//! existing one, a resumable interrupted initialization, or a recovery case.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use super::super::bootstrap::{
    global_home_exemptions, home_project_refusal, is_global_home, read_bootstrap_plan,
    selected_project_directory_present, validate_bootstrap_plan,
};
use super::super::guide::DesktopGuideManifest;
use super::super::journal::initialization_checkpoint_rank;
use super::super::runtime::ActiveRuntime;
use super::super::{BOOTSTRAP_PLAN, lock, path_is_present, random_operation_id, recovery};
use super::ProductionDesktopAuthority;
use crate::{
    AppError, AppResult, InitializationCheckpointV1, InitializationOutcomeV1,
    InitializationStatusV1, ProjectGuideChoiceV1, ProjectTargetKindV1, ProjectTargetValidationV1,
};

impl ProductionDesktopAuthority {
    pub(super) fn recovery_validation(
        canonical_root: &str,
        reason: impl Into<String>,
    ) -> ProjectTargetValidationV1 {
        ProjectTargetValidationV1 {
            kind: ProjectTargetKindV1::RecoveryRequired,
            canonical_root: canonical_root.to_owned(),
            operation_id: String::new(),
            reason: reason.into(),
            initialization: None,
            goal: None,
            guide_choice: None,
        }
    }

    pub(super) fn validate_target_inner(
        &self,
        selected: &Path,
    ) -> AppResult<ProjectTargetValidationV1> {
        let canonical = fs::canonicalize(selected)?;
        let canonical_text = canonical.to_str().ok_or_else(|| {
            AppError::Message("the selected target path is not valid UTF-8".to_owned())
        })?;
        if !canonical.is_dir() {
            return Ok(Self::recovery_validation(
                canonical_text,
                "the selected target is not a directory",
            ));
        }
        let (mut runtime, initialization, initialization_goal, initialization_guide) = {
            let state = lock(&self.state);
            (
                state.runtime.clone(),
                state.initialization.clone(),
                state.initialization_goal.clone(),
                state.initialization_guide.clone(),
            )
        };
        if runtime.is_none()
            && initialization.as_ref().is_some_and(|status| {
                initialization_checkpoint_rank(status.checkpoint)
                    >= initialization_checkpoint_rank(InitializationCheckpointV1::RuntimeCommitted)
            })
        {
            runtime = ActiveRuntime::load(&self.global_home, &self.writer_version)
                .ok()
                .flatten();
        }
        let resumable = initialization.as_ref().filter(|status| {
            status.outcome != InitializationOutcomeV1::Complete
                && status.canonical_root == canonical_text
        });
        let resume = Resume {
            goal: initialization_goal.as_ref(),
            guide: initialization_guide.as_ref(),
        };
        if let Some(status) = resumable {
            if let Some(validation) = self.resumable_target(
                &canonical,
                canonical_text,
                status,
                runtime.as_ref(),
                &resume,
            )? {
                return Ok(validation);
            }
        } else if initialization.as_ref().is_some_and(|status| {
            status.outcome != InitializationOutcomeV1::Complete
                && status.checkpoint != InitializationCheckpointV1::None
        }) {
            return Ok(Self::recovery_validation(
                canonical_text,
                "another project has an incomplete initialization",
            ));
        }
        if let Some(validation) = self.registered_target(&canonical, runtime)? {
            return Ok(validation);
        }
        if let Some(validation) = self.storage_refusal(&canonical, canonical_text)? {
            return Ok(validation);
        }
        let (initialization, goal, guide_choice) =
            resumable.map_or((None, None, None), |status| resume.metadata(status));
        Ok(ProjectTargetValidationV1 {
            kind: ProjectTargetKindV1::New,
            canonical_root: canonical_text.to_owned(),
            operation_id: resumable.map_or_else(random_operation_id, |status| {
                Ok(status.operation_id.clone())
            })?,
            reason: String::new(),
            initialization,
            goal,
            guide_choice,
        })
    }

    /// Classifies the folder an interrupted initialization was working on,
    /// or `None` when it can start over as a new project.
    fn resumable_target(
        &self,
        canonical: &Path,
        canonical_text: &str,
        status: &InitializationStatusV1,
        runtime: Option<&Arc<ActiveRuntime>>,
        resume: &Resume<'_>,
    ) -> AppResult<Option<ProjectTargetValidationV1>> {
        let plan_path = self.global_home.join("runtime").join(BOOTSTRAP_PLAN);
        if path_is_present(&plan_path)? {
            let plan = read_bootstrap_plan(&plan_path)?;
            let home = fs::canonicalize(&self.global_home)?;
            validate_bootstrap_plan(&home, canonical, &plan, &self.writer_version)?;
            if plan.operation_id.as_deref() != Some(status.operation_id.as_str()) {
                return Ok(Some(Self::recovery_validation(
                    canonical_text,
                    "bootstrap storage is not bound to this initialization operation",
                )));
            }
            return Ok(Some(resume.resumed(status)));
        }
        if status.checkpoint != InitializationCheckpointV1::None {
            if runtime.is_some_and(|runtime| {
                runtime
                    .bindings_for_exact_root(canonical)
                    .is_ok_and(|bindings| bindings.project.is_some())
            }) {
                return Ok(Some(resume.resumed(status)));
            }
            return Ok(Some(Self::recovery_validation(
                canonical_text,
                "the interrupted initialization cannot be resumed safely",
            )));
        }
        if selected_project_directory_present(canonical) {
            return Ok(Some(Self::recovery_validation(
                canonical_text,
                "interrupted project storage requires recovery",
            )));
        }
        Ok(None)
    }

    /// The `Existing` classification of a folder the marker already maps,
    /// reloading the marker once in case the command line registered it.
    fn registered_target(
        &self,
        canonical: &Path,
        runtime: Option<Arc<ActiveRuntime>>,
    ) -> AppResult<Option<ProjectTargetValidationV1>> {
        let runtime = match runtime {
            Some(current) if current.bindings_for(canonical)?.project.is_none() => {
                Some(self.reload_marker().unwrap_or(current))
            }
            other => other,
        };
        if let Some(runtime) = runtime
            && let Some(project) = runtime.bindings_for(canonical)?.project
        {
            return Ok(Some(ProjectTargetValidationV1 {
                kind: ProjectTargetKindV1::Existing,
                canonical_root: project
                    .root
                    .to_str()
                    .ok_or_else(|| recovery("registered project root is not valid UTF-8"))?
                    .to_owned(),
                operation_id: String::new(),
                reason: String::new(),
                initialization: None,
                goal: None,
                guide_choice: None,
            }));
        }
        Ok(None)
    }

    /// The recovery classification of a folder whose own or ancestor storage,
    /// or the global runtime state, must be repaired before a new project.
    fn storage_refusal(
        &self,
        canonical: &Path,
        canonical_text: &str,
    ) -> AppResult<Option<ProjectTargetValidationV1>> {
        let refusal =
            |reason: &'static str| Ok(Some(Self::recovery_validation(canonical_text, reason)));
        let global_homes = global_home_exemptions(&self.global_home);
        // A home directory — p-track's or the user's own — is refused by
        // name rather than read as a foreign project store.
        if let Some(reason) = home_project_refusal(canonical, &global_homes) {
            return refusal(reason);
        }
        for (depth, ancestor) in canonical.ancestors().enumerate() {
            let storage = ancestor.join(".ptrack");
            // Depth 0 is the selected root itself, which never gets the exemption:
            // its own `.ptrack` must not be the global home.
            if depth > 0 && is_global_home(&storage, &global_homes) {
                continue;
            }
            if path_is_present(&storage.join("ptrack.redb"))? {
                // A store at the selected root itself is the moved-project
                // shape. The hint never opens the file: relocation fail-closes
                // on anything that is not a genuinely moved store, and this
                // walk must stay a read-only classification.
                return refusal(if depth == 0 {
                    "an unregistered project store requires recovery; if this project folder was moved, quit p-track and run 'ptrack relocate' in it to re-register it"
                } else {
                    "an unregistered project store requires recovery"
                });
            }
            if let Ok(metadata) = fs::symlink_metadata(&storage) {
                return refusal(if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    "project storage is unsafe"
                } else {
                    "preexisting project storage requires recovery"
                });
            }
        }
        // A bound runtime is the proof the global state is healthy. With no
        // bound runtime, the database's presence means a store the runtime
        // refused, and a leftover bootstrap plan is an interrupted bootstrap
        // either way.
        if (lock(&self.state).runtime.is_none()
            && path_is_present(&self.global_home.join("global.redb"))?)
            || path_is_present(&self.global_home.join("runtime").join(BOOTSTRAP_PLAN))?
        {
            return refusal("global runtime state requires recovery");
        }
        Ok(None)
    }
}

/// What a resumed initialization reports back to the first-run dialog.
struct Resume<'a> {
    goal: Option<&'a String>,
    guide: Option<&'a DesktopGuideManifest>,
}

impl Resume<'_> {
    fn metadata(
        &self,
        status: &InitializationStatusV1,
    ) -> (
        Option<InitializationStatusV1>,
        Option<String>,
        Option<ProjectGuideChoiceV1>,
    ) {
        match (self.goal, self.guide) {
            (Some(goal), Some(guide)) => {
                (Some(status.clone()), Some(goal.clone()), Some(guide.choice))
            }
            _ => (None, None, None),
        }
    }

    /// The `New` classification that resumes `status` where it stopped.
    fn resumed(&self, status: &InitializationStatusV1) -> ProjectTargetValidationV1 {
        let (initialization, goal, guide_choice) = self.metadata(status);
        ProjectTargetValidationV1 {
            kind: ProjectTargetKindV1::New,
            canonical_root: status.canonical_root.clone(),
            operation_id: status.operation_id.clone(),
            reason: String::new(),
            initialization,
            goal,
            guide_choice,
        }
    }
}
