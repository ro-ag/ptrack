//! Project guidance through the desktop authority: previewing the proposed
//! guide files and binding the user's guide choice to an initialization.

use std::collections::BTreeMap;
use std::path::Path;

#[cfg(unix)]
use ptrack_core::upsert_guide;
use ptrack_store::{PinnedProjectDirectory, PrivatePathIdentity};

#[cfg(unix)]
use super::super::guide::{
    DesktopGuideFileManifest, guide_diff, guide_line_counts, read_guide_template,
};
use super::super::guide::{
    DesktopGuideManifest, guide_manifest_has_applied_output, validate_guide_manifest,
};
use super::super::journal::stale_guide_skip_allowed;
#[cfg(unix)]
use super::super::pinned_guide::PinnedGuideRoot;
#[cfg(unix)]
use super::super::{
    GUIDE_DIFF_LINE_LIMIT, GUIDE_FILES, GUIDE_OUTPUT_LIMIT, GUIDE_PREVIEW_LIMIT, content_digest,
    random_operation_id, validate_operation_id,
};
use super::super::{GUIDE_PARTIALLY_APPLIED, GUIDE_PREVIEW_STALE, lock, recovery};
use super::ProductionDesktopAuthority;
#[cfg(not(unix))]
use crate::ProjectGuidePreviewV1;
use crate::{
    AppError, AppResult, InitializationCheckpointV1, InitializationOutcomeV1,
    InitializationStatusV1, InitializeProjectRequestV1, ProjectGuideChoiceV1,
};
#[cfg(unix)]
use crate::{
    ProjectGuideFileActionV1, ProjectGuideFilePreviewV1, ProjectGuidePreviewRequestV1,
    ProjectGuidePreviewV1, ProjectTargetKindV1,
};

impl ProductionDesktopAuthority {
    #[cfg(not(unix))]
    pub(super) fn guide_unavailable() -> ProjectGuidePreviewV1 {
        ProjectGuidePreviewV1 {
            available: false,
            message: GUIDE_UNAVAILABLE.to_owned(),
            preview_token: String::new(),
            files: Vec::new(),
        }
    }

    #[cfg(unix)]
    pub(super) fn preview_guide_inner(
        &self,
        request: &ProjectGuidePreviewRequestV1,
    ) -> AppResult<ProjectGuidePreviewV1> {
        validate_operation_id(&request.operation_id)?;
        let validation = self.validate_target_inner(Path::new(&request.root))?;
        if validation.kind != ProjectTargetKindV1::New
            || validation.operation_id != request.operation_id
            || validation.canonical_root != request.root
        {
            return Err(AppError::Message(
                "project guide preview target is stale or unsafe".to_owned(),
            ));
        }
        let root = Path::new(&request.root);
        let root_identity = PinnedProjectDirectory::identify_root(root).map_err(recovery)?;
        let template = read_guide_template(&self.global_home)?;
        let template_digest = content_digest(template.as_bytes());
        let preview_token = random_operation_id()?;
        let mut files = Vec::with_capacity(GUIDE_FILES.len());
        let mut manifests = Vec::with_capacity(GUIDE_FILES.len());
        let guide_root = PinnedGuideRoot::capture(root, root_identity)?;
        for name in GUIDE_FILES {
            let base = guide_root.read(name)?;
            let base_content = base
                .as_ref()
                .map_or("", |snapshot| snapshot.content.as_str());
            let (output, changed) = upsert_guide(base_content, &template);
            if output.len() > GUIDE_OUTPUT_LIMIT {
                return Err(AppError::Message(
                    "project guide proposed content exceeds its byte limit".to_owned(),
                ));
            }
            let action = if !changed {
                ProjectGuideFileActionV1::NoChange
            } else if base.is_some() {
                ProjectGuideFileActionV1::Update
            } else {
                ProjectGuideFileActionV1::Create
            };
            let diff = guide_diff(name, base_content, &output, action)?;
            let (additions, deletions) = guide_line_counts(base_content, &output, action);
            if additions > GUIDE_DIFF_LINE_LIMIT || deletions > GUIDE_DIFF_LINE_LIMIT {
                return Err(AppError::Message(
                    "project guide preview line count exceeds its limit".to_owned(),
                ));
            }
            files.push(ProjectGuideFilePreviewV1 {
                path: name.to_owned(),
                action,
                additions,
                deletions,
                diff,
            });
            manifests.push(DesktopGuideFileManifest {
                name: name.to_owned(),
                action,
                base_identity: base.as_ref().map(|snapshot| snapshot.identity),
                base_digest: base
                    .as_ref()
                    .map_or_else(String::new, |snapshot| snapshot.digest.clone()),
                output_digest: content_digest(output.as_bytes()),
                mode: base.as_ref().map_or(0o644, |snapshot| snapshot.mode),
            });
        }
        guide_root.verify()?;
        let manifest = DesktopGuideManifest {
            version: "1".to_owned(),
            choice: ProjectGuideChoiceV1::Install,
            operation_id: request.operation_id.clone(),
            canonical_root: request.root.clone(),
            preview_token: preview_token.clone(),
            root_identity: Some(root_identity),
            template_digest,
            files: manifests,
        };
        validate_guide_manifest(&manifest)?;
        let mut state = lock(&self.state);
        state
            .guide_previews
            .retain(|_, preview| preview.operation_id != request.operation_id);
        while state.guide_previews.len() >= GUIDE_PREVIEW_LIMIT {
            let Some(oldest) = state.guide_previews.keys().next().cloned() else {
                break;
            };
            state.guide_previews.remove(&oldest);
        }
        state.guide_previews.insert(preview_token.clone(), manifest);
        drop(state);
        Ok(ProjectGuidePreviewV1 {
            available: true,
            message: String::new(),
            preview_token,
            files,
        })
    }

    pub(super) fn bind_guide_manifest(
        &self,
        request: &InitializeProjectRequestV1,
        ready: &InitializationStatusV1,
    ) -> AppResult<DesktopGuideManifest> {
        let mut state = lock(&self.state);
        let existing = state.initialization_guide.clone();
        let immutable_root_identity = existing.as_ref().and_then(|guide| guide.root_identity);
        let selected_root_identity =
            || PinnedProjectDirectory::identify_root(Path::new(&request.root)).map_err(recovery);
        let postcommit_refresh_allowed = ready.checkpoint
            == InitializationCheckpointV1::ProjectCommitted
            && ready.outcome == InitializationOutcomeV1::RecoveryRequired
            && matches!(
                ready.error_kind.as_str(),
                GUIDE_PREVIEW_STALE | GUIDE_PARTIALLY_APPLIED
            );
        let stale_skip_allowed = stale_guide_skip_allowed(ready);
        let precommit_refresh_allowed = ready.checkpoint == InitializationCheckpointV1::None
            && ready.outcome == InitializationOutcomeV1::Ready
            && ready.error_kind == GUIDE_PREVIEW_STALE;
        let selected = match (existing, request.guide_choice) {
            (None, ProjectGuideChoiceV1::Skip) => DesktopGuideManifest::skip(
                request.operation_id.clone(),
                request.root.clone(),
                selected_root_identity()?,
            ),
            (None, ProjectGuideChoiceV1::Install) => {
                take_guide_preview(&mut state.guide_previews, &request.guide_preview_token)?
            }
            (Some(existing), choice)
                if existing.choice == choice
                    && existing.preview_token == request.guide_preview_token =>
            {
                existing
            }
            (Some(existing), ProjectGuideChoiceV1::Install)
                if existing.choice == ProjectGuideChoiceV1::Skip =>
            {
                return Err(AppError::Message(
                    "skipped project guidance cannot be upgraded for this operation".to_owned(),
                ));
            }
            (Some(existing), ProjectGuideChoiceV1::Skip) if stale_skip_allowed => {
                if ready.checkpoint == InitializationCheckpointV1::ProjectCommitted
                    && guide_manifest_has_applied_output(&existing)?
                {
                    let status = InitializationStatusV1 {
                        error_kind: GUIDE_PARTIALLY_APPLIED.to_owned(),
                        ..ready.clone()
                    };
                    drop(state);
                    self.record_initialization_status(status, &request.goal)?;
                    return Err(AppError::Message(GUIDE_PARTIALLY_APPLIED.to_owned()));
                }
                DesktopGuideManifest::skip(
                    request.operation_id.clone(),
                    request.root.clone(),
                    selected_root_identity()?,
                )
            }
            (Some(_), ProjectGuideChoiceV1::Install) if postcommit_refresh_allowed => {
                take_guide_preview(&mut state.guide_previews, &request.guide_preview_token)?
            }
            (Some(_), ProjectGuideChoiceV1::Install) if precommit_refresh_allowed => {
                take_guide_preview(&mut state.guide_previews, &request.guide_preview_token)?
            }
            (Some(existing), ProjectGuideChoiceV1::Install)
                if existing.choice == ProjectGuideChoiceV1::Install
                    && !matches!(
                        ready.checkpoint,
                        InitializationCheckpointV1::GuideApplied
                            | InitializationCheckpointV1::DesktopBound
                    ) =>
            {
                take_guide_preview(&mut state.guide_previews, &request.guide_preview_token)?
            }
            (Some(_), _) => {
                return Err(AppError::Message(
                    "project guide choice does not match its durable request".to_owned(),
                ));
            }
        };
        require_guide_matches_request(&selected, request, immutable_root_identity)?;
        Ok(selected)
    }
}

/// Takes the preview a request names out of the pending previews.
fn take_guide_preview(
    previews: &mut BTreeMap<String, DesktopGuideManifest>,
    token: &str,
) -> AppResult<DesktopGuideManifest> {
    previews
        .remove(token)
        .ok_or_else(|| AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))
}

/// A bound manifest must name exactly the request's operation, root, and
/// choice, and never a different root identity than the durable one.
fn require_guide_matches_request(
    selected: &DesktopGuideManifest,
    request: &InitializeProjectRequestV1,
    immutable_root_identity: Option<PrivatePathIdentity>,
) -> AppResult<()> {
    validate_guide_manifest(selected)?;
    if immutable_root_identity.is_some() && selected.root_identity != immutable_root_identity {
        return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
    }
    if selected.operation_id != request.operation_id
        || selected.canonical_root != request.root
        || selected.choice != request.guide_choice
    {
        return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
    }
    Ok(())
}
