//! The desktop initialization journal: its canonical on-disk form, the lock
//! that serializes writers, the checkpoint and guide-consent transition
//! rules, and the startup reconciliation of an interrupted initialization.

use std::fs::{self, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use ptrack_store::{
    CutoverLockMode, acquire_cutover_lock, open_private_path, protect_private_file,
    replace_private_file, sync_private_directory,
};
use serde::{Deserialize, Serialize};

use super::bootstrap::{
    ensure_private_directory, ensure_private_home, read_bootstrap_plan,
    selected_project_directory_present, validate_bootstrap_plan_intent,
};
use super::guide::{DesktopGuideManifest, validate_guide_manifest};
use super::{
    BOOTSTRAP_PLAN, DESKTOP_INITIALIZATION, DESKTOP_INITIALIZATION_LIMIT,
    DESKTOP_INITIALIZATION_LOCK, DESKTOP_INITIALIZATION_LOCK_TIMEOUT, GUIDE_PARTIALLY_APPLIED,
    GUIDE_PREVIEW_STALE, RECOVERY_REQUIRED, path_is_present, random_id, recovery,
    validate_operation_id,
};
use crate::{
    AppError, AppResult, InitializationCheckpointV1, InitializationOutcomeV1,
    InitializationStatusV1, ProjectGuideChoiceV1,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DesktopInitializationJournal {
    pub(super) format: String,
    pub(super) version: String,
    pub(super) status: InitializationStatusV1,
    pub(super) goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) guide: Option<DesktopGuideManifest>,
}

pub(super) fn read_desktop_initialization(
    home: &Path,
) -> AppResult<Option<DesktopInitializationJournal>> {
    let path = home.join("runtime").join(DESKTOP_INITIALIZATION);
    if !path_is_present(&path)? {
        return Ok(None);
    }
    let file = open_private_path(&path, false, false).map_err(recovery)?;
    let length = file.metadata()?.len();
    if length == 0 || length > DESKTOP_INITIALIZATION_LIMIT {
        return Err(recovery("desktop initialization status size is invalid"));
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(length)
            .map_err(|_| recovery("desktop initialization status is too large"))?,
    );
    file.take(DESKTOP_INITIALIZATION_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    let journal: DesktopInitializationJournal = serde_json::from_slice(&bytes)
        .map_err(|_| recovery("desktop initialization status is invalid"))?;
    if journal.format != "ptrack-desktop-initialization"
        || journal.version != "1"
        || canonical_desktop_initialization_bytes(&journal)? != bytes
    {
        return Err(recovery("desktop initialization status is not canonical"));
    }
    validate_operation_id(&journal.status.operation_id)?;
    let status_shape_is_valid = match journal.status.checkpoint {
        InitializationCheckpointV1::None => matches!(
            journal.status.outcome,
            InitializationOutcomeV1::Ready | InitializationOutcomeV1::InProgress
        ),
        InitializationCheckpointV1::Prepared
        | InitializationCheckpointV1::RuntimeCommitted
        | InitializationCheckpointV1::ProjectCommitted
        | InitializationCheckpointV1::GuideApplied => matches!(
            journal.status.outcome,
            InitializationOutcomeV1::InProgress | InitializationOutcomeV1::RecoveryRequired
        ),
        InitializationCheckpointV1::DesktopBound => {
            journal.status.outcome == InitializationOutcomeV1::Complete
        }
    };
    if journal.status.canonical_root.is_empty()
        || journal.status.canonical_root.len() > 4_096
        || !Path::new(&journal.status.canonical_root).is_absolute()
        || journal.status.error_kind.len() > 64
        || journal.goal.is_empty()
        || journal.goal.trim() != journal.goal
        || journal.goal.len() > 4_096
        || !status_shape_is_valid
    {
        return Err(recovery("desktop initialization status fields are invalid"));
    }
    if let Some(guide) = &journal.guide {
        validate_guide_manifest(guide)?;
        if guide.operation_id != journal.status.operation_id
            || guide.canonical_root != journal.status.canonical_root
        {
            return Err(recovery(
                "desktop initialization guide does not match its operation",
            ));
        }
    }
    Ok(Some(journal))
}

pub(super) fn publish_desktop_initialization(
    home: &Path,
    status: &InitializationStatusV1,
    goal: &str,
    guide: Option<&DesktopGuideManifest>,
) -> AppResult<()> {
    validate_operation_id(&status.operation_id)?;
    if goal.is_empty() || goal.trim() != goal || goal.len() > 4_096 {
        return Err(AppError::Message(
            "initialization goal is invalid".to_owned(),
        ));
    }
    ensure_private_home(home)?;
    let runtime = home.join("runtime");
    ensure_private_directory(&runtime)?;
    with_desktop_initialization_lock(home, || {
        if let Some(existing) = read_desktop_initialization(home)? {
            validate_desktop_initialization_transition(&existing.status, status)?;
            validate_guide_transition(existing.guide.as_ref(), guide, &existing.status, status)?;
        }
        publish_desktop_initialization_locked(home, status, goal, guide)
    })
}

pub(super) fn reconcile_startup_initialization(
    home: &Path,
    writer_version: &str,
    loaded: &mut DesktopInitializationJournal,
) -> AppResult<()> {
    if loaded.status.checkpoint != InitializationCheckpointV1::None
        || loaded.status.outcome == InitializationOutcomeV1::Complete
    {
        return Ok(());
    }
    let _lease = acquire_cutover_lock(home, CutoverLockMode::Shared).map_err(recovery)?;
    with_desktop_initialization_lock(home, || {
        let Some(mut current) = read_desktop_initialization(home)? else {
            return Err(recovery("desktop initialization status disappeared"));
        };
        if current.status.checkpoint != InitializationCheckpointV1::None
            || current.status.outcome == InitializationOutcomeV1::Complete
        {
            *loaded = current;
            return Ok(());
        }
        let project_storage_present =
            selected_project_directory_present(Path::new(&current.status.canonical_root));
        let bootstrap_present = path_is_present(&home.join("runtime").join(BOOTSTRAP_PLAN))?;
        if bootstrap_present {
            let canonical_home = fs::canonicalize(home)?;
            let plan = read_bootstrap_plan(&home.join("runtime").join(BOOTSTRAP_PLAN))?;
            if plan.operation_id.as_deref() != Some(current.status.operation_id.as_str())
                || plan.project_root != current.status.canonical_root
            {
                return Err(recovery(
                    "bootstrap plan is not bound to its initialization operation",
                ));
            }
            current.status.checkpoint = InitializationCheckpointV1::Prepared;
            current.status.outcome = InitializationOutcomeV1::RecoveryRequired;
            match fs::canonicalize(&current.status.canonical_root) {
                Ok(canonical_root) => {
                    validate_bootstrap_plan_intent(
                        &canonical_home,
                        &canonical_root,
                        &plan,
                        writer_version,
                    )?;
                    "interrupted-bootstrap-plan".clone_into(&mut current.status.error_kind);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    "project-not-found".clone_into(&mut current.status.error_kind);
                }
                Err(_) => "filesystem-error".clone_into(&mut current.status.error_kind),
            }
        } else if project_storage_present {
            current.status.checkpoint = InitializationCheckpointV1::Prepared;
            current.status.outcome = InitializationOutcomeV1::RecoveryRequired;
            "interrupted-project-storage".clone_into(&mut current.status.error_kind);
        } else if current.status.outcome == InitializationOutcomeV1::InProgress
            && !bootstrap_present
        {
            current.status.outcome = InitializationOutcomeV1::Ready;
            "interrupted-before-commit".clone_into(&mut current.status.error_kind);
        } else {
            *loaded = current;
            return Ok(());
        }
        publish_desktop_initialization_locked(
            home,
            &current.status,
            &current.goal,
            current.guide.as_ref(),
        )?;
        *loaded = current;
        Ok(())
    })
}

pub(super) fn publish_desktop_initialization_locked(
    home: &Path,
    status: &InitializationStatusV1,
    goal: &str,
    guide: Option<&DesktopGuideManifest>,
) -> AppResult<()> {
    let runtime = home.join("runtime");
    let path = runtime.join(DESKTOP_INITIALIZATION);
    if path_is_present(&path)? {
        drop(open_private_path(&path, false, true).map_err(recovery)?);
    }
    let journal = DesktopInitializationJournal {
        format: "ptrack-desktop-initialization".to_owned(),
        version: "1".to_owned(),
        status: status.clone(),
        goal: goal.to_owned(),
        guide: guide.cloned(),
    };
    let bytes = canonical_desktop_initialization_bytes(&journal)?;
    let temporary = runtime.join(format!(".{DESKTOP_INITIALIZATION}.{}.tmp", random_id()?));
    let result = (|| -> AppResult<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        protect_private_file(&temporary).map_err(recovery)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        replace_private_file(&temporary, &path).map_err(recovery)?;
        sync_private_directory(&runtime).map_err(recovery)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(super) fn with_desktop_initialization_lock<R>(
    home: &Path,
    operation: impl FnOnce() -> AppResult<R>,
) -> AppResult<R> {
    let path = home.join("runtime").join(DESKTOP_INITIALIZATION_LOCK);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    protect_private_file(&path).map_err(recovery)?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock)
                if started.elapsed() < DESKTOP_INITIALIZATION_LOCK_TIMEOUT =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(TryLockError::WouldBlock) => {
                return Err(recovery("desktop initialization status is busy"));
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    let result = operation();
    let unlock = file.unlock().map_err(AppError::Io);
    match (result, unlock) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(AppError::Message(format!("{first}\n{second}"))),
    }
}

pub(crate) fn validate_desktop_initialization_transition(
    existing: &InitializationStatusV1,
    incoming: &InitializationStatusV1,
) -> AppResult<()> {
    if existing.operation_id == incoming.operation_id {
        if existing.outcome == InitializationOutcomeV1::Complete
            && incoming.outcome != InitializationOutcomeV1::Complete
        {
            return Err(recovery(
                "desktop initialization status cannot regress from complete",
            ));
        }
        if initialization_checkpoint_rank(incoming.checkpoint)
            < initialization_checkpoint_rank(existing.checkpoint)
        {
            return Err(recovery("desktop initialization checkpoint cannot regress"));
        }
    } else if existing.outcome != InitializationOutcomeV1::Complete
        && (existing.checkpoint != InitializationCheckpointV1::None
            || incoming.checkpoint != InitializationCheckpointV1::None)
    {
        return Err(recovery(
            "a different initialization operation cannot replace durable progress",
        ));
    }
    Ok(())
}

pub(super) const fn initialization_checkpoint_rank(checkpoint: InitializationCheckpointV1) -> u8 {
    match checkpoint {
        InitializationCheckpointV1::None => 0,
        InitializationCheckpointV1::Prepared => 1,
        InitializationCheckpointV1::RuntimeCommitted => 2,
        InitializationCheckpointV1::ProjectCommitted => 3,
        InitializationCheckpointV1::GuideApplied => 4,
        InitializationCheckpointV1::DesktopBound => 5,
    }
}

pub(super) fn validate_guide_transition(
    existing: Option<&DesktopGuideManifest>,
    incoming: Option<&DesktopGuideManifest>,
    existing_status: &InitializationStatusV1,
    incoming_status: &InitializationStatusV1,
) -> AppResult<()> {
    if existing_status.operation_id != incoming_status.operation_id
        && existing_status.outcome == InitializationOutcomeV1::Complete
    {
        // A finished operation's manifest binds nothing in the next one, which
        // brings its own root, choice, and consent. An unfinished operation
        // still owns the journal: the status rule above refuses replacing it,
        // and a racing authority must reconcile the winner's manifest.
        return Ok(());
    }
    if existing.is_some_and(|existing| {
        incoming.is_some_and(|incoming| existing.root_identity != incoming.root_identity)
    }) {
        return Err(recovery(
            "desktop initialization project root identity cannot change",
        ));
    }
    match (existing, incoming) {
        (None, _) => Ok(()),
        (Some(_), Some(_)) if existing == incoming => Ok(()),
        (Some(_), None) => Err(recovery(
            "desktop initialization guide consent cannot be removed",
        )),
        (Some(existing), Some(_)) if existing.choice == ProjectGuideChoiceV1::Skip => {
            Err(recovery("skipped project guidance cannot be upgraded"))
        }
        (Some(existing), Some(incoming))
            if existing.choice == ProjectGuideChoiceV1::Install
                && incoming.choice == ProjectGuideChoiceV1::Skip
                && stale_guide_skip_allowed(existing_status)
                && incoming_status.checkpoint == existing_status.checkpoint =>
        {
            Ok(())
        }
        (Some(existing), Some(incoming))
            if existing.choice == ProjectGuideChoiceV1::Install
                && incoming.choice == ProjectGuideChoiceV1::Install
                && !matches!(
                    existing_status.checkpoint,
                    InitializationCheckpointV1::GuideApplied
                        | InitializationCheckpointV1::DesktopBound
                )
                && incoming_status.checkpoint == existing_status.checkpoint =>
        {
            Ok(())
        }
        (Some(existing), Some(incoming))
            if existing.choice == ProjectGuideChoiceV1::Install
                && incoming.choice == ProjectGuideChoiceV1::Install
                && existing_status.checkpoint == InitializationCheckpointV1::None
                && existing_status.outcome == InitializationOutcomeV1::Ready
                && existing_status.error_kind == GUIDE_PREVIEW_STALE
                && incoming_status.checkpoint == InitializationCheckpointV1::None =>
        {
            Ok(())
        }
        (Some(_), Some(_)) => Err(recovery(
            "desktop initialization guide consent is immutable",
        )),
    }
}

pub(crate) fn stale_guide_skip_allowed(status: &InitializationStatusV1) -> bool {
    (status.error_kind == GUIDE_PREVIEW_STALE
        || (status.error_kind == "interrupted-before-commit"
            && status.checkpoint == InitializationCheckpointV1::None
            && status.outcome == InitializationOutcomeV1::Ready))
        && match status.checkpoint {
            InitializationCheckpointV1::None => status.outcome == InitializationOutcomeV1::Ready,
            InitializationCheckpointV1::Prepared
            | InitializationCheckpointV1::RuntimeCommitted
            | InitializationCheckpointV1::ProjectCommitted => {
                status.outcome == InitializationOutcomeV1::RecoveryRequired
            }
            InitializationCheckpointV1::GuideApplied | InitializationCheckpointV1::DesktopBound => {
                false
            }
        }
}

pub(super) fn canonical_desktop_initialization_bytes(
    journal: &DesktopInitializationJournal,
) -> AppResult<Vec<u8>> {
    let mut bytes = serde_json::to_vec(journal)
        .map_err(|error| AppError::Message(format!("{RECOVERY_REQUIRED}: {error}")))?;
    bytes.push(b'\n');
    if bytes.len() as u64 > DESKTOP_INITIALIZATION_LIMIT {
        return Err(recovery(
            "desktop initialization status exceeds the fixed limit",
        ));
    }
    Ok(bytes)
}

pub(super) fn initialization_error_kind(error: &AppError) -> &'static str {
    match error {
        AppError::NoProject => "project-not-found",
        AppError::NotImplemented(_) => "unsupported",
        AppError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => "project-not-found",
        AppError::Io(_) => "filesystem-error",
        AppError::Message(message) if message == GUIDE_PREVIEW_STALE => GUIDE_PREVIEW_STALE,
        AppError::Message(message) if message == GUIDE_PARTIALLY_APPLIED => GUIDE_PARTIALLY_APPLIED,
        AppError::Message(message) if message.starts_with(RECOVERY_REQUIRED) => "recovery-required",
        AppError::Message(message)
            if message.contains("cutover")
                || message.contains("lock")
                || message.contains("busy") =>
        {
            "runtime-busy"
        }
        AppError::Message(_) | AppError::ScratchpadConflict(_) => "initialization-failed",
    }
}
