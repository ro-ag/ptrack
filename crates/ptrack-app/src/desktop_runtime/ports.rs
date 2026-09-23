//! The services the desktop coordinator is built from: the event plane, the
//! workspace and its factory, the recent-projects registry, and first-run
//! initialization, with the inert implementations an unconfigured runtime
//! uses.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::DEFAULT_CONFIRMATION_TTL;
use super::admission::DesktopAdmissionFence;
use super::support::unavailable;
use super::wire::{
    ActiveResourceSummary, DesktopEvent, DesktopNotificationSnapshotV1,
    ForgetRecentProjectResultV1, InitializationStatusV1, InitializeProjectRequestV1,
    PendingInitializationV1, ProjectGuidePreviewRequestV1, ProjectGuidePreviewV1,
    ProjectTargetValidationV1, RecentProjectOpenAuthorizationV1, RecentProjectRegistryCommitV1,
    RecentProjectsV1, ResolvedRecentProjectV1, WorkspaceProject,
};
use crate::diagnostics_report::CapabilityCountsV1;
use crate::{AppError, AppResult};

pub trait DesktopEventSink: Send + Sync {
    fn emit(&self, event: DesktopEvent);
}

/// Adapts terminal lifecycle events into the desktop event plane without
/// granting the native shell access to the terminal manager.
pub struct DesktopTerminalEventSink {
    sink: Arc<dyn DesktopEventSink>,
}

impl DesktopTerminalEventSink {
    #[must_use]
    pub fn new(sink: Arc<dyn DesktopEventSink>) -> Arc<Self> {
        Arc::new(Self { sink })
    }
}

impl crate::TerminalEventSink for DesktopTerminalEventSink {
    fn status(&self, event: crate::TerminalStatusV2) {
        self.sink.emit(DesktopEvent::TerminalStatus(event));
    }

    fn exited(&self, event: crate::TerminalExitV2) {
        self.sink.emit(DesktopEvent::TerminalExit(event));
    }

    fn runtime_changed(&self, generation: u64) {
        self.sink
            .emit(DesktopEvent::WorkspaceRuntimeChanged(generation));
    }
}

/// Adapts updater state publication into the one-way desktop event plane.
pub struct DesktopUpdateEventSink {
    sink: Arc<dyn DesktopEventSink>,
}

impl DesktopUpdateEventSink {
    #[must_use]
    pub fn new(sink: Arc<dyn DesktopEventSink>) -> Arc<Self> {
        Arc::new(Self { sink })
    }
}

impl crate::UpdateEventSink for DesktopUpdateEventSink {
    fn state_changed(&self, state: crate::UpdateState) {
        if let Ok(value) = serde_json::to_value(state) {
            self.sink.emit(DesktopEvent::UpdateStateChanged(value));
        }
    }
}

/// Generation-owned workspace service. Implementations own all database,
/// terminal, agent, and watcher authority; native shells receive
/// only this command surface.
#[allow(clippy::missing_errors_doc)]
pub trait DesktopWorkspace: Send + Sync {
    fn project(&self) -> WorkspaceProject;
    fn invoke(&self, method: &str, arguments: &[Value]) -> AppResult<Value>;
    fn active_resources(&self) -> AppResult<ActiveResourceSummary>;
    fn fence_resource_admission(&self) -> AppResult<DesktopAdmissionFence> {
        Ok(DesktopAdmissionFence::empty())
    }
    fn drain_runtime_invalidations(&self) -> AppResult<bool> {
        Ok(false)
    }
    fn notification_snapshot(&self) -> AppResult<DesktopNotificationSnapshotV1> {
        Ok(DesktopNotificationSnapshotV1::default())
    }
    /// Counts the capability records an older build left in the project.
    /// Absent when the workspace cannot answer for them.
    fn capability_counts(&self) -> Option<CapabilityCountsV1> {
        None
    }
    /// Revokes every capability grant an older build left in the project and
    /// removes its broker descriptor, returning how many grants went.
    fn revoke_capability_grants(&self) -> AppResult<usize> {
        Ok(0)
    }
    fn shutdown(&self) -> AppResult<()>;
}

#[allow(clippy::missing_errors_doc)]
pub trait DesktopWorkspaceFactory: Send + Sync {
    fn build(&self, root: &Path, generation: u64) -> AppResult<Arc<dyn DesktopWorkspace>>;
}

#[allow(clippy::missing_errors_doc)]
pub trait RecentProjectsProvider: Send + Sync {
    fn recent_projects(&self) -> AppResult<Vec<Value>>;

    fn global_overview_v1(&self) -> AppResult<crate::overview::GlobalOverviewV1> {
        Ok(crate::overview::GlobalOverviewV1::default())
    }

    fn refresh_global_overview_v1(&self) -> AppResult<crate::overview::RefreshGlobalOverviewV1> {
        Ok(crate::overview::RefreshGlobalOverviewV1::default())
    }

    fn recent_projects_v1(&self) -> AppResult<RecentProjectsV1> {
        Ok(RecentProjectsV1 {
            projects: Vec::new(),
        })
    }

    fn resolve_recent_project(
        &self,
        _entry_id: &str,
        _base: &str,
        _candidate: &Path,
    ) -> AppResult<ResolvedRecentProjectV1> {
        Err(unavailable("recent-project recovery"))
    }

    fn authorize_recent_project_open(
        &self,
        _entry_id: &str,
        _base: &str,
        _canonical_root: &Path,
        _relocation_confirmation_token: &str,
    ) -> AppResult<RecentProjectOpenAuthorizationV1> {
        Err(unavailable("recent-project recovery"))
    }

    fn finish_recent_project_open(
        &self,
        _authorization: &RecentProjectOpenAuthorizationV1,
    ) -> AppResult<RecentProjectRegistryCommitV1> {
        Err(unavailable("recent-project recovery"))
    }

    fn forget_recent_project(
        &self,
        _entry_id: &str,
        _base: &str,
    ) -> AppResult<ForgetRecentProjectResultV1> {
        Err(unavailable("recent-project recovery"))
    }
}

#[allow(clippy::missing_errors_doc)]
pub trait DesktopInitializationService: Send + Sync {
    fn validate_target(&self, selected: &Path) -> AppResult<ProjectTargetValidationV1>;
    fn preview_guide(
        &self,
        _request: &ProjectGuidePreviewRequestV1,
    ) -> AppResult<ProjectGuidePreviewV1> {
        Ok(project_guide_unavailable())
    }
    fn initialize(&self, request: &InitializeProjectRequestV1)
    -> AppResult<InitializationStatusV1>;
    fn status(&self, operation_id: &str) -> AppResult<InitializationStatusV1>;
    fn pending(&self) -> AppResult<PendingInitializationV1> {
        Ok(PendingInitializationV1 {
            pending: false,
            initialization: None,
            validation: None,
        })
    }
    fn completed_initialization(&self) -> AppResult<Option<InitializationStatusV1>> {
        Ok(None)
    }
    fn mark_desktop_bound(&self, operation_id: &str) -> AppResult<InitializationStatusV1>;
}

#[derive(Default)]
pub struct NoRecentProjectsProvider;

impl RecentProjectsProvider for NoRecentProjectsProvider {
    fn recent_projects(&self) -> AppResult<Vec<Value>> {
        Ok(Vec::new())
    }
}

#[derive(Default)]
pub struct NoDesktopWorkspaceFactory;

impl DesktopWorkspaceFactory for NoDesktopWorkspaceFactory {
    fn build(&self, _root: &Path, _generation: u64) -> AppResult<Arc<dyn DesktopWorkspace>> {
        Err(AppError::Message(
            "active runtime binding is unavailable".to_owned(),
        ))
    }
}

#[derive(Default)]
pub struct NoDesktopInitializationService;

impl DesktopInitializationService for NoDesktopInitializationService {
    fn validate_target(&self, _selected: &Path) -> AppResult<ProjectTargetValidationV1> {
        Err(unavailable("project initialization"))
    }

    fn initialize(
        &self,
        _request: &InitializeProjectRequestV1,
    ) -> AppResult<InitializationStatusV1> {
        Err(unavailable("project initialization"))
    }

    fn preview_guide(
        &self,
        _request: &ProjectGuidePreviewRequestV1,
    ) -> AppResult<ProjectGuidePreviewV1> {
        Ok(project_guide_unavailable())
    }

    fn status(&self, _operation_id: &str) -> AppResult<InitializationStatusV1> {
        Err(unavailable("project initialization"))
    }

    fn mark_desktop_bound(&self, _operation_id: &str) -> AppResult<InitializationStatusV1> {
        Err(unavailable("project initialization"))
    }
}

pub struct DesktopRuntimeConfig {
    pub version: String,
    pub factory: Arc<dyn DesktopWorkspaceFactory>,
    pub event_sink: Option<Arc<dyn DesktopEventSink>>,
    pub initial_workspace: Option<Arc<dyn DesktopWorkspace>>,
    pub recent_projects: Arc<dyn RecentProjectsProvider>,
    pub initialization: Arc<dyn DesktopInitializationService>,
    pub update_service: Arc<dyn crate::DesktopUpdateService>,
    pub confirmation_ttl: Duration,
}

impl DesktopRuntimeConfig {
    #[must_use]
    pub fn unavailable(version: impl Into<String>) -> Self {
        let version = version.into();
        Self {
            version: version.clone(),
            factory: Arc::new(NoDesktopWorkspaceFactory),
            event_sink: None,
            initial_workspace: None,
            recent_projects: Arc::new(NoRecentProjectsProvider),
            initialization: Arc::new(NoDesktopInitializationService),
            update_service: crate::UnavailableUpdateService::new(version),
            confirmation_ttl: DEFAULT_CONFIRMATION_TTL,
        }
    }
}

fn project_guide_unavailable() -> ProjectGuidePreviewV1 {
    ProjectGuidePreviewV1 {
        available: false,
        message: "Project guidance is not available on this platform yet".to_owned(),
        preview_token: String::new(),
        files: Vec::new(),
    }
}
