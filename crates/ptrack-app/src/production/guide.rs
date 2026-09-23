//! Project guide manifests: the consent record a preview produces, its
//! validation, and applying or installing it through the pinned publisher.

use std::io::Read;
use std::path::Path;

#[cfg(unix)]
use ptrack_core::upsert_guide;
use ptrack_store::{PinnedProjectDirectory, PrivatePathIdentity, open_private_path};
use serde::{Deserialize, Serialize};

#[cfg(unix)]
use super::pinned_guide::PinnedGuideRoot;
use super::{
    GUIDE_DIFF_LIMIT, GUIDE_FILE_LIMIT, GUIDE_FILES, GUIDE_OUTPUT_LIMIT, GUIDE_PARTIALLY_APPLIED,
    GUIDE_PREVIEW_STALE, content_digest, path_is_present, recovery, validate_operation_id,
};
use crate::{AppError, AppResult, ProjectGuideChoiceV1, ProjectGuideFileActionV1};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DesktopGuideManifest {
    pub(super) version: String,
    pub(super) choice: ProjectGuideChoiceV1,
    pub(super) operation_id: String,
    pub(super) canonical_root: String,
    pub(super) preview_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) root_identity: Option<PrivatePathIdentity>,
    pub(super) template_digest: String,
    pub(super) files: Vec<DesktopGuideFileManifest>,
}

impl DesktopGuideManifest {
    pub(super) fn skip(
        operation_id: String,
        canonical_root: String,
        root_identity: PrivatePathIdentity,
    ) -> Self {
        Self {
            version: "1".to_owned(),
            choice: ProjectGuideChoiceV1::Skip,
            operation_id,
            canonical_root,
            preview_token: String::new(),
            root_identity: Some(root_identity),
            template_digest: String::new(),
            files: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DesktopGuideFileManifest {
    pub(super) name: String,
    pub(super) action: ProjectGuideFileActionV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) base_identity: Option<PrivatePathIdentity>,
    pub(super) base_digest: String,
    pub(super) output_digest: String,
    pub(super) mode: u32,
}

#[derive(Clone, Debug)]
pub(super) struct GuideFileSnapshot {
    pub(super) identity: PrivatePathIdentity,
    pub(super) digest: String,
    pub(super) content: String,
    pub(super) mode: u32,
}

fn valid_digest(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(super) fn validate_guide_manifest(manifest: &DesktopGuideManifest) -> AppResult<()> {
    validate_operation_id(&manifest.operation_id)?;
    if manifest.version != "1"
        || manifest.canonical_root.is_empty()
        || manifest.canonical_root.len() > 4_096
        || !Path::new(&manifest.canonical_root).is_absolute()
    {
        return Err(recovery("desktop initialization guide is invalid"));
    }
    match manifest.choice {
        ProjectGuideChoiceV1::Skip => {
            if !manifest.preview_token.is_empty()
                || manifest.root_identity.is_none()
                || !manifest.template_digest.is_empty()
                || !manifest.files.is_empty()
            {
                return Err(recovery("skipped project guide manifest is invalid"));
            }
        }
        ProjectGuideChoiceV1::Install => {
            validate_operation_id(&manifest.preview_token)?;
            if manifest.root_identity.is_none()
                || !valid_digest(&manifest.template_digest)
                || manifest.files.len() != GUIDE_FILES.len()
            {
                return Err(recovery("project guide manifest is incomplete"));
            }
            for (file, expected_name) in manifest.files.iter().zip(GUIDE_FILES) {
                if file.name != expected_name
                    || !valid_digest(&file.output_digest)
                    || file.mode > 0o7777
                    || match file.action {
                        ProjectGuideFileActionV1::Create => {
                            file.base_identity.is_some() || !file.base_digest.is_empty()
                        }
                        ProjectGuideFileActionV1::Update | ProjectGuideFileActionV1::NoChange => {
                            file.base_identity.is_none() || !valid_digest(&file.base_digest)
                        }
                    }
                {
                    return Err(recovery("project guide file manifest is invalid"));
                }
            }
        }
    }
    Ok(())
}

pub(super) fn read_guide_template(home: &Path) -> AppResult<String> {
    let path = home.join("guide.md");
    if !path_is_present(&path)? {
        return Ok(String::new());
    }
    let file = open_private_path(&path, false, false).map_err(recovery)?;
    let length = file.metadata()?.len();
    if length > GUIDE_FILE_LIMIT {
        return Err(AppError::Message(
            "project guide template exceeds its byte limit".to_owned(),
        ));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(length).unwrap_or_default());
    file.take(GUIDE_FILE_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > GUIDE_FILE_LIMIT {
        return Err(AppError::Message(
            "project guide template exceeds its byte limit".to_owned(),
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| AppError::Message("project guide template is not valid UTF-8".to_owned()))
}

pub(super) fn validate_guide_before_commit(
    home: &Path,
    manifest: &DesktopGuideManifest,
) -> AppResult<()> {
    validate_guide_manifest(manifest)?;
    if manifest.choice == ProjectGuideChoiceV1::Skip {
        let current = PinnedProjectDirectory::identify_root(Path::new(&manifest.canonical_root))
            .map_err(recovery)?;
        return if Some(current) == manifest.root_identity {
            Ok(())
        } else {
            Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))
        };
    }
    #[cfg(not(unix))]
    {
        let _ = home;
        return Err(AppError::Message("project-guide-unavailable".to_owned()));
    }
    #[cfg(unix)]
    {
        let root_identity = manifest
            .root_identity
            .ok_or_else(|| recovery("project guide root identity is missing"))?;
        let root = PinnedGuideRoot::capture(Path::new(&manifest.canonical_root), root_identity)?;
        let template = read_guide_template(home)?;
        if content_digest(template.as_bytes()) != manifest.template_digest {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        for file in &manifest.files {
            validate_guide_file_state(&root, file, &template)?;
        }
        root.verify()
    }
}

pub(super) fn apply_guide_manifest(
    home: &Path,
    manifest: &DesktopGuideManifest,
    pinned: &PinnedProjectDirectory,
) -> AppResult<()> {
    if manifest.choice == ProjectGuideChoiceV1::Skip {
        pinned.verify().map_err(recovery)?;
        return if Some(pinned.root_identity()) == manifest.root_identity {
            Ok(())
        } else {
            Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))
        };
    }
    #[cfg(not(unix))]
    {
        let _ = (home, pinned);
        return Err(AppError::Message("project-guide-unavailable".to_owned()));
    }
    #[cfg(unix)]
    {
        let root_identity = manifest
            .root_identity
            .ok_or_else(|| recovery("project guide root identity is missing"))?;
        if pinned.root_identity() != root_identity {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        let guide_root = PinnedGuideRoot::from_pinned(pinned, root_identity)?;
        let template = read_guide_template(home)?;
        if content_digest(template.as_bytes()) != manifest.template_digest {
            return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
        }
        for file in &manifest.files {
            if let Err(error) = validate_guide_file_state(&guide_root, file, &template) {
                return if guide_root_has_applied_output(&guide_root, &manifest.files)? {
                    Err(AppError::Message(GUIDE_PARTIALLY_APPLIED.to_owned()))
                } else {
                    Err(error)
                };
            }
        }
        for file in &manifest.files {
            let applied = (|| -> AppResult<()> {
                let current = guide_root.read(&file.name)?;
                if current
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.digest == file.output_digest)
                {
                    return Ok(());
                }
                require_guide_base(current.as_ref(), file)?;
                let base = current
                    .as_ref()
                    .map_or("", |snapshot| snapshot.content.as_str());
                let (output, _) = upsert_guide(base, &template);
                if output.len() > GUIDE_OUTPUT_LIMIT
                    || content_digest(output.as_bytes()) != file.output_digest
                {
                    return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
                }
                guide_root.publish(file, &output)
            })();
            if let Err(error) = applied {
                return if guide_root_has_applied_output(&guide_root, &manifest.files)? {
                    Err(AppError::Message(GUIDE_PARTIALLY_APPLIED.to_owned()))
                } else {
                    Err(error)
                };
            }
        }
        guide_root.verify()
    }
}

#[cfg(unix)]
pub(super) fn guide_root_has_applied_output(
    root: &PinnedGuideRoot<'_>,
    files: &[DesktopGuideFileManifest],
) -> AppResult<bool> {
    for file in files {
        if file.action != ProjectGuideFileActionV1::NoChange
            && root
                .read(&file.name)?
                .is_some_and(|snapshot| snapshot.digest == file.output_digest)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn guide_manifest_has_applied_output(
    manifest: &DesktopGuideManifest,
) -> AppResult<bool> {
    if manifest.choice != ProjectGuideChoiceV1::Install {
        return Ok(false);
    }
    #[cfg(not(unix))]
    {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        let root_identity = manifest
            .root_identity
            .ok_or_else(|| recovery("project guide root identity is missing"))?;
        let root = PinnedGuideRoot::capture(Path::new(&manifest.canonical_root), root_identity)?;
        for file in &manifest.files {
            if file.action != ProjectGuideFileActionV1::NoChange
                && root
                    .read(&file.name)?
                    .is_some_and(|snapshot| snapshot.digest == file.output_digest)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(unix)]
pub(crate) fn install_project_guide_pinned(
    pinned: &PinnedProjectDirectory,
    extra: &str,
) -> AppResult<Vec<&'static str>> {
    let root = PinnedGuideRoot::from_pinned(pinned, pinned.root_identity())?;
    let mut written = Vec::new();
    for name in GUIDE_FILES {
        let base = root.read(name)?;
        let base_content = base
            .as_ref()
            .map_or("", |snapshot| snapshot.content.as_str());
        let (output, changed) = upsert_guide(base_content, extra);
        if !changed {
            continue;
        }
        if output.len() > GUIDE_OUTPUT_LIMIT {
            return Err(AppError::Message(
                "project guide proposed content exceeds its byte limit".to_owned(),
            ));
        }
        let manifest = DesktopGuideFileManifest {
            name: name.to_owned(),
            action: if base.is_some() {
                ProjectGuideFileActionV1::Update
            } else {
                ProjectGuideFileActionV1::Create
            },
            base_identity: base.as_ref().map(|snapshot| snapshot.identity),
            base_digest: base
                .as_ref()
                .map_or_else(String::new, |snapshot| snapshot.digest.clone()),
            output_digest: content_digest(output.as_bytes()),
            mode: base.as_ref().map_or(0o644, |snapshot| snapshot.mode),
        };
        root.publish(&manifest, &output)?;
        written.push(name);
    }
    root.verify()?;
    pinned.verify().map_err(recovery)?;
    Ok(written)
}

#[cfg(not(unix))]
pub(crate) fn install_project_guide_pinned(
    _pinned: &PinnedProjectDirectory,
    _extra: &str,
) -> AppResult<Vec<&'static str>> {
    Err(AppError::Message("project-guide-unavailable".to_owned()))
}

#[cfg(unix)]
pub(super) fn validate_guide_file_state(
    root: &PinnedGuideRoot<'_>,
    file: &DesktopGuideFileManifest,
    template: &str,
) -> AppResult<()> {
    let current = root.read(&file.name)?;
    if current
        .as_ref()
        .is_some_and(|snapshot| snapshot.digest == file.output_digest)
    {
        return Ok(());
    }
    require_guide_base(current.as_ref(), file)?;
    let base = current
        .as_ref()
        .map_or("", |snapshot| snapshot.content.as_str());
    let (output, _) = upsert_guide(base, template);
    if output.len() > GUIDE_OUTPUT_LIMIT || content_digest(output.as_bytes()) != file.output_digest
    {
        return Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()));
    }
    Ok(())
}

pub(super) fn require_guide_base(
    current: Option<&GuideFileSnapshot>,
    file: &DesktopGuideFileManifest,
) -> AppResult<()> {
    let matches = match (current, file.base_identity) {
        (None, None) => file.base_digest.is_empty(),
        (Some(current), Some(identity)) => {
            current.identity == identity
                && current.digest == file.base_digest
                && current.mode == file.mode
        }
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(AppError::Message(GUIDE_PREVIEW_STALE.to_owned()))
    }
}

pub(super) fn guide_line_counts(
    base: &str,
    output: &str,
    action: ProjectGuideFileActionV1,
) -> (usize, usize) {
    match action {
        ProjectGuideFileActionV1::Create => (output.lines().count(), 0),
        ProjectGuideFileActionV1::Update => (output.lines().count(), base.lines().count()),
        ProjectGuideFileActionV1::NoChange => (0, 0),
    }
}

pub(super) fn guide_diff(
    name: &str,
    base: &str,
    output: &str,
    action: ProjectGuideFileActionV1,
) -> AppResult<String> {
    if action == ProjectGuideFileActionV1::NoChange {
        return Ok(String::new());
    }
    let old_name = if action == ProjectGuideFileActionV1::Create {
        "/dev/null"
    } else {
        name
    };
    let mut diff = format!(
        "--- {old_name}\n+++ {name}\n@@ -1,{} +1,{} @@\n",
        base.lines().count(),
        output.lines().count()
    );
    for line in base.lines() {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in output.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    if diff.len() > GUIDE_DIFF_LIMIT {
        return Err(AppError::Message(
            "project guide preview diff exceeds its byte limit".to_owned(),
        ));
    }
    Ok(diff)
}
