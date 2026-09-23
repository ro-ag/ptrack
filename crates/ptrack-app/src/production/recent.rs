//! The recent-projects registry: listing, relocation confirmations, the
//! replay-safe open, and forgetting an entry.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ptrack_core::ProjectRef;
use ptrack_store::{
    GlobalStore, ProjectRegistryCasResult, ProjectStore, StoreError, sha256_digest,
};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::runtime::ActiveRuntime;
use super::{
    RECENT_CONFIRMATION_LIMIT, RECENT_CONFIRMATION_TTL, RECENT_ID_BYTES, RECENT_LISTING_TTL,
    RECENT_PATH_LIMIT, lock, project_name, random_operation_id,
};
use crate::{
    AppError, AppResult, ForgetRecentProjectResultV1, ProjectEndpoint, RecentProjectAvailabilityV1,
    RecentProjectLanguageV1, RecentProjectOpenAuthorizationV1, RecentProjectRegistryCommitV1,
    RecentProjectRegistryStatusV1, RecentProjectResolutionV1, RecentProjectStackV1,
    RecentProjectV1, RecentProjectsProvider, RecentProjectsV1, ResolvedRecentProjectV1,
};

pub struct ProductionRecentProjects {
    runtime: Arc<ActiveRuntime>,
    confirmations: Mutex<BTreeMap<String, RecentLocationConfirmation>>,
    completed: Mutex<BTreeMap<String, CompletedRecentOpen>>,
    listed: Mutex<BTreeMap<String, ListedRecentEntry>>,
}

#[derive(Clone)]
struct RecentLocationConfirmation {
    source: ProjectRef,
    source_base: String,
    target_root: String,
    target_database: String,
    target_database_id: String,
    generation: String,
    expires_at: Instant,
}

#[derive(Clone)]
struct CompletedRecentOpen {
    authorization: RecentProjectOpenAuthorizationV1,
    commit: RecentProjectRegistryCommitV1,
    expires_at: Instant,
}

#[derive(Clone)]
struct ListedRecentEntry {
    project: ProjectRef,
    base: String,
    expires_at: Instant,
}

impl ProductionRecentProjects {
    #[must_use]
    pub fn new(runtime: Arc<ActiveRuntime>) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            confirmations: Mutex::new(BTreeMap::new()),
            completed: Mutex::new(BTreeMap::new()),
            listed: Mutex::new(BTreeMap::new()),
        })
    }

    fn global_store(&self) -> AppResult<GlobalStore> {
        let bindings = self
            .runtime
            .global_bindings(self.runtime.global_home())
            .map_err(|_| recent_projects_unavailable())?;
        GlobalStore::open_existing(&bindings.global_database, &bindings.global_binding)
            .map_err(|_| recent_projects_unavailable())
    }

    fn refresh_project_summary(&self, project: &ProjectRef) -> AppResult<()> {
        // Only a registry root that still has the exact active binding is eligible.
        if !self
            .runtime
            .marker()
            .projects
            .iter()
            .any(|entry| entry.root == project.path)
        {
            return Err(AppError::NoProject);
        }
        let bindings = self
            .runtime
            .bindings_for_exact_root(Path::new(&project.path))?;
        let endpoint = bindings.project.ok_or(AppError::NoProject)?;
        let pinned = crate::local_mode::pin(&endpoint.root)?;
        let store = ProjectStore::open_existing_pinned(
            &pinned,
            &endpoint.binding,
            &self.runtime.writer_version,
        )?;
        let snapshot = store.snapshot()?;
        drop(store);
        pinned.verify()?;
        crate::overview::write_project_summary(
            self.runtime.global_home(),
            &endpoint.root,
            &endpoint.binding.database_id,
            &snapshot,
        )
    }

    fn registry_entry(&self, entry_id: &str, base: &str) -> AppResult<ProjectRef> {
        validate_recent_id(entry_id)?;
        validate_recent_id(base)?;
        let listed = lock(&self.listed)
            .get(entry_id)
            .filter(|listed| listed.base == base && Instant::now() <= listed.expires_at)
            .cloned()
            .ok_or_else(recent_entry_stale)?;
        let project = self
            .global_store()?
            .project(&listed.project.path)
            .map_err(|_| recent_projects_unavailable())?
            .ok_or_else(recent_entry_stale)?;
        if recent_entry_base(&self.runtime, &project) != base || project != listed.project {
            return Err(recent_entry_stale());
        }
        Ok(project)
    }

    fn resolved_candidate(&self, candidate: &Path) -> AppResult<ProjectEndpoint> {
        let candidate = candidate.to_str().ok_or_else(recent_project_changed)?;
        if candidate.is_empty() || candidate.len() > RECENT_PATH_LIMIT {
            return Err(recent_project_changed());
        }
        let canonical = fs::canonicalize(candidate).map_err(|error| sanitize_recent_io(&error))?;
        if !canonical.is_dir() {
            return Err(recent_project_changed());
        }
        let bindings = self
            .runtime
            .bindings_for(&canonical)
            .map_err(sanitize_recent_app_error)?;
        let endpoint = bindings.project.ok_or_else(recent_project_changed)?;
        ProjectStore::open_existing(
            &endpoint.database,
            &endpoint.binding,
            &self.runtime.writer_version,
        )
        .map_err(sanitize_recent_store_error)?;
        Ok(endpoint)
    }

    fn exact_candidate(&self, candidate: &Path) -> AppResult<(PathBuf, ProjectEndpoint)> {
        let endpoint = self.resolved_candidate(candidate)?;
        let canonical = fs::canonicalize(candidate).map_err(|error| sanitize_recent_io(&error))?;
        if canonical != endpoint.root {
            return Err(recent_project_changed());
        }
        Ok((canonical, endpoint))
    }

    fn authorization_for(
        &self,
        entry_id: &str,
        base: &str,
        canonical_root: &Path,
        relocation_confirmation_token: &str,
    ) -> AppResult<RecentProjectOpenAuthorizationV1> {
        validate_recent_id(entry_id)?;
        validate_recent_id(base)?;
        let (canonical, endpoint) = self.exact_candidate(canonical_root)?;
        if canonical != canonical_root {
            return Err(recent_project_changed());
        }
        let canonical_root = canonical
            .to_str()
            .ok_or_else(recent_project_changed)?
            .to_owned();
        let replay_key = recent_open_key(
            entry_id,
            base,
            &canonical_root,
            relocation_confirmation_token,
        );
        if let Some(completed) = lock(&self.completed).get(&replay_key).cloned()
            && Instant::now() <= completed.expires_at
        {
            let mut authorization = completed.authorization;
            authorization.already_completed = true;
            return Ok(authorization);
        }
        let source = self.registry_entry(entry_id, base)?;
        if canonical_root == source.path {
            if !relocation_confirmation_token.is_empty() {
                return Err(recent_confirmation_invalid());
            }
        } else {
            validate_recent_id(relocation_confirmation_token)?;
            let confirmation = lock(&self.confirmations)
                .get(relocation_confirmation_token)
                .cloned()
                .ok_or_else(recent_confirmation_invalid)?;
            if confirmation.source != source
                || confirmation.source_base != base
                || confirmation.target_root != canonical_root
                || confirmation.target_database
                    != endpoint
                        .database
                        .to_str()
                        .ok_or_else(recent_project_changed)?
                || confirmation.target_database_id != endpoint.binding.database_id
                || confirmation.generation != self.runtime.marker.generation
                || Instant::now() > confirmation.expires_at
            {
                return Err(recent_confirmation_invalid());
            }
        }
        Ok(RecentProjectOpenAuthorizationV1 {
            entry_id: entry_id.to_owned(),
            base: base.to_owned(),
            canonical_root,
            name: project_name(&endpoint.root),
            relocation_confirmation_token: relocation_confirmation_token.to_owned(),
            already_completed: false,
        })
    }
}

impl RecentProjectsProvider for ProductionRecentProjects {
    fn global_overview_v1(&self) -> AppResult<crate::overview::GlobalOverviewV1> {
        let registered = self.global_store()?.projects()?;
        Ok(crate::overview::read_global_overview(
            self.runtime.global_home(),
            &registered,
            &self.runtime.marker().projects,
        ))
    }

    fn refresh_global_overview_v1(&self) -> AppResult<crate::overview::RefreshGlobalOverviewV1> {
        let registered = self.global_store()?.projects()?;
        let mut refreshed_projects = 0;
        let mut skipped_projects = 0;
        for project in &registered {
            if self.refresh_project_summary(project).is_ok() {
                refreshed_projects += 1;
            } else {
                skipped_projects += 1;
            }
        }
        Ok(crate::overview::RefreshGlobalOverviewV1 {
            overview: crate::overview::read_global_overview(
                self.runtime.global_home(),
                &registered,
                &self.runtime.marker().projects,
            ),
            refreshed_projects,
            skipped_projects,
        })
    }

    fn recent_projects(&self) -> AppResult<Vec<Value>> {
        Ok(self
            .recent_projects_v1()?
            .projects
            .into_iter()
            .map(|project| {
                json!({
                    "name": project.name,
                    "path": project.canonical_path,
                    "lastSeen": project.last_opened_at,
                    "available": project.availability == RecentProjectAvailabilityV1::Available
                })
            })
            .collect())
    }

    fn recent_projects_v1(&self) -> AppResult<RecentProjectsV1> {
        let store = self.global_store()?;
        let registered = store
            .recent_projects(20)
            .map_err(|_| recent_projects_unavailable())?;
        let projects = registered
            .iter()
            .map(|project| {
                if project.name.is_empty()
                    || project.name.len() > 4_096
                    || project.path.is_empty()
                    || project.path.len() > RECENT_PATH_LIMIT
                {
                    return Err(recent_projects_unavailable());
                }
                Ok(RecentProjectV1 {
                    entry_id: recent_entry_id(project),
                    base: recent_entry_base(&self.runtime, project),
                    name: project.name.clone(),
                    canonical_path: project.path.clone(),
                    last_opened_at: format_timestamp(project.last_seen),
                    availability: recent_availability(&self.runtime, project),
                    stack: project.stack.as_ref().map(|summary| RecentProjectStackV1 {
                        languages: summary
                            .languages
                            .iter()
                            .map(|(language, files)| RecentProjectLanguageV1 {
                                language: language.as_str().to_owned(),
                                files: *files,
                            })
                            .collect(),
                        tracked_files: summary.tracked_files,
                    }),
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        let expires_at = Instant::now() + RECENT_LISTING_TTL;
        let mut listed = lock(&self.listed);
        listed.retain(|_, entry| Instant::now() <= entry.expires_at);
        for project in registered {
            let entry_id = recent_entry_id(&project);
            listed.insert(
                entry_id,
                ListedRecentEntry {
                    base: recent_entry_base(&self.runtime, &project),
                    project,
                    expires_at,
                },
            );
        }
        while listed.len() > RECENT_CONFIRMATION_LIMIT {
            let Some(oldest) = listed
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(entry_id, _)| entry_id.clone())
            else {
                break;
            };
            listed.remove(&oldest);
        }
        drop(listed);
        Ok(RecentProjectsV1 { projects })
    }

    fn resolve_recent_project(
        &self,
        entry_id: &str,
        base: &str,
        candidate: &Path,
    ) -> AppResult<ResolvedRecentProjectV1> {
        let source = self.registry_entry(entry_id, base)?;
        let endpoint = self.resolved_candidate(candidate)?;
        let canonical_root = endpoint
            .root
            .to_str()
            .ok_or_else(recent_project_changed)?
            .to_owned();
        let (resolution, confirmation_token) = if canonical_root == source.path {
            (RecentProjectResolutionV1::Ready, String::new())
        } else {
            let token = random_operation_id().map_err(|_| {
                AppError::Message("recent-project confirmation is unavailable".to_owned())
            })?;
            let confirmation = RecentLocationConfirmation {
                source,
                source_base: base.to_owned(),
                target_root: canonical_root.clone(),
                target_database: endpoint
                    .database
                    .to_str()
                    .ok_or_else(recent_project_changed)?
                    .to_owned(),
                target_database_id: endpoint.binding.database_id.clone(),
                generation: self.runtime.marker.generation.clone(),
                expires_at: Instant::now() + RECENT_CONFIRMATION_TTL,
            };
            let mut confirmations = lock(&self.confirmations);
            confirmations.retain(|_, value| Instant::now() <= value.expires_at);
            if confirmations.len() >= RECENT_CONFIRMATION_LIMIT
                && let Some(oldest) = confirmations
                    .iter()
                    .min_by_key(|(_, value)| value.expires_at)
                    .map(|(token, _)| token.clone())
            {
                confirmations.remove(&oldest);
            }
            confirmations.insert(token.clone(), confirmation);
            drop(confirmations);
            (RecentProjectResolutionV1::ConfirmationRequired, token)
        };
        Ok(ResolvedRecentProjectV1 {
            entry_id: entry_id.to_owned(),
            base: base.to_owned(),
            canonical_root,
            name: project_name(&endpoint.root),
            resolution,
            confirmation_token,
        })
    }

    fn authorize_recent_project_open(
        &self,
        entry_id: &str,
        base: &str,
        canonical_root: &Path,
        relocation_confirmation_token: &str,
    ) -> AppResult<RecentProjectOpenAuthorizationV1> {
        self.authorization_for(
            entry_id,
            base,
            canonical_root,
            relocation_confirmation_token,
        )
    }

    fn finish_recent_project_open(
        &self,
        authorization: &RecentProjectOpenAuthorizationV1,
    ) -> AppResult<RecentProjectRegistryCommitV1> {
        let replay_key = recent_open_key(
            &authorization.entry_id,
            &authorization.base,
            &authorization.canonical_root,
            &authorization.relocation_confirmation_token,
        );
        if authorization.already_completed
            && let Some(completed) = lock(&self.completed).get(&replay_key).cloned()
            && Instant::now() <= completed.expires_at
        {
            return Ok(completed.commit);
        }
        let commit = (|| {
            let confirmed = self.authorization_for(
                &authorization.entry_id,
                &authorization.base,
                Path::new(&authorization.canonical_root),
                &authorization.relocation_confirmation_token,
            )?;
            let expected = self.registry_entry(&authorization.entry_id, &authorization.base)?;
            let same_path = confirmed.canonical_root == expected.path;
            let result = self
                .global_store()?
                .relocate_project_if_matches(
                    &expected,
                    &authorization.name,
                    &authorization.canonical_root,
                )
                .map_err(|_| recent_projects_unavailable())?;
            Ok(match result {
                ProjectRegistryCasResult::Applied(project) => RecentProjectRegistryCommitV1 {
                    base: recent_entry_base(&self.runtime, &project),
                    status: if same_path {
                        RecentProjectRegistryStatusV1::Unchanged
                    } else {
                        RecentProjectRegistryStatusV1::Relocated
                    },
                },
                ProjectRegistryCasResult::Absent | ProjectRegistryCasResult::Stale => {
                    RecentProjectRegistryCommitV1 {
                        base: authorization.base.clone(),
                        status: RecentProjectRegistryStatusV1::Stale,
                    }
                }
            })
        })()
        .unwrap_or_else(|_: AppError| RecentProjectRegistryCommitV1 {
            base: authorization.base.clone(),
            status: RecentProjectRegistryStatusV1::Stale,
        });
        if !authorization.relocation_confirmation_token.is_empty() {
            lock(&self.confirmations).remove(&authorization.relocation_confirmation_token);
        }
        let mut completed = lock(&self.completed);
        completed.retain(|_, value| Instant::now() <= value.expires_at);
        if completed.len() >= RECENT_CONFIRMATION_LIMIT
            && let Some(oldest) = completed
                .iter()
                .min_by_key(|(_, value)| value.expires_at)
                .map(|(key, _)| key.clone())
        {
            completed.remove(&oldest);
        }
        completed.insert(
            replay_key,
            CompletedRecentOpen {
                authorization: authorization.clone(),
                commit: commit.clone(),
                expires_at: Instant::now() + RECENT_CONFIRMATION_TTL,
            },
        );
        drop(completed);
        Ok(commit)
    }

    fn forget_recent_project(
        &self,
        entry_id: &str,
        base: &str,
    ) -> AppResult<ForgetRecentProjectResultV1> {
        validate_recent_id(entry_id)?;
        validate_recent_id(base)?;
        let listed = lock(&self.listed)
            .get(entry_id)
            .filter(|listed| listed.base == base && Instant::now() <= listed.expires_at)
            .cloned()
            .ok_or_else(recent_entry_stale)?;
        let store = self.global_store()?;
        let current = store
            .project(&listed.project.path)
            .map_err(|_| recent_projects_unavailable())?;
        let Some(current) = current else {
            return Ok(ForgetRecentProjectResultV1 {
                entry_id: entry_id.to_owned(),
                registry_base: base.to_owned(),
                forgotten: true,
            });
        };
        if recent_entry_base(&self.runtime, &current) != base || current != listed.project {
            return Err(recent_entry_stale());
        }
        match store
            .forget_project_if_matches(&current)
            .map_err(|_| recent_projects_unavailable())?
        {
            ProjectRegistryCasResult::Applied(_) | ProjectRegistryCasResult::Absent => {
                Ok(ForgetRecentProjectResultV1 {
                    entry_id: entry_id.to_owned(),
                    registry_base: base.to_owned(),
                    forgotten: true,
                })
            }
            ProjectRegistryCasResult::Stale => Err(recent_entry_stale()),
        }
    }
}

fn recent_entry_id(project: &ProjectRef) -> String {
    URL_SAFE_NO_PAD.encode(sha256_digest(project.path.as_bytes()))
}

fn recent_open_key(
    entry_id: &str,
    base: &str,
    canonical_root: &str,
    relocation_confirmation_token: &str,
) -> String {
    let mut bytes = Vec::new();
    push_recent_field(&mut bytes, entry_id.as_bytes());
    push_recent_field(&mut bytes, base.as_bytes());
    push_recent_field(&mut bytes, canonical_root.as_bytes());
    push_recent_field(&mut bytes, relocation_confirmation_token.as_bytes());
    URL_SAFE_NO_PAD.encode(sha256_digest(&bytes))
}

fn recent_entry_base(runtime: &ActiveRuntime, project: &ProjectRef) -> String {
    let mut bytes = Vec::new();
    push_recent_field(&mut bytes, runtime.marker.generation.as_bytes());
    push_recent_field(&mut bytes, project.name.as_bytes());
    push_recent_field(&mut bytes, project.path.as_bytes());
    if let Some(mapped) = runtime
        .marker
        .projects
        .iter()
        .find(|mapped| mapped.root == project.path)
    {
        push_recent_field(&mut bytes, mapped.database_id.as_bytes());
        push_recent_field(&mut bytes, mapped.path.as_bytes());
    } else {
        push_recent_field(&mut bytes, b"unmapped");
    }
    match project.last_seen {
        ptrack_core::Timestamp::Zero => bytes.push(0),
        ptrack_core::Timestamp::Fixed {
            seconds,
            nanoseconds,
            offset_seconds,
        } => {
            bytes.push(1);
            bytes.extend_from_slice(&seconds.to_le_bytes());
            bytes.extend_from_slice(&nanoseconds.to_le_bytes());
            bytes.extend_from_slice(&offset_seconds.to_le_bytes());
        }
    }
    URL_SAFE_NO_PAD.encode(sha256_digest(&bytes))
}

fn push_recent_field(bytes: &mut Vec<u8>, field: &[u8]) {
    bytes.extend_from_slice(&u64::try_from(field.len()).unwrap_or(u64::MAX).to_le_bytes());
    bytes.extend_from_slice(field);
}

fn validate_recent_id(value: &str) -> AppResult<()> {
    if value.len() == RECENT_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err(AppError::Message(
            "recent-project identity is invalid".to_owned(),
        ))
    }
}

fn recent_entry_stale() -> AppError {
    AppError::Message("recent-project-entry-stale".to_owned())
}

fn recent_projects_unavailable() -> AppError {
    AppError::Message("recent-projects-unavailable".to_owned())
}

fn recent_confirmation_invalid() -> AppError {
    AppError::Message("recent-project-confirmation-invalid".to_owned())
}

fn recent_project_changed() -> AppError {
    AppError::Message("recent-project-changed".to_owned())
}

fn recent_project_missing() -> AppError {
    AppError::Message("recent-project-folder-not-found".to_owned())
}

fn recent_project_permission() -> AppError {
    AppError::Message("recent-project-permission-required".to_owned())
}

fn sanitize_recent_io(error: &std::io::Error) -> AppError {
    match error.kind() {
        ErrorKind::NotFound => recent_project_missing(),
        ErrorKind::PermissionDenied => recent_project_permission(),
        _ => recent_project_changed(),
    }
}

fn sanitize_recent_store_error(error: StoreError) -> AppError {
    match error {
        StoreError::Io(error) if error.kind() == ErrorKind::PermissionDenied => {
            recent_project_permission()
        }
        _ => recent_project_changed(),
    }
}

fn sanitize_recent_app_error(error: AppError) -> AppError {
    match error {
        AppError::Io(error) => sanitize_recent_io(&error),
        AppError::NoProject
        | AppError::NotImplemented(_)
        | AppError::Message(_)
        | AppError::ScratchpadConflict(_) => recent_project_changed(),
    }
}

fn recent_availability(
    runtime: &ActiveRuntime,
    project: &ProjectRef,
) -> RecentProjectAvailabilityV1 {
    let canonical = match fs::canonicalize(&project.path) {
        Ok(canonical) => canonical,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return RecentProjectAvailabilityV1::Missing;
        }
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            return RecentProjectAvailabilityV1::PermissionRequired;
        }
        Err(_) => return RecentProjectAvailabilityV1::Changed,
    };
    let Some(mapped) = runtime
        .marker
        .projects
        .iter()
        .find(|mapped| mapped.root == project.path)
    else {
        return RecentProjectAvailabilityV1::Changed;
    };
    if canonical != Path::new(&mapped.root) || !canonical.is_dir() {
        return RecentProjectAvailabilityV1::Changed;
    }
    let Ok(binding) = runtime.marker.project_binding(mapped) else {
        return RecentProjectAvailabilityV1::Changed;
    };
    match ProjectStore::open_existing(&mapped.path, &binding, &runtime.writer_version) {
        Ok(_) => RecentProjectAvailabilityV1::Available,
        Err(StoreError::Io(error)) if error.kind() == ErrorKind::PermissionDenied => {
            RecentProjectAvailabilityV1::PermissionRequired
        }
        Err(_) => RecentProjectAvailabilityV1::Changed,
    }
}

fn format_timestamp(value: ptrack_core::Timestamp) -> String {
    let ptrack_core::Timestamp::Fixed {
        seconds,
        nanoseconds,
        ..
    } = value
    else {
        return "0001-01-01T00:00:00Z".to_owned();
    };
    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .and_then(|value| value.replace_nanosecond(nanoseconds).ok())
        .and_then(|value| value.format(&Rfc3339).ok())
        .unwrap_or_default()
}
