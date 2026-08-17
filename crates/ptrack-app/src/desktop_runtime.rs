use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ptrack_agent::{
    AgentRuntimeSummary, AgentWorkflowKind, AssociationCatalog, AssociationHost,
    AssociationPointer as AgentAssociationPointer, BoundedItems, LaunchContextStore, LeaseState,
    ProcessState, RegistrationKind, Run, RunState, RuntimeAssociation, ScanBoundedItems,
    build_launch_context, contains_potential_credential,
};
use ptrack_capability::{Broker, ConnectionDiagnostic, ConnectionTester};
use ptrack_capability_policy::{
    CapabilityAuditWire, CapabilityDraftWire, CapabilityWire, confirm_approval, normalize,
};
use ptrack_core::{
    Capability, CapabilityKind, Commit, Issue, IssueStatus, MemoryKind, Meta, Note, NoteTarget,
    Plan, ProjectSnapshot, Task, TaskStatus, Timestamp,
};
use ptrack_store::{
    FIRST_RUN_TITLE_MAX_BYTES, GlobalStore, MemoryWriteRequest, ProjectStore, StoreError,
    find_project_database,
};
use ptrack_terminal::{SessionInfo, SessionState};
use ptrack_terminal::{TerminalAssociation, TerminalAssociationPointer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};
use tokio_util::sync::CancellationToken;

use crate::diagnostics_report::{CapabilityCountsV1, DiagnosticsReportV1};
use crate::layout_state::{layout_state, reset_window_layout, set_layout_state};
use crate::preferences::{PreferencesDocumentV1, preferences, reset_preferences, set_preferences};
use crate::terminal_windows::{TerminalWindowTab, TerminalWindows};
use crate::{
    ActiveRuntime, AgentRuntimeService, AppError, AppResult, ApplicationPort,
    LaunchedEventAuthority, LinkedAgentRuntimeHooks, Mutation, MutationResult, ProjectEndpoint,
    TerminalRuntime, WorkspaceBindings,
};

const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const DEFAULT_CONFIRMATION_TTL: Duration = Duration::from_secs(60);
const RUNTIME_CALL_TIMEOUT: Duration = Duration::from_millis(250);
const RECENT_PROJECT_LIMIT: usize = 20;
const RECENT_PROJECT_PATH_LIMIT: usize = 16 * 1024;
const RECENT_PROJECT_TOKEN_BYTES: usize = 43;
const SEARCH_RESULT_LIMIT: usize = 50;
const SEARCH_SNIPPET_SPAN: usize = 60;
const TASK_CONFIRMATION_TTL: Duration = Duration::from_secs(90);
const TASK_CONFIRMATION_LIMIT: usize = 64;
const TASK_RESOURCE_LIMIT: usize = 1_024;
const WORKSPACE_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(8);
const SNAPSHOT_PLAN_LIMIT: usize = 100;
const SNAPSHOT_TASK_LIMIT: usize = 300;
const SNAPSHOT_BLOCKER_LIMIT: usize = 50;
const SNAPSHOT_NOTE_LIMIT: usize = 50;
const SNAPSHOT_ISSUE_LIMIT: usize = 50;
const SNAPSHOT_ACTIVITY_LIMIT: usize = 24;
const SNAPSHOT_RUNTIME_LIMIT: usize = 64;
const WORKSPACE_WATCH_INTERVAL: Duration = Duration::from_secs(2);
const WORKSPACE_WATCH_DEBOUNCE: Duration = Duration::from_millis(500);
const WORKSPACE_OPERATION_DRAIN_TIMEOUT: Duration = Duration::from_secs(3);

pub const FIRST_RUN_GOAL_MAX_BYTES: usize = 4_096;

const COMMANDS: [&str; 88] = [
    "AcknowledgeAgentHandoffV2",
    "AddTask",
    "AddTaskNote",
    "AddTaskNoteV2",
    "AddTaskV2",
    "ApplyUpdate",
    "ApproveAgentWorkflowV2",
    "AssociateAgentRunV2",
    "AssociateTerminalV2",
    "CancelUpdateOperation",
    "CancelWorkspaceChange",
    "CheckForUpdates",
    "ClaimTerminalStream",
    "CloseProject",
    "CloseTerminal",
    "CloseTerminalV2",
    "CreateFirstPlanV1",
    "CreateFirstTaskV1",
    "CreateTerminal",
    "CreateTerminalV2",
    "DisableCapabilityV2",
    "DismissAgentWorkflowV2",
    "DownloadUpdate",
    "EnableCapabilityV2",
    "ExpireCapabilityV2",
    "ForgetRecentProjectV1",
    "GetActivityHeatmapV2",
    "GetAgentIntelligenceV2",
    "GetAgentRunsV2",
    "GetBoard",
    "GetBoardV2",
    "GetCapabilitiesV2",
    "GetCapabilityAuditsV2",
    "GetDiagnosticsReport",
    "GetInitializationStatusV1",
    "GetLayoutState",
    "GetPendingInitializationV1",
    "GetPreferences",
    "GetRecentProjects",
    "GetRecentProjectsV1",
    "GetTaskDetailV2",
    "GetTerminalProfiles",
    "GetTerminalProfilesV2",
    "GetTerminalWindowTab",
    "GetUpdateState",
    "GetWorkspaceSnapshot",
    "GetWorkspaceState",
    "InitializeProjectV1",
    "InstallShellCommand",
    "LaunchLinkedAgentV2",
    "MoveTask",
    "MoveTaskV2",
    "MoveTaskV3",
    "MutateTerminalAssociationV2",
    "OpenHelpDestination",
    "OpenProject",
    "OpenRecentProjectV1",
    "OpenTerminalWindow",
    "PickProjectDirectory",
    "PrepareAgentWorkflowV2",
    "PreviewAgentHandoffV2",
    "PreviewCapabilityV2",
    "PreviewProjectGuideV1",
    "PreviewTerminalWritebackV2",
    "RemoveCapabilityV2",
    "RenameTask",
    "RenameTaskV2",
    "ResetApplicationState",
    "ResetPreferences",
    "ResetWindowLayout",
    "ResizeTerminal",
    "ResizeTerminalV2",
    "ResolveRecentProjectV1",
    "RollbackLinkedAgentLaunchV2",
    "SaveCapabilityV2",
    "SearchV2",
    "SendAgentHandoffV2",
    "SetAgentTaskOwnershipV2",
    "SetAgentWorktreeV2",
    "SetAutomaticUpdateChecks",
    "SetLayoutState",
    "SetPreferences",
    "SetTerminalWindowTab",
    "StartFirstTaskV1",
    "TestCapabilityV2",
    "ValidateProjectTargetV1",
    "ValidateTerminalCWDsV2",
    "WriteTerminalMemoryV2",
];

/// Exact current 88-method desktop bridge command allowlist.
#[must_use]
pub const fn allowed_desktop_commands() -> &'static [&'static str] {
    &COMMANDS
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopCommandRequest {
    pub method: String,
    #[serde(default)]
    pub arguments: Vec<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectTargetKindV1 {
    New,
    Existing,
    RecoveryRequired,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTargetValidationV1 {
    pub kind: ProjectTargetKindV1,
    pub canonical_root: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub operation_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initialization: Option<InitializationStatusV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guide_choice: Option<ProjectGuideChoiceV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingInitializationV1 {
    pub pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initialization: Option<InitializationStatusV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation: Option<ProjectTargetValidationV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitializeProjectRequestV1 {
    pub operation_id: String,
    pub root: String,
    pub goal: String,
    #[serde(default)]
    pub guide_choice: ProjectGuideChoiceV1,
    #[serde(default)]
    pub guide_preview_token: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectGuideChoiceV1 {
    #[default]
    Skip,
    Install,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectGuidePreviewRequestV1 {
    pub operation_id: String,
    pub root: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectGuideFileActionV1 {
    Create,
    Update,
    NoChange,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGuideFilePreviewV1 {
    pub path: String,
    pub action: ProjectGuideFileActionV1,
    pub additions: usize,
    pub deletions: usize,
    pub diff: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGuidePreviewV1 {
    pub available: bool,
    pub message: String,
    pub preview_token: String,
    pub files: Vec<ProjectGuideFilePreviewV1>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InitializationCheckpointV1 {
    None,
    Prepared,
    RuntimeCommitted,
    ProjectCommitted,
    GuideApplied,
    DesktopBound,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InitializationOutcomeV1 {
    Ready,
    InProgress,
    RecoveryRequired,
    Complete,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitializationStatusV1 {
    pub operation_id: String,
    pub canonical_root: String,
    pub checkpoint: InitializationCheckpointV1,
    pub outcome: InitializationOutcomeV1,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeProjectResultV1 {
    pub initialization: InitializationStatusV1,
    pub state: WorkspaceState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstPlanV1 {
    pub id: u64,
    pub title: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstTaskV1 {
    pub id: u64,
    pub plan_id: u64,
    pub title: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstRunWorkspaceStateV1 {
    pub status: WorkspaceStatus,
    pub generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFirstPlanResultV1 {
    pub plan: FirstPlanV1,
    pub state: FirstRunWorkspaceStateV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFirstTaskResultV1 {
    pub task: FirstTaskV1,
    pub state: FirstRunWorkspaceStateV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceStatus {
    Welcome,
    Loading,
    Open,
    Error,
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProject {
    pub name: String,
    pub root: String,
    pub db_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceState {
    pub status: WorkspaceStatus,
    pub generation: u64,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<WorkspaceProject>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveResourceSummary {
    pub terminals: usize,
    pub agent_runs: usize,
    pub pending_admissions: usize,
    pub resource_revision: u64,
}

impl ActiveResourceSummary {
    const fn requires_confirmation(self) -> bool {
        self.terminals != 0 || self.agent_runs != 0 || self.pending_admissions != 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChangeResult {
    pub state: WorkspaceState,
    pub requires_confirmation: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub confirmation_token: String,
    pub active_resources: ActiveResourceSummary,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub warning: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecentProjectAvailabilityV1 {
    Available,
    Missing,
    PermissionRequired,
    Changed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProjectV1 {
    pub entry_id: String,
    pub base: String,
    pub name: String,
    pub canonical_path: String,
    pub last_opened_at: String,
    pub availability: RecentProjectAvailabilityV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProjectsV1 {
    pub projects: Vec<RecentProjectV1>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecentProjectResolutionV1 {
    Ready,
    ConfirmationRequired,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRecentProjectV1 {
    pub entry_id: String,
    pub base: String,
    pub canonical_root: String,
    pub name: String,
    pub resolution: RecentProjectResolutionV1,
    pub confirmation_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecentProjectOpenAuthorizationV1 {
    pub entry_id: String,
    pub base: String,
    pub canonical_root: String,
    pub name: String,
    pub relocation_confirmation_token: String,
    pub already_completed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecentProjectRegistryStatusV1 {
    Unchanged,
    Relocated,
    Stale,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecentProjectRegistryCommitV1 {
    pub base: String,
    pub status: RecentProjectRegistryStatusV1,
}

/// What a full application-state reset cleared: the exact global config keys
/// it deleted, and how many capability grants it revoked. The records are
/// deleted before any grant is revoked, and a failing delete fails the command
/// with every grant still in place, so a result at all means the grants went
/// with the records. `records` is the fixed manifest the confirmation dialog
/// names, not a per-key report.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetApplicationStateResultV1 {
    pub records: [&'static str; 4],
    pub capability_grants: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRecentProjectResultV1 {
    pub open: WorkspaceChangeResult,
    pub entry_id: String,
    pub registry_base: String,
    pub registry_status: RecentProjectRegistryStatusV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgetRecentProjectResultV1 {
    pub entry_id: String,
    pub registry_base: String,
    pub forgotten: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "name", content = "payload")]
pub enum DesktopEvent {
    #[serde(rename = "workspace:runtime-changed")]
    WorkspaceRuntimeChanged(u64),
    #[serde(rename = "workspace:data-changed")]
    WorkspaceDataChanged(u64),
    #[serde(rename = "update:state-changed")]
    UpdateStateChanged(Value),
    #[serde(rename = "terminal:status")]
    TerminalStatus(crate::TerminalStatusV2),
    #[serde(rename = "terminal:exit")]
    TerminalExit(crate::TerminalExitV2),
}

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
/// terminal, agent, capability, and watcher authority; native shells receive
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
    fn shutdown(&self) -> AppResult<()>;
}

#[allow(clippy::missing_errors_doc)]
pub trait DesktopWorkspaceFactory: Send + Sync {
    fn build(&self, root: &Path, generation: u64) -> AppResult<Arc<dyn DesktopWorkspace>>;
}

#[allow(clippy::missing_errors_doc)]
pub trait RecentProjectsProvider: Send + Sync {
    fn recent_projects(&self) -> AppResult<Vec<Value>>;

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

struct Confirmation {
    token: String,
    action: ConfirmationAction,
    path: PathBuf,
    generation: u64,
    resource_revision: u64,
    resources: ActiveResourceSummary,
    expires_at: Instant,
    _expiry_cancellation: Sender<()>,
    _admission: DesktopAdmissionFence,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ConfirmationAction {
    Open,
    Close,
}

struct RuntimeState {
    status: WorkspaceStatus,
    generation: u64,
    workspace: Option<Arc<dyn DesktopWorkspace>>,
    error: String,
    confirmation: Option<Confirmation>,
    shutting_down: bool,
    shutdown_retry: bool,
    authority_changing: bool,
    active_calls: usize,
    completed_initialization: Option<CompletedInitializationReplay>,
}

struct CompletedInitializationReplay {
    request: InitializeProjectRequestV1,
    result: InitializeProjectResultV1,
}

struct ResourceAdmissionState {
    fences: usize,
    pending: usize,
    revision: u64,
}

struct ResourceAdmissionGate {
    state: Mutex<ResourceAdmissionState>,
}

pub(super) struct ResourceAdmissionLease(Arc<ResourceAdmissionGate>);

impl Drop for ResourceAdmissionLease {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.pending = state.pending.saturating_sub(1);
        state.revision = state.revision.saturating_add(1);
    }
}

struct WorkspaceCallState {
    closing: bool,
    active: usize,
}

struct WorkspaceCallGate {
    state: Mutex<WorkspaceCallState>,
    idle: Condvar,
}

pub(super) struct WorkspaceCallLease(Arc<WorkspaceCallGate>);

impl Drop for WorkspaceCallLease {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.active = state.active.saturating_sub(1);
        if state.active == 0 {
            self.0.idle.notify_all();
        }
    }
}

struct ResourceAdmissionFence(Arc<ResourceAdmissionGate>);

impl Drop for ResourceAdmissionFence {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.fences = state.fences.saturating_sub(1);
    }
}

/// Opaque coordinator-owned fence that blocks both terminal and `AgentRun`
/// admission for the lifetime of a workspace confirmation.
pub struct DesktopAdmissionFence {
    _resource: Option<ResourceAdmissionFence>,
    _agent: Option<crate::AgentAdmissionFence>,
}

impl DesktopAdmissionFence {
    const fn empty() -> Self {
        Self {
            _resource: None,
            _agent: None,
        }
    }
}

/// Lease for a native action that must finish before desktop shutdown.
pub struct DesktopNativeActionLease {
    runtime: Arc<DesktopRuntime>,
}

impl Drop for DesktopNativeActionLease {
    fn drop(&mut self) {
        let mut state = lock(&self.runtime.state);
        state.active_calls = state.active_calls.saturating_sub(1);
        self.runtime.calls_changed.notify_all();
    }
}

struct WorkspaceWatcher {
    cancellations: Vec<Sender<()>>,
    handles: Vec<JoinHandle<()>>,
}

impl WorkspaceWatcher {
    fn stop(self) {
        for cancel in self.cancellations {
            let _ = cancel.send(());
        }
        for handle in self.handles {
            let _ = handle.join();
        }
    }
}

pub struct DesktopRuntime {
    version: String,
    factory: Arc<dyn DesktopWorkspaceFactory>,
    event_sink: Option<Arc<dyn DesktopEventSink>>,
    transition: Mutex<()>,
    recent_mutation: Mutex<()>,
    state: Mutex<RuntimeState>,
    calls_changed: Condvar,
    watcher: Mutex<Option<WorkspaceWatcher>>,
    terminal_windows: Mutex<TerminalWindows>,
    recent_projects: Arc<dyn RecentProjectsProvider>,
    initialization: Arc<dyn DesktopInitializationService>,
    update_service: Arc<dyn crate::DesktopUpdateService>,
    confirmation_ttl: Duration,
}

impl DesktopRuntime {
    #[must_use]
    pub fn new(config: DesktopRuntimeConfig) -> Arc<Self> {
        let initial = config.initial_workspace.clone();
        let (status, generation) = if config.initial_workspace.is_some() {
            (WorkspaceStatus::Open, 1)
        } else {
            (WorkspaceStatus::Welcome, 0)
        };
        let runtime = Arc::new(Self {
            version: config.version,
            factory: config.factory,
            event_sink: config.event_sink,
            transition: Mutex::new(()),
            recent_mutation: Mutex::new(()),
            state: Mutex::new(RuntimeState {
                status,
                generation,
                workspace: config.initial_workspace,
                error: String::new(),
                confirmation: None,
                shutting_down: false,
                shutdown_retry: false,
                authority_changing: false,
                active_calls: 0,
                completed_initialization: None,
            }),
            calls_changed: Condvar::new(),
            watcher: Mutex::new(None),
            terminal_windows: Mutex::new(TerminalWindows::default()),
            recent_projects: config.recent_projects,
            initialization: config.initialization,
            update_service: config.update_service,
            confirmation_ttl: if config.confirmation_ttl.is_zero() {
                DEFAULT_CONFIRMATION_TTL
            } else {
                config.confirmation_ttl
            },
        });
        let _ = runtime.update_service.start();
        if let Some(workspace) = initial {
            let project = workspace.project();
            runtime.start_workspace_watcher(1, PathBuf::from(project.db_path), workspace);
        }
        runtime
    }

    /// Dispatches one size-bounded allowlisted desktop request.
    ///
    /// # Errors
    /// Returns validation, lifecycle, or command-specific errors.
    #[allow(clippy::needless_pass_by_value)]
    pub fn invoke(self: &Arc<Self>, request: DesktopCommandRequest) -> AppResult<Value> {
        validate_request(&request)?;
        if let Some(result) = self.application_state(&request.method, &request.arguments) {
            return result;
        }
        match request.method.as_str() {
            "GetWorkspaceState" => value(self.workspace_state()),
            "GetInitializationStatusV1" => self.initialization_status(&request.arguments),
            "GetPendingInitializationV1" => self.pending_initialization(&request.arguments),
            "InitializeProjectV1" => {
                let request: InitializeProjectRequestV1 = typed_arg(&request.arguments, 0)?;
                value(self.initialize_project(request)?)
            }
            "PreviewProjectGuideV1" => {
                let _lease = self.begin_native_action()?;
                let request: ProjectGuidePreviewRequestV1 = typed_arg(&request.arguments, 0)?;
                value(self.initialization.preview_guide(&request)?)
            }
            "OpenProject" => {
                let _lease = self.begin_native_action()?;
                let root = path_arg(&request.arguments, 0)?;
                let token = string_arg(&request.arguments, 1)?;
                value(self.open_project(&root, token)?)
            }
            "CloseProject" => {
                let _lease = self.begin_native_action()?;
                let token = string_arg(&request.arguments, 0)?;
                value(self.close_project(token)?)
            }
            "CancelWorkspaceChange" => {
                let _lease = self.begin_native_action()?;
                self.cancel_workspace_change(string_arg(&request.arguments, 0)?)?;
                Ok(Value::Null)
            }
            "OpenHelpDestination" => {
                let _lease = self.begin_native_action()?;
                let destination = string_arg(&request.arguments, 0)?;
                Ok(Value::String(help_destination(destination)?.to_owned()))
            }
            "PickProjectDirectory" => Err(unavailable("directory picker")),
            "GetUpdateState" => {
                let _lease = self.begin_native_action()?;
                value(self.update_service.state())
            }
            "CancelUpdateOperation" => {
                let _lease = self.begin_native_action()?;
                value(self.update_service.cancel_operation())
            }
            "SetAutomaticUpdateChecks" => {
                let _lease = self.begin_native_action()?;
                value(
                    self.update_service
                        .set_automatic_checks(bool_arg(&request.arguments, 0)?)
                        .map_err(AppError::Message)?,
                )
            }
            "CheckForUpdates" => {
                let _lease = self.begin_native_action()?;
                value(
                    self.update_service
                        .check_for_updates()
                        .map_err(AppError::Message)?,
                )
            }
            "DownloadUpdate" => {
                let _lease = self.begin_native_action()?;
                value(
                    self.update_service
                        .download_update(string_arg(&request.arguments, 0)?)
                        .map_err(AppError::Message)?,
                )
            }
            "ApplyUpdate" => {
                let _lease = self.begin_native_action()?;
                value(
                    self.update_service
                        .apply_update(string_arg(&request.arguments, 0)?)
                        .map_err(AppError::Message)?,
                )
            }
            "InstallShellCommand" => {
                let _lease = self.begin_native_action()?;
                value(crate::install_shell_command().message)
            }
            "GetRecentProjects" => self.get_recent_projects(),
            "GetRecentProjectsV1" => self.get_recent_projects_v1(&request.arguments),
            "ResolveRecentProjectV1" => self.resolve_recent_project_v1(&request.arguments),
            "OpenRecentProjectV1" => self.open_recent_project_v1(&request.arguments),
            "ForgetRecentProjectV1" => self.forget_recent_project_v1(&request.arguments),
            "ValidateProjectTargetV1" => {
                let _lease = self.begin_native_action()?;
                value(
                    self.initialization
                        .validate_target(&path_arg(&request.arguments, 0)?)?,
                )
            }
            method => self.with_workspace(method, &request.arguments),
        }
    }

    /// Application-scoped state, answered above any project workspace so it
    /// stays reachable on Welcome. `None` leaves the method to the workspace
    /// dispatch.
    fn application_state(
        self: &Arc<Self>,
        method: &str,
        arguments: &[Value],
    ) -> Option<AppResult<Value>> {
        Some(match method {
            "GetPreferences" => self.get_preferences(arguments),
            "SetPreferences" => self.set_preferences(arguments),
            "ResetPreferences" => self.reset_preferences(arguments),
            "GetDiagnosticsReport" => self.get_diagnostics_report(arguments),
            "GetLayoutState" => self.get_layout_state(arguments),
            "SetLayoutState" => self.set_layout_state(arguments),
            "ResetWindowLayout" => self.reset_window_layout(arguments),
            "ResetApplicationState" => self.reset_application_state(arguments),
            "OpenTerminalWindow" => self.open_terminal_window_command(arguments),
            "GetTerminalWindowTab" => self.terminal_window_tab_command(arguments),
            "SetTerminalWindowTab" => self.set_terminal_window_tab_command(arguments),
            _ => return None,
        })
    }

    fn open_terminal_window_command(&self, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("OpenTerminalWindow", arguments, 2)?;
        let tab = tab_args("OpenTerminalWindow", arguments, 0)?;
        let label = self.open_terminal_window(tab)?;
        Ok(json!({ "label": label }))
    }

    fn terminal_window_tab_command(&self, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("GetTerminalWindowTab", arguments, 1)?;
        Ok(match self.terminal_window_tab(string_arg(arguments, 0)?) {
            Some(tab) => json!({ "sessions": tab.sessions, "shape": tab.shape }),
            None => json!({ "sessions": null, "shape": null }),
        })
    }

    fn set_terminal_window_tab_command(&self, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("SetTerminalWindowTab", arguments, 3)?;
        let label = string_arg(arguments, 0)?.to_owned();
        let tab = tab_args("SetTerminalWindowTab", arguments, 1)?;
        self.set_terminal_window_tab(&label, tab)?;
        Ok(json!({}))
    }

    /// The generation a terminal-window assignment is fenced by: the open
    /// workspace's generation, and nothing at all while no project is open.
    fn terminal_window_fence(&self) -> Option<u64> {
        let state = lock(&self.state);
        (state.status == WorkspaceStatus::Open).then_some(state.generation)
    }

    /// Records one window assignment and returns its minted label. The shell
    /// builds the window from that label and calls `close_terminal_window` if
    /// the build fails, so a failed pop-out never leaves a session unowned.
    ///
    /// # Errors
    /// Returns an error with no project open, without at least one session,
    /// when any session is already shown by a window, or at the window limit.
    pub fn open_terminal_window(&self, tab: TerminalWindowTab) -> AppResult<String> {
        let fence = self.terminal_window_fence();
        lock(&self.terminal_windows).open(fence, tab)
    }

    /// The tab one terminal window owns, or `None` for an unknown label.
    #[must_use]
    pub fn terminal_window_tab(&self, label: &str) -> Option<TerminalWindowTab> {
        lock(&self.terminal_windows).tab(label).cloned()
    }

    /// Replaces one window's tab after a split changed inside it.
    ///
    /// # Errors
    /// Returns an error for an unknown label, without at least one session, or
    /// when any session belongs to a different window.
    pub fn set_terminal_window_tab(&self, label: &str, tab: TerminalWindowTab) -> AppResult<()> {
        lock(&self.terminal_windows).set_tab(label, tab)
    }

    /// Clears one assignment and reports the tab it freed, once: the shell
    /// pops a tab back in exactly when this answers `Some`, so a second call
    /// for the same window must free nothing.
    pub fn close_terminal_window(&self, label: &str) -> Option<TerminalWindowTab> {
        lock(&self.terminal_windows).close(label)
    }

    /// Labels whose workspace is gone — a switched or closed project — so the
    /// shell can close their windows. Empty while the workspace is unchanged.
    pub fn expire_terminal_windows(&self) -> Vec<String> {
        let fence = self.terminal_window_fence();
        lock(&self.terminal_windows).expire(fence)
    }

    /// Clears every assignment and reports the labels, for app shutdown.
    pub fn drain_terminal_windows(&self) -> Vec<String> {
        lock(&self.terminal_windows).drain()
    }

    fn get_preferences(self: &Arc<Self>, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("GetPreferences", arguments, 0)?;
        let _lease = self.begin_native_action()?;
        value(preferences(&self.global_store()?))
    }

    fn set_preferences(self: &Arc<Self>, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("SetPreferences", arguments, 1)?;
        let _lease = self.begin_native_action()?;
        value(apply_preferences(
            &self.global_store()?,
            &arguments[0],
            self.open_project_root().as_deref(),
        )?)
    }

    fn reset_preferences(self: &Arc<Self>, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("ResetPreferences", arguments, 0)?;
        let _lease = self.begin_native_action()?;
        value(reset_preferences(&self.global_store()?)?)
    }

    fn get_layout_state(self: &Arc<Self>, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("GetLayoutState", arguments, 0)?;
        let _lease = self.begin_native_action()?;
        value(layout_state(&self.global_store()?))
    }

    fn set_layout_state(self: &Arc<Self>, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("SetLayoutState", arguments, 1)?;
        let _lease = self.begin_native_action()?;
        value(set_layout_state(&self.global_store()?, &arguments[0])?)
    }

    fn reset_window_layout(self: &Arc<Self>, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("ResetWindowLayout", arguments, 0)?;
        let _lease = self.begin_native_action()?;
        value(reset_window_layout(&self.global_store()?)?)
    }

    /// Clears every app-scoped record and revokes every capability grant, and
    /// reports what went so the confirmation dialog can be honest. Grants live
    /// in the project, so revoking them writes to the open project's store;
    /// plans, tasks, notes, the recents registry, and capability definitions
    /// are untouched. The store is opened and the records are deleted first: a
    /// store that cannot be opened must not cost the user their grants for
    /// nothing.
    fn reset_application_state(self: &Arc<Self>, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("ResetApplicationState", arguments, 0)?;
        let _lease = self.begin_native_action()?;
        let records = reset_application_records(&self.global_store()?)?;
        value(ResetApplicationStateResultV1 {
            records,
            capability_grants: self.revoke_capability_grants(),
        })
    }

    /// Disables every enabled capability in the open workspace, which revokes
    /// the grant without deleting the operator's capability definition. There
    /// is nothing to revoke while no workspace is open.
    fn revoke_capability_grants(self: &Arc<Self>) -> usize {
        let generation = self.workspace_state().generation;
        let Ok(listed) = self.with_workspace("GetCapabilitiesV2", &[json!(generation)]) else {
            return 0;
        };
        let granted: Vec<u64> = listed["capabilities"]
            .as_array()
            .map_or_else(Vec::new, |rows| {
                rows.iter()
                    .filter(|row| row["state"] == "enabled")
                    .filter_map(|row| row["capability"]["id"].as_u64())
                    .collect()
            });
        granted
            .into_iter()
            .filter(|id| {
                self.with_workspace("DisableCapabilityV2", &[json!(generation), json!(id)])
                    .is_ok()
            })
            .count()
    }

    fn get_diagnostics_report(self: &Arc<Self>, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("GetDiagnosticsReport", arguments, 0)?;
        let _lease = self.begin_native_action()?;
        value(self.diagnostics_report()?)
    }

    /// Opens the global store for project-independent application state. The
    /// home is the same fixed platform home the host resolved at startup.
    fn global_store(&self) -> AppResult<GlobalStore> {
        let home = crate::resolve_global_home()?;
        let runtime = ActiveRuntime::load(&home, &self.version)?.ok_or_else(|| {
            AppError::Message("p-track runtime is not initialized (run 'ptrack init')".to_owned())
        })?;
        let bindings = runtime.global_bindings(runtime.global_home())?;
        Ok(GlobalStore::open_existing(
            &bindings.global_database,
            &bindings.global_binding,
        )?)
    }

    fn diagnostics_report(&self) -> AppResult<DiagnosticsReportV1> {
        let home = crate::resolve_global_home()?;
        let state = self.workspace_state();
        Ok(crate::diagnostics_report::report(
            &home,
            &self.version,
            state.project.as_ref(),
            self.capability_counts(state.generation),
        ))
    }

    /// Counts capability grants through the open workspace. Absent while no
    /// project workspace can answer for them.
    fn capability_counts(&self, generation: u64) -> Option<CapabilityCountsV1> {
        let capabilities = self
            .with_workspace("GetCapabilitiesV2", &[json!(generation)])
            .ok()?;
        let rows = capabilities.get("capabilities")?.as_array()?;
        Some(CapabilityCountsV1 {
            granted: rows.iter().filter(|row| row["state"] == "enabled").count(),
            total: rows.len(),
        })
    }

    fn get_recent_projects(self: &Arc<Self>) -> AppResult<Value> {
        let _lease = self.begin_native_action()?;
        value(self.recent_projects.recent_projects()?)
    }

    fn get_recent_projects_v1(self: &Arc<Self>, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("GetRecentProjectsV1", arguments, 0)?;
        let _lease = self.begin_native_action()?;
        value(self.recent_projects.recent_projects_v1()?)
    }

    fn resolve_recent_project_v1(self: &Arc<Self>, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("ResolveRecentProjectV1", arguments, 3)?;
        let _lease = self.begin_native_action()?;
        value(self.recent_projects.resolve_recent_project(
            recent_identifier_arg(arguments, 0)?,
            recent_identifier_arg(arguments, 1)?,
            &recent_path_arg(arguments, 2)?,
        )?)
    }

    fn forget_recent_project_v1(self: &Arc<Self>, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("ForgetRecentProjectV1", arguments, 2)?;
        let _lease = self.begin_native_action()?;
        let _mutation = lock(&self.recent_mutation);
        value(self.recent_projects.forget_recent_project(
            recent_identifier_arg(arguments, 0)?,
            recent_identifier_arg(arguments, 1)?,
        )?)
    }

    fn open_recent_project_v1(self: &Arc<Self>, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("OpenRecentProjectV1", arguments, 5)?;
        let _lease = self.begin_native_action()?;
        let _mutation = lock(&self.recent_mutation);
        let workspace_token = recent_optional_token_arg(arguments, 4)?;
        let authorization = (|| {
            self.recent_projects.authorize_recent_project_open(
                recent_identifier_arg(arguments, 0)?,
                recent_identifier_arg(arguments, 1)?,
                &recent_path_arg(arguments, 2)?,
                recent_optional_token_arg(arguments, 3)?,
            )
        })();
        let authorization = match authorization {
            Ok(authorization) => authorization,
            Err(error) => {
                if !workspace_token.is_empty() {
                    self.cancel_workspace_change_if_exact(workspace_token);
                }
                return Err(error);
            }
        };
        let mut open = if authorization.already_completed
            && let Some(completed) = self.completed_recent_open(&authorization)
        {
            completed
        } else {
            match self.open_project(Path::new(&authorization.canonical_root), workspace_token) {
                Ok(open) => open,
                Err(error) => {
                    if !workspace_token.is_empty() {
                        self.cancel_workspace_change_if_exact(workspace_token);
                    }
                    return Err(sanitize_recent_open_error(error));
                }
            }
        };
        let commit = if open.requires_confirmation {
            RecentProjectRegistryCommitV1 {
                base: authorization.base.clone(),
                status: RecentProjectRegistryStatusV1::Unchanged,
            }
        } else {
            self.recent_projects
                .finish_recent_project_open(&authorization)
                .unwrap_or_else(|_| {
                    if open.warning.is_empty() {
                        "recent-project registry update is incomplete"
                            .clone_into(&mut open.warning);
                    }
                    RecentProjectRegistryCommitV1 {
                        base: authorization.base.clone(),
                        status: RecentProjectRegistryStatusV1::Stale,
                    }
                })
        };
        value(OpenRecentProjectResultV1 {
            open,
            entry_id: authorization.entry_id,
            registry_base: commit.base,
            registry_status: commit.status,
        })
    }

    fn completed_recent_open(
        &self,
        authorization: &RecentProjectOpenAuthorizationV1,
    ) -> Option<WorkspaceChangeResult> {
        let state = self.workspace_state();
        if state.status != WorkspaceStatus::Open
            || state.project.as_ref().map(|project| project.root.as_str())
                != Some(authorization.canonical_root.as_str())
        {
            return None;
        }
        Some(WorkspaceChangeResult {
            state,
            requires_confirmation: false,
            confirmation_token: String::new(),
            active_resources: ActiveResourceSummary::default(),
            warning: String::new(),
        })
    }

    #[must_use]
    pub fn workspace_state(&self) -> WorkspaceState {
        let state = lock(&self.state);
        state_view(&state, &self.version)
    }

    /// Fences new calls, drains active leases, and tears down the workspace.
    ///
    /// # Errors
    /// Returns a bounded drain timeout or workspace teardown error.
    pub fn begin_shutdown(&self) -> AppResult<()> {
        {
            let mut state = lock(&self.state);
            if state.authority_changing {
                return Err(AppError::Message(
                    "runtime authority is changing".to_owned(),
                ));
            }
            if state.shutting_down && state.workspace.is_none() {
                return Ok(());
            }
            if state.shutting_down && !state.shutdown_retry {
                return Err(shutting_down());
            }
            state.shutting_down = true;
            state.shutdown_retry = false;
            state.confirmation = None;
        }
        if let Err(error) = self.update_service.shutdown() {
            lock(&self.state).shutting_down = false;
            return Err(AppError::Message(error));
        }
        {
            let mut state = lock(&self.state);
            let deadline = Instant::now() + RUNTIME_CALL_TIMEOUT;
            while state.active_calls != 0 {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    state.shutting_down = false;
                    return Err(AppError::Message(
                        "runtime calls did not finish before close".to_owned(),
                    ));
                }
                let (next, result) = self
                    .calls_changed
                    .wait_timeout(state, remaining)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state = next;
                if result.timed_out() && state.active_calls != 0 {
                    state.shutting_down = false;
                    return Err(AppError::Message(
                        "runtime calls did not finish before close".to_owned(),
                    ));
                }
            }
        }
        let _transition = lock(&self.transition);
        let workspace = lock(&self.state).workspace.clone();
        self.stop_workspace_watcher();
        if let Some(workspace) = workspace
            && let Err(error) = workspace.shutdown()
        {
            lock(&self.state).shutdown_retry = true;
            return Err(error);
        }
        let mut state = lock(&self.state);
        state.workspace = None;
        state.status = WorkspaceStatus::Closed;
        Ok(())
    }

    /// Acquires one shutdown-fenced lease for a native menu, dialog, browser,
    /// or clipboard action.
    ///
    /// # Errors
    /// Returns the exact shutdown fence error once close has begun.
    pub fn begin_native_action(self: &Arc<Self>) -> AppResult<DesktopNativeActionLease> {
        let mut state = lock(&self.state);
        if state.shutting_down {
            return Err(shutting_down());
        }
        if state.authority_changing {
            return Err(AppError::Message(
                "runtime authority is changing".to_owned(),
            ));
        }
        state.active_calls = state.active_calls.saturating_add(1);
        drop(state);
        Ok(DesktopNativeActionLease {
            runtime: Arc::clone(self),
        })
    }

    fn with_workspace(&self, method: &str, arguments: &[Value]) -> AppResult<Value> {
        let workspace = {
            let mut state = lock(&self.state);
            if state.shutting_down {
                return Err(shutting_down());
            }
            if state.authority_changing {
                return Err(AppError::Message(
                    "runtime authority is changing".to_owned(),
                ));
            }
            if state.status != WorkspaceStatus::Open {
                return Err(AppError::Message("no project workspace is open".to_owned()));
            }
            let workspace = state
                .workspace
                .clone()
                .ok_or_else(|| AppError::Message("no project workspace is open".to_owned()))?;
            state.active_calls = state.active_calls.saturating_add(1);
            workspace
        };
        let _lease = DesktopCallLease { runtime: self };
        workspace.invoke(method, arguments)
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
                    state.authority_changing = false;
                    state.status = WorkspaceStatus::Error;
                    "runtime calls did not finish before initialization"
                        .clone_into(&mut state.error);
                    return Err(AppError::Message(state.error.clone()));
                }
                let (next, result) = self
                    .calls_changed
                    .wait_timeout(state, remaining)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state = next;
                if result.timed_out() && state.active_calls != 0 {
                    state.authority_changing = false;
                    state.status = WorkspaceStatus::Error;
                    "runtime calls did not finish before initialization"
                        .clone_into(&mut state.error);
                    return Err(AppError::Message(state.error.clone()));
                }
            }
        }
        let _transition = lock(&self.transition);

        let initialized = (|| -> AppResult<InitializeProjectResultV1> {
            let status = self.initialization.initialize(&request)?;
            if status.outcome == InitializationOutcomeV1::RecoveryRequired {
                let mut state = lock(&self.state);
                state.status = WorkspaceStatus::Error;
                state.error.clone_from(&status.error_kind);
                return Ok(InitializeProjectResultV1 {
                    initialization: status,
                    state: state_view(&state, &self.version),
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
            self.emit(DesktopEvent::WorkspaceDataChanged(next_generation));
            Ok(result)
        })();

        let mut state = lock(&self.state);
        state.authority_changing = false;
        if let Err(error) = &initialized
            && state.workspace.is_none()
        {
            state.status = WorkspaceStatus::Error;
            state.error = error.to_string();
        }
        drop(state);
        self.calls_changed.notify_all();
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
        Ok(Some(result))
    }

    fn pending_initialization(self: &Arc<Self>, arguments: &[Value]) -> AppResult<Value> {
        require_argument_count("GetPendingInitializationV1", arguments, 0)?;
        let _lease = self.begin_native_action()?;
        let _transition = lock(&self.transition);
        let pending = self.initialization.pending()?;
        if !pending.pending
            && let Some(status) = self.initialization.completed_initialization()?
        {
            self.bind_completed_initialization_locked(&status)?;
        }
        value(pending)
    }

    fn initialization_status(self: &Arc<Self>, arguments: &[Value]) -> AppResult<Value> {
        let operation_id = string_arg(arguments, 0)?;
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

    fn open_project(
        self: &Arc<Self>,
        root: &Path,
        token: &str,
    ) -> AppResult<WorkspaceChangeResult> {
        let _transition = lock(&self.transition);
        self.require_not_shutting_down()?;
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
    fn open_project_root(&self) -> Option<String> {
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
            state_view(&state, &self.version)
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
        if valid {
            Ok(())
        } else {
            Err(AppError::Message(
                "invalid or expired workspace confirmation".to_owned(),
            ))
        }
    }

    fn cancel_workspace_change_if_exact(&self, token: &str) {
        let mut state = lock(&self.state);
        if state
            .confirmation
            .as_ref()
            .is_some_and(|confirmation| confirmation.token == token)
        {
            state.confirmation = None;
        }
    }

    fn emit(&self, event: DesktopEvent) {
        if let Some(sink) = &self.event_sink {
            sink.emit(event);
        }
    }

    fn require_not_shutting_down(&self) -> AppResult<()> {
        if lock(&self.state).shutting_down {
            Err(shutting_down())
        } else {
            Ok(())
        }
    }

    fn start_workspace_watcher(
        &self,
        generation: u64,
        database: PathBuf,
        workspace: Arc<dyn DesktopWorkspace>,
    ) {
        self.stop_workspace_watcher();
        let Some(sink) = self.event_sink.clone() else {
            return;
        };
        let file_sink = sink.clone();
        let (file_cancel, file_cancellation) = channel();
        let file_handle = thread::Builder::new()
            .name("ptrack-workspace-watch".to_owned())
            .spawn(move || {
                watch_workspace_data(
                    &file_cancellation,
                    &database,
                    WORKSPACE_WATCH_INTERVAL,
                    WORKSPACE_WATCH_DEBOUNCE,
                    || file_sink.emit(DesktopEvent::WorkspaceDataChanged(generation)),
                );
            });
        let (runtime_cancel, runtime_cancellation) = channel();
        let runtime_handle = thread::Builder::new()
            .name("ptrack-runtime-watch".to_owned())
            .spawn(move || {
                while let Err(RecvTimeoutError::Timeout) =
                    runtime_cancellation.recv_timeout(Duration::from_millis(100))
                {
                    if workspace.drain_runtime_invalidations().unwrap_or(false) {
                        sink.emit(DesktopEvent::WorkspaceRuntimeChanged(generation));
                    }
                }
            });
        if let (Ok(file_handle), Ok(runtime_handle)) = (file_handle, runtime_handle) {
            *lock(&self.watcher) = Some(WorkspaceWatcher {
                cancellations: vec![file_cancel, runtime_cancel],
                handles: vec![file_handle, runtime_handle],
            });
        }
    }

    fn stop_workspace_watcher(&self) {
        let watcher = lock(&self.watcher).take();
        if let Some(watcher) = watcher {
            watcher.stop();
        }
    }
}

impl Drop for DesktopRuntime {
    fn drop(&mut self) {
        if let Some(watcher) = self
            .watcher
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            watcher.stop();
        }
        let workspace = self
            .state
            .get_mut()
            .ok()
            .and_then(|state| state.workspace.take());
        if let Some(workspace) = workspace {
            let _ = workspace.shutdown();
        }
    }
}

struct DesktopCallLease<'a> {
    runtime: &'a DesktopRuntime,
}

impl Drop for DesktopCallLease<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.runtime.state);
        state.active_calls = state.active_calls.saturating_sub(1);
        self.runtime.calls_changed.notify_all();
    }
}

/// Composite authority needed by desktop linked-launch and exact-resource
/// coordination. Presentation callers never receive this object.
pub trait DesktopAgentRuntime:
    AgentRuntimeService + LaunchedEventAuthority + LinkedAgentRuntimeHooks + Send + Sync
{
}

impl<T> DesktopAgentRuntime for T where
    T: AgentRuntimeService + LaunchedEventAuthority + LinkedAgentRuntimeHooks + Send + Sync
{
}

struct ProjectLaunchContextStore<'a> {
    root: &'a Path,
    snapshot: &'a ProjectSnapshot,
}

impl LaunchContextStore for ProjectLaunchContextStore<'_> {
    fn project_root(&self) -> Result<PathBuf, String> {
        Ok(self.root.to_path_buf())
    }

    fn meta(&self) -> Result<Meta, String> {
        Ok(self.snapshot.meta.clone())
    }

    fn plan(&self, id: u64) -> Result<Option<Plan>, String> {
        Ok(self.snapshot.plan(id).cloned())
    }

    fn task(&self, id: u64) -> Result<Option<Task>, String> {
        Ok(self.snapshot.task(id).cloned())
    }

    fn recent_notes(&self, limit: usize) -> Result<BoundedItems<Note>, String> {
        let total = self.snapshot.notes.len();
        Ok(BoundedItems {
            items: self
                .snapshot
                .notes
                .iter()
                .rev()
                .take(limit)
                .cloned()
                .collect(),
            more: total.saturating_sub(limit),
        })
    }

    fn open_issues(&self, limit: usize) -> Result<ScanBoundedItems<Issue>, String> {
        let mut issues = self
            .snapshot
            .issues
            .iter()
            .filter(|issue| issue.status == IssueStatus::Open)
            .cloned()
            .collect::<Vec<_>>();
        let truncated = issues.len() > limit;
        issues.truncate(limit);
        Ok(ScanBoundedItems {
            items: issues,
            truncated,
        })
    }

    fn recent_commits(&self, limit: usize) -> Result<BoundedItems<Commit>, String> {
        let total = self.snapshot.commits.len();
        Ok(BoundedItems {
            items: self
                .snapshot
                .commits
                .iter()
                .rev()
                .take(limit)
                .cloned()
                .collect(),
            more: total.saturating_sub(limit),
        })
    }
}

impl AssociationCatalog for ProjectLaunchContextStore<'_> {
    fn validate_plan(&self, plan_id: u64) -> Result<(), String> {
        self.snapshot
            .plan(plan_id)
            .map(|_| ())
            .ok_or_else(|| "project plan is unavailable".to_owned())
    }

    fn task_plan(&self, task_id: u64) -> Result<u64, String> {
        self.snapshot
            .task(task_id)
            .map(|task| task.plan_id)
            .ok_or_else(|| "project task is unavailable".to_owned())
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TaskResource {
    kind: &'static str,
    id: String,
    revision: u64,
    state: String,
    process_state: String,
    lease_state: String,
    lifecycle_revision: u64,
}

#[derive(Clone)]
struct TaskChallenge {
    generation: u64,
    task_id: u64,
    plan_id: u64,
    from_status: TaskStatus,
    to_status: TaskStatus,
    task_updated_at: Timestamp,
    terminal_revision: u64,
    agent_revision: u64,
    admission_revision: u64,
    resources: Vec<TaskResource>,
    active_terminals: usize,
    active_agents: usize,
    issued_at: Instant,
    expires_at: Instant,
}

/// Explicitly bound production workspace used by the desktop coordinator.
/// Construction takes attested bindings and never performs project discovery.
pub struct BoundDesktopWorkspace {
    generation: u64,
    initial_plan: u64,
    bindings: WorkspaceBindings,
    endpoint: ProjectEndpoint,
    application: Mutex<Box<dyn ApplicationPort + Send>>,
    terminal: Option<Arc<TerminalRuntime>>,
    agent: Option<Arc<dyn DesktopAgentRuntime>>,
    broker: Option<Arc<Broker>>,
    resource_transition: Mutex<()>,
    resource_admission: Arc<ResourceAdmissionGate>,
    workspace_calls: Arc<WorkspaceCallGate>,
    task_challenges: Mutex<BTreeMap<String, TaskChallenge>>,
}

impl BoundDesktopWorkspace {
    /// Constructs an explicitly bound desktop workspace.
    ///
    /// # Panics
    /// Panics when the attested bindings omit their project endpoint.
    #[must_use]
    pub fn new(
        generation: u64,
        initial_plan: u64,
        bindings: WorkspaceBindings,
        application: Box<dyn ApplicationPort + Send>,
        terminal: Option<Arc<TerminalRuntime>>,
        agent: Option<Arc<dyn DesktopAgentRuntime>>,
        broker: Option<Arc<Broker>>,
    ) -> Self {
        let endpoint = bindings
            .project
            .clone()
            .expect("bound desktop workspace requires a project endpoint");
        Self {
            generation,
            initial_plan,
            bindings,
            endpoint,
            application: Mutex::new(application),
            terminal,
            agent,
            broker,
            resource_transition: Mutex::new(()),
            resource_admission: Arc::new(ResourceAdmissionGate {
                state: Mutex::new(ResourceAdmissionState {
                    fences: 0,
                    pending: 0,
                    revision: 0,
                }),
            }),
            workspace_calls: Arc::new(WorkspaceCallGate {
                state: Mutex::new(WorkspaceCallState {
                    closing: false,
                    active: 0,
                }),
                idle: Condvar::new(),
            }),
            task_challenges: Mutex::new(BTreeMap::new()),
        }
    }

    pub(super) fn begin_resource_admission(&self) -> AppResult<ResourceAdmissionLease> {
        let mut state = lock(&self.resource_admission.state);
        if state.fences != 0 {
            return Err(AppError::Message(
                "workspace resource admission is fenced".to_owned(),
            ));
        }
        state.pending = state.pending.saturating_add(1);
        state.revision = state.revision.saturating_add(1);
        drop(state);
        Ok(ResourceAdmissionLease(Arc::clone(&self.resource_admission)))
    }

    pub(super) fn begin_workspace_call(&self) -> AppResult<WorkspaceCallLease> {
        let mut state = lock(&self.workspace_calls.state);
        if state.closing {
            return Err(AppError::Message("workspace is closing".to_owned()));
        }
        state.active = state.active.saturating_add(1);
        drop(state);
        Ok(WorkspaceCallLease(Arc::clone(&self.workspace_calls)))
    }

    fn close_workspace_calls(&self) -> bool {
        let deadline = Instant::now() + WORKSPACE_OPERATION_DRAIN_TIMEOUT;
        let mut state = lock(&self.workspace_calls.state);
        state.closing = true;
        while state.active != 0 {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (next, _) = self
                .workspace_calls
                .idle
                .wait_timeout(state, deadline.saturating_duration_since(now))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
        }
        true
    }

    fn admission_revision(&self) -> u64 {
        lock(&self.resource_admission.state).revision
    }

    fn require_generation(&self, expected: u64) -> AppResult<()> {
        if expected != 0 && expected != self.generation {
            return Err(AppError::Message(format!(
                "stale workspace generation: expected {expected}, active {}",
                self.generation
            )));
        }
        Ok(())
    }

    fn require_exact_generation(&self, expected: u64) -> AppResult<()> {
        if expected == 0 || expected != self.generation {
            return Err(AppError::Message(format!(
                "stale workspace generation: expected {expected}, active {}",
                self.generation
            )));
        }
        Ok(())
    }

    const fn first_run_state(&self) -> FirstRunWorkspaceStateV1 {
        FirstRunWorkspaceStateV1 {
            status: WorkspaceStatus::Open,
            generation: self.generation,
        }
    }

    fn snapshot(&self) -> AppResult<ProjectSnapshot> {
        lock(&self.application).snapshot()
    }

    fn project_store(&self) -> AppResult<ProjectStore> {
        Ok(ProjectStore::open_existing(
            &self.endpoint.database,
            &self.endpoint.binding,
            &self.bindings.writer_version,
        )?)
    }

    #[allow(clippy::too_many_lines)] // One bounded storage capture keeps totals and rows aligned.
    fn bounded_snapshot_tracking(
        &self,
        requested_plan: u64,
        deadline: Instant,
    ) -> AppResult<SnapshotTrackingCapture> {
        let store = self.project_store()?;
        let meta = store.meta()?;
        ensure_snapshot_deadline(deadline)?;
        let plans = store.plans_bounded(SNAPSHOT_PLAN_LIMIT)?;
        let selected_plan = if requested_plan == 0 {
            if self.initial_plan == 0 {
                meta.active_plan
            } else {
                self.initial_plan
            }
        } else {
            requested_plan
        };
        let tasks = if selected_plan == 0 {
            ptrack_store::Bounded {
                items: Vec::new(),
                total: 0,
                more: 0,
            }
        } else {
            store.tasks_by_plan_bounded_until(selected_plan, SNAPSHOT_TASK_LIMIT, deadline)?
        };
        ensure_snapshot_deadline(deadline)?;
        let blockers = store.blocked_tasks_bounded_until(SNAPSHOT_BLOCKER_LIMIT, deadline)?;
        let notes = store.recent_notes_bounded(SNAPSHOT_NOTE_LIMIT)?;
        let commits = store.recent_commits_bounded(SNAPSHOT_NOTE_LIMIT)?;
        let issues = store.open_issues_bounded_until(SNAPSHOT_ISSUE_LIMIT, deadline)?;
        let task_ids = tasks
            .items
            .iter()
            .map(|task| task.id)
            .collect::<BTreeSet<_>>();
        let associations = store.task_associations_until(&task_ids, deadline)?;
        let mut plan_ids = plans
            .items
            .iter()
            .map(|plan| plan.id)
            .collect::<BTreeSet<_>>();
        if selected_plan != 0 {
            plan_ids.insert(selected_plan);
        }
        let plan_progress = store.plan_task_progress_for_until(&plan_ids, deadline)?;
        let counts = store.counts_until(deadline)?;
        ensure_snapshot_deadline(deadline)?;
        let mut snapshot_plans = plans.items.clone();
        if selected_plan != 0 && !snapshot_plans.iter().any(|plan| plan.id == selected_plan) {
            snapshot_plans.push(store.plan(selected_plan).map_err(|error| match error {
                StoreError::NotFound => {
                    AppError::Message(format!("plan #{selected_plan} not found"))
                }
                other => AppError::from(other),
            })?);
        }
        let snapshot = ProjectSnapshot::new(
            meta,
            Vec::new(),
            snapshot_plans,
            tasks.items,
            issues.items,
            notes.items,
            commits.items,
        );
        let mut board = snapshot_board_view(&snapshot, self.project().name, selected_plan)?;
        board.plans = plans
            .items
            .iter()
            .map(|plan| {
                let progress = plan_progress.get(&plan.id).copied().unwrap_or_default();
                PlanSummaryView {
                    id: plan.id,
                    title: plan.title.clone(),
                    is_active: plan.id == snapshot.meta.active_plan,
                    tasks_total: progress.total,
                    tasks_done: progress.done,
                }
            })
            .collect();
        for column in &mut board.columns {
            for task in &mut column.tasks {
                task.note_count = *associations.note_counts.get(&task.id).unwrap_or(&0);
                task.commit_count = *associations.commit_counts.get(&task.id).unwrap_or(&0);
                task.issue_count = *associations.issue_counts.get(&task.id).unwrap_or(&0);
                task.latest_note = associations
                    .latest_notes
                    .get(&task.id)
                    .cloned()
                    .unwrap_or_default();
            }
        }
        let progress = plan_progress
            .get(&selected_plan)
            .copied()
            .unwrap_or_default();
        board.stats = ProjectStatsView {
            plan_tasks: progress.total,
            plan_tasks_done: progress.done,
            tasks_open: counts.tasks_open,
            tasks_blocked: counts.tasks_blocked,
            notes: counts.notes,
            commits: counts.commits,
            open_issues: counts.issues_open,
        };
        let activity_total = notes.total.saturating_add(commits.total);
        Ok(SnapshotTrackingCapture {
            snapshot,
            board,
            blockers: blockers.items.iter().map(snapshot_blocker_card).collect(),
            bounds: SnapshotTrackingBounds {
                plans: plans.total,
                tasks: tasks.total,
                blockers: blockers.total,
                notes: notes.total,
                activity: activity_total,
                issues: issues.total,
            },
        })
    }

    fn board(&self, plan_id: u64) -> AppResult<BoardView> {
        let snapshot = self.snapshot()?;
        let mut board = board_view(
            &snapshot,
            self.project().name,
            if plan_id == 0 {
                self.initial_plan
            } else {
                plan_id
            },
        )?;
        apply_linked_runtime_to_board(&mut board, &self.runtime_projection(&snapshot)?);
        Ok(board)
    }

    fn runtime_projection(&self, snapshot: &ProjectSnapshot) -> AppResult<RuntimeProjectionView> {
        let candidates = self.agent.as_ref().map_or_else(
            || Ok(empty_agent_candidates(self.generation)),
            |agent| agent.agent_runtime_candidates(self.generation),
        )?;
        self.runtime_projection_with_agents(snapshot, candidates)
    }

    fn runtime_projection_with_agents(
        &self,
        snapshot: &ProjectSnapshot,
        candidates: ptrack_agent::AgentRuntimeCandidatesV2,
    ) -> AppResult<RuntimeProjectionView> {
        self.runtime_projection_with_agents_until(snapshot, candidates, None)
    }

    fn runtime_projection_with_agents_until(
        &self,
        _snapshot: &ProjectSnapshot,
        candidates: ptrack_agent::AgentRuntimeCandidatesV2,
        deadline: Option<Instant>,
    ) -> AppResult<RuntimeProjectionView> {
        if let Some(deadline) = deadline {
            ensure_snapshot_deadline(deadline)?;
        }
        let store = self.project_store()?;
        let (terminals, terminal_total) = self.terminal.as_ref().map_or_else(
            || Ok((Vec::new(), 0)),
            |terminal| {
                terminal
                    .runtime_session_snapshot(self.generation)
                    .and_then(|(sessions, total)| {
                        let mut rows = Vec::with_capacity(sessions.len());
                        for session in &sessions {
                            if let Some(deadline) = deadline {
                                ensure_snapshot_deadline(deadline)?;
                            }
                            rows.push(terminal_runtime_summary(&store, session));
                        }
                        Ok((rows, total))
                    })
            },
        )?;
        if let Some(deadline) = deadline {
            ensure_snapshot_deadline(deadline)?;
        }
        Ok(RuntimeProjectionView {
            sources_truncated: candidates.sources_truncated || terminal_total > terminals.len(),
            terminals,
            terminal_total,
            agent_total: candidates.bounds.total,
            agents: candidates.runs,
        })
    }

    fn task_detail(&self, snapshot: &ProjectSnapshot, task_id: u64) -> AppResult<Value> {
        let projection = self.runtime_projection(snapshot)?;
        let detail = task_linked_runtime(&projection, task_id);
        let mut intelligence = Vec::new();
        if let Some(agent) = &self.agent {
            for run in &detail.agents {
                if let Some(value) = agent_intelligence_for_task_result(
                    run,
                    task_id,
                    agent.agent_intelligence(self.generation, &run.run_id),
                )? {
                    intelligence.push(value);
                }
            }
        }
        task_detail_value(self.generation, snapshot, task_id, &detail, &intelligence)
    }

    pub(super) fn resolve_linked_launch_cwd(&self, requested: &str) -> AppResult<PathBuf> {
        let candidate = if requested.is_empty() {
            self.endpoint.root.clone()
        } else {
            let requested = Path::new(requested);
            if requested.is_absolute() {
                requested.to_path_buf()
            } else {
                self.endpoint.root.join(requested)
            }
        };
        let canonical = fs::canonicalize(candidate).map_err(|error| {
            AppError::Message(format!(
                "canonicalize linked launch working directory: {error}"
            ))
        })?;
        if canonical.starts_with(&self.endpoint.root) {
            return Ok(canonical);
        }
        let cancellation = ptrack_git::CancellationToken::new();
        ptrack_git::RepositoryService::new()
            .inspect_worktree(&cancellation, &self.endpoint.root, &canonical)
            .map(|_| canonical)
            .map_err(|_| {
                AppError::Message(
                    "linked launch working directory is outside the current project or its existing worktrees"
                        .to_owned(),
                )
            })
    }

    fn resource_revisions(&self) -> AppResult<(u64, u64)> {
        let terminal = self.terminal.as_ref().map_or(Ok(0), |terminal| {
            terminal.resource_revision(self.generation)
        })?;
        let agent = self
            .agent
            .as_ref()
            .map_or(Ok(0), |agent| agent.resource_revision(self.generation))?;
        Ok((terminal, agent))
    }

    fn with_exact_task_resources<T>(
        &self,
        snapshot: &ProjectSnapshot,
        task_id: u64,
        use_resources: impl FnOnce(Vec<TaskResource>) -> AppResult<T>,
    ) -> AppResult<T> {
        let mut use_resources = Some(use_resources);
        let mut run = |sessions: &[SessionInfo]| {
            let terminal_resources = terminal_task_resources(snapshot, task_id, sessions);
            if let Some(agent) = &self.agent {
                let mut output = None;
                let mut callback = |runs: &[Run]| {
                    let mut resources = terminal_resources.clone();
                    resources.extend(agent_task_resources(
                        snapshot,
                        &self.endpoint.root,
                        self.generation,
                        task_id,
                        runs,
                    ));
                    resources.sort();
                    output = Some(use_resources
                        .take()
                        .expect("exact task resource callback is single-use")(
                        resources
                    ));
                };
                agent.with_exact_runtime_snapshot(
                    self.generation,
                    TASK_RESOURCE_LIMIT,
                    &mut callback,
                )?;
                output.ok_or_else(|| {
                    AppError::Message("exact AgentRun resource snapshot is unavailable".to_owned())
                })?
            } else {
                use_resources
                    .take()
                    .expect("exact task resource callback is single-use")(
                    terminal_resources
                )
            }
        };
        if let Some(terminal) = &self.terminal {
            terminal.with_exact_session_snapshot(self.generation, run)
        } else {
            run(&[])
        }
    }

    fn issue_task_challenge(&self, mut challenge: TaskChallenge) -> AppResult<(String, String)> {
        let now = Instant::now();
        let mut challenges = lock(&self.task_challenges);
        challenges.retain(|_, value| now < value.expires_at);
        if challenges.len() >= TASK_CONFIRMATION_LIMIT
            && let Some(oldest) = challenges
                .iter()
                .min_by(|(left_token, left), (right_token, right)| {
                    left.issued_at
                        .cmp(&right.issued_at)
                        .then_with(|| left_token.cmp(right_token))
                })
                .map(|(token, _)| token.clone())
        {
            challenges.remove(&oldest);
        }
        challenge.issued_at = now;
        challenge.expires_at = now + TASK_CONFIRMATION_TTL;
        let expires_at = (OffsetDateTime::now_utc() + time::Duration::seconds(90))
            .format(&Rfc3339)
            .map_err(message)?;
        for _ in 0..4 {
            let token = random_token()?;
            if challenges.contains_key(&token) {
                continue;
            }
            challenges.insert(token.clone(), challenge.clone());
            return Ok((token, expires_at));
        }
        Err(AppError::Message(
            "create unique task transition confirmation".to_owned(),
        ))
    }

    fn consume_task_challenge(
        &self,
        token: &str,
        task_id: u64,
        to_status: TaskStatus,
    ) -> AppResult<TaskChallenge> {
        if token.is_empty() || token.len() > 128 {
            return Err(invalid_task_confirmation());
        }
        let now = Instant::now();
        let mut challenges = lock(&self.task_challenges);
        challenges.retain(|_, value| now < value.expires_at);
        let challenge = challenges
            .remove(token)
            .ok_or_else(invalid_task_confirmation)?;
        if now >= challenge.expires_at
            || challenge.generation != self.generation
            || challenge.task_id != task_id
            || challenge.to_status != to_status
        {
            return Err(invalid_task_confirmation());
        }
        Ok(challenge)
    }

    #[allow(clippy::too_many_lines)]
    fn move_task_v3(
        &self,
        task_id: u64,
        wanted: TaskStatus,
        confirmation_token: &str,
    ) -> AppResult<Value> {
        let _transition = lock(&self.resource_transition);
        let _admission = self.fence_resource_admission()?;
        if lock(&self.resource_admission.state).pending != 0 {
            return Err(AppError::Message(
                "task transition must retry after resource admission completes".to_owned(),
            ));
        }
        let store = self.project_store()?;
        if confirmation_token.is_empty() {
            let task = store.task(task_id).map_err(|error| match error {
                StoreError::NotFound => AppError::Message(format!("task #{task_id} not found")),
                other => AppError::from(other),
            })?;
            let base = task_transition_base(self.generation, &task, wanted);
            if task.status == wanted {
                return Ok(task_transition_applied(base));
            }
            let revisions = self.resource_revisions()?;
            let snapshot = store.snapshot()?;
            return self.with_exact_task_resources(&snapshot, task_id, |resources| {
                let active_terminals = resources
                    .iter()
                    .filter(|resource| resource.kind == "terminal")
                    .count();
                let active_agents = resources.len().saturating_sub(active_terminals);
                if resources.is_empty() {
                    store
                        .compare_and_set_task_status(
                            task.id,
                            task.plan_id,
                            task.status,
                            task.updated_at,
                            wanted,
                        )
                        .map_err(AppError::from)?;
                    return Ok(task_transition_applied(base));
                }
                let (token, expires_at) = self.issue_task_challenge(TaskChallenge {
                    generation: self.generation,
                    task_id,
                    plan_id: task.plan_id,
                    from_status: task.status,
                    to_status: wanted,
                    task_updated_at: task.updated_at,
                    terminal_revision: revisions.0,
                    agent_revision: revisions.1,
                    admission_revision: self.admission_revision(),
                    resources,
                    active_terminals,
                    active_agents,
                    issued_at: Instant::now(),
                    expires_at: Instant::now(),
                })?;
                Ok(json!({
                    "generation": self.generation,
                    "taskId": task_id,
                    "fromStatus": task.status.as_str(),
                    "toStatus": wanted.as_str(),
                    "applied": false,
                    "requiresConfirmation": true,
                    "confirmation": {
                        "token": token,
                        "expiresAt": expires_at,
                        "activeTerminals": active_terminals,
                        "activeAgents": active_agents
                    }
                }))
            });
        }

        let challenge = self.consume_task_challenge(confirmation_token, task_id, wanted)?;
        if self.resource_revisions()? != (challenge.terminal_revision, challenge.agent_revision)
            || self.admission_revision() != challenge.admission_revision
        {
            return Err(invalid_task_confirmation());
        }
        let snapshot = store.snapshot()?;
        self.with_exact_task_resources(&snapshot, task_id, |resources| {
            let active_terminals = resources
                .iter()
                .filter(|resource| resource.kind == "terminal")
                .count();
            let active_agents = resources.len().saturating_sub(active_terminals);
            if resources != challenge.resources
                || active_terminals != challenge.active_terminals
                || active_agents != challenge.active_agents
                || self.resource_revisions()?
                    != (challenge.terminal_revision, challenge.agent_revision)
                || self.admission_revision() != challenge.admission_revision
            {
                return Err(invalid_task_confirmation());
            }
            let result = store.compare_and_set_task_status(
                task_id,
                challenge.plan_id,
                challenge.from_status,
                challenge.task_updated_at,
                wanted,
            );
            match result {
                Ok(_) => Ok(task_transition_applied(json!({
                    "generation": self.generation,
                    "taskId": task_id,
                    "fromStatus": challenge.from_status.as_str(),
                    "toStatus": wanted.as_str()
                }))),
                Err(StoreError::TaskStatusChanged(_)) => Err(invalid_task_confirmation()),
                Err(error) => Err(AppError::from(error)),
            }
        })
    }

    fn start_first_task_v1(&self, task_id: u64, expected_updated_at: &str) -> AppResult<Task> {
        let expected = parse_first_run_timestamp(expected_updated_at)?;
        let store = self.project_store()?;
        let task = store.task(task_id).map_err(|error| match error {
            StoreError::NotFound => AppError::Message(format!("task #{task_id} not found")),
            other => AppError::from(other),
        })?;
        if task.status == TaskStatus::Doing {
            return store
                .start_first_task(task_id, expected)
                .map_err(AppError::from);
        }
        let _transition = lock(&self.resource_transition);
        let _admission = self.fence_resource_admission()?;
        if lock(&self.resource_admission.state).pending != 0 {
            return Err(AppError::Message(
                "first task start must retry after resource admission completes".to_owned(),
            ));
        }
        if task.status != TaskStatus::Todo || timestamp(task.updated_at) != expected_updated_at {
            return Err(AppError::Message(
                "first task changed before it could be started".to_owned(),
            ));
        }
        let snapshot = store.snapshot()?;
        self.with_exact_task_resources(&snapshot, task_id, |resources| {
            if !resources.is_empty() {
                return Err(AppError::Message(
                    "first task start requires resource confirmation".to_owned(),
                ));
            }
            store
                .start_first_task(task_id, expected)
                .map_err(AppError::from)
        })
    }

    fn workspace_snapshot(
        generation: u64,
        project: &WorkspaceProject,
        mut tracking: SnapshotTrackingCapture,
        runtime: &RuntimeProjectionView,
        agent_sections: Option<&ptrack_agent::AgentWorkspaceSnapshotV2>,
        git: &Value,
        deadline: Instant,
    ) -> AppResult<Value> {
        ensure_snapshot_deadline(deadline)?;
        let snapshot = &tracking.snapshot;
        let blockers = tracking.blockers;
        let notes = snapshot
            .notes
            .iter()
            .rev()
            .take(SNAPSHOT_NOTE_LIMIT)
            .map(note_snapshot_view)
            .collect::<Vec<_>>();
        let issues = snapshot
            .issues
            .iter()
            .rev()
            .filter(|issue| issue.status == IssueStatus::Open)
            .take(SNAPSHOT_ISSUE_LIMIT)
            .map(issue_snapshot_view)
            .collect::<Vec<_>>();
        let (agent_activity, drift) = agent_sections.map_or_else(
            || {
                Ok((
                    empty_agent_activity(),
                    json!({ "state": "ready", "findings": [], "bounds": bound(0, 0), "incomplete": false }),
                ))
            },
            |sections| {
                Ok::<_, AppError>((
                    serde_json::to_value(&sections.activity).map_err(message)?,
                    serde_json::to_value(&sections.drift).map_err(message)?,
                ))
            },
        )?;
        ensure_snapshot_deadline(deadline)?;
        let terminal_total = runtime.terminal_total;
        let terminals = json!({
            "state": "ready",
            "sessions": runtime.terminals.iter().take(SNAPSHOT_RUNTIME_LIMIT).collect::<Vec<_>>(),
            "bounds": bound(terminal_total.min(SNAPSHOT_RUNTIME_LIMIT), terminal_total)
        });
        let agent_total = runtime.agent_total;
        let agent_runs = json!({
            "state": "ready",
            "runs": runtime.agents.iter().take(SNAPSHOT_RUNTIME_LIMIT).collect::<Vec<_>>(),
            "bounds": bound(agent_total.min(SNAPSHOT_RUNTIME_LIMIT), agent_total)
        });
        tracking.board.plans.truncate(SNAPSHOT_PLAN_LIMIT);
        let mut remaining_tasks = SNAPSHOT_TASK_LIMIT;
        for column in &mut tracking.board.columns {
            column.tasks.truncate(remaining_tasks);
            remaining_tasks = remaining_tasks.saturating_sub(column.tasks.len());
        }
        tracking.board.activity.truncate(SNAPSHOT_ACTIVITY_LIMIT);
        let board_task_count = tracking
            .board
            .columns
            .iter()
            .map(|column| column.tasks.len())
            .sum::<usize>();
        let board_plan_count = tracking.board.plans.len();
        let board_activity_count = tracking.board.activity.len();
        let storage = project_storage(&project.db_path, &snapshot.meta);
        Ok(json!({
            "generation": generation,
            "capturedAt": OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default(),
            "project": {
                "name": project.name,
                "root": project.root,
                "storage": storage
            },
            "tracking": {
                "state": "ready",
                "board": tracking.board,
                "blockers": blockers,
                "notes": notes,
                "issues": issues,
                "bounds": {
                    "plans": bound(board_plan_count, tracking.bounds.plans),
                    "tasks": bound(board_task_count, tracking.bounds.tasks),
                    "blockers": bound(blockers.len(), tracking.bounds.blockers),
                    "notes": bound(notes.len(), tracking.bounds.notes),
                    "activity": bound(board_activity_count, tracking.bounds.activity),
                    "issues": bound(issues.len(), tracking.bounds.issues)
                }
            },
            "git": git,
            "agentActivity": agent_activity,
            "terminals": terminals,
            "agentRuns": agent_runs,
            "drift": drift
        }))
    }

    #[allow(clippy::too_many_lines)]
    fn invoke_agent(&self, method: &str, arguments: &[Value]) -> AppResult<Value> {
        let agent = self
            .agent
            .as_ref()
            .ok_or_else(|| unavailable("AgentRun registry"))?;
        match method {
            "AssociateAgentRunV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                let pointer = association_pointer_arg(arguments, 2)?;
                validate_association_pointer(&self.snapshot()?, pointer)?;
                let _transition = lock(&self.resource_transition);
                value(agent.associate_run(
                    self.generation,
                    string_arg(arguments, 1)?,
                    AgentAssociationPointer {
                        version: pointer.version,
                        plan_id: pointer.plan_id,
                        task_id: pointer.task_id,
                    },
                )?)
            }
            "GetAgentRunsV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                value(agent.agent_runs(self.generation)?)
            }
            "GetAgentIntelligenceV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                value(agent.agent_intelligence(self.generation, string_arg(arguments, 1)?)?)
            }
            "PreviewAgentHandoffV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                value(agent.preview_handoff(self.generation, string_arg(arguments, 1)?)?)
            }
            "SendAgentHandoffV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                value(agent.send_handoff(
                    self.generation,
                    string_arg(arguments, 1)?,
                    string_arg(arguments, 2)?,
                    u64_arg(arguments, 3)?,
                    u64_arg(arguments, 4)?,
                )?)
            }
            "AcknowledgeAgentHandoffV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                value(agent.acknowledge_handoff(
                    self.generation,
                    string_arg(arguments, 1)?,
                    string_arg(arguments, 2)?,
                )?)
            }
            "SetAgentTaskOwnershipV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                value(agent.set_task_ownership(
                    self.generation,
                    string_arg(arguments, 1)?,
                    u64_arg(arguments, 2)?,
                    bool_arg(arguments, 3)?,
                )?)
            }
            "SetAgentWorktreeV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                value(agent.set_worktree(
                    self.generation,
                    string_arg(arguments, 1)?,
                    u64_arg(arguments, 2)?,
                    string_arg(arguments, 3)?,
                    bool_arg(arguments, 4)?,
                )?)
            }
            "PrepareAgentWorkflowV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                let kind = parse_workflow_kind(string_arg(arguments, 3)?)?;
                value(agent.prepare_workflow(
                    self.generation,
                    string_arg(arguments, 1)?,
                    u64_arg(arguments, 2)?,
                    kind,
                    string_arg(arguments, 4)?,
                )?)
            }
            "ApproveAgentWorkflowV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                value(agent.approve_workflow(self.generation, string_arg(arguments, 1)?)?)
            }
            "DismissAgentWorkflowV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                value(agent.dismiss_workflow(self.generation, string_arg(arguments, 1)?)?)
            }
            _ => Err(unavailable(method)),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn invoke_capability(&self, method: &str, arguments: &[Value]) -> AppResult<Value> {
        let generation = u64_arg(arguments, 0)?;
        self.require_generation(generation)?;
        match method {
            "GetCapabilitiesV2" => {
                let store = self.project_store()?;
                let capabilities = store
                    .capabilities()?
                    .iter()
                    .map(capability_view)
                    .collect::<AppResult<Vec<_>>>()?;
                Ok(json!({ "generation": self.generation, "capabilities": capabilities }))
            }
            "PreviewCapabilityV2" => {
                let capability = capability_draft_arg(arguments, 1)?;
                let preview = normalize(&capability).map_err(message)?;
                Ok(json!({
                    "generation": self.generation,
                    "view": capability_preview_view(&preview.capability, &preview.effective_scope, "draft")?
                }))
            }
            "SaveCapabilityV2" => {
                let draft = capability_draft_arg(arguments, 1)?;
                let preview = normalize(&draft).map_err(message)?;
                let store = self.project_store()?;
                let saved = if draft.id == 0 {
                    store.add_capability(preview.capability)?
                } else {
                    let stored = store.capability(draft.id)?;
                    let mut candidate = preview.capability;
                    candidate.id = stored.id;
                    candidate.revision = stored.revision;
                    candidate.enabled = stored.enabled;
                    candidate.approved_at = stored.approved_at;
                    candidate.expires_at = stored.expires_at;
                    store.update_capability(candidate)?
                };
                self.revoke_capability(saved.id);
                Ok(json!({ "generation": self.generation, "view": capability_view(&saved)? }))
            }
            "EnableCapabilityV2" => {
                let id = u64_arg(arguments, 1)?;
                let expected = string_arg(arguments, 2)?;
                let store = self.project_store()?;
                let stored = store.capability(id)?;
                let wire = CapabilityWire::try_from(&stored).map_err(message)?;
                if wire.scope_digest != expected {
                    return Err(AppError::Message(
                        "effective scope changed; preview again before enabling".to_owned(),
                    ));
                }
                let proof = confirm_approval(&stored, stored.scope_digest).map_err(message)?;
                let saved = store.approve_capability(proof)?;
                self.revoke_capability(id);
                Ok(json!({ "generation": self.generation, "view": capability_view(&saved)? }))
            }
            "DisableCapabilityV2" | "ExpireCapabilityV2" | "RemoveCapabilityV2" => {
                let id = u64_arg(arguments, 1)?;
                let store = self.project_store()?;
                let stored = store.capability(id)?;
                let result = match method {
                    "DisableCapabilityV2" => Some(store.disable_capability(id, stored.revision)?),
                    "ExpireCapabilityV2" => Some(store.expire_capability(id, stored.revision)?),
                    _ => {
                        store.delete_capability(id, stored.revision)?;
                        None
                    }
                };
                self.revoke_capability(id);
                result.map_or_else(
                    || Ok(json!({ "generation": self.generation })),
                    |saved| {
                        Ok(json!({
                            "generation": self.generation,
                            "view": capability_view(&saved)?
                        }))
                    },
                )
            }
            "GetCapabilityAuditsV2" => {
                let id = u64_arg(arguments, 1)?;
                let requested = i64_arg(arguments, 2)?;
                let limit = if (1..=100).contains(&requested) {
                    usize::try_from(requested).unwrap_or(25)
                } else {
                    25
                };
                let store = self.project_store()?;
                let audits = store
                    .capability_audits(id, limit)?
                    .iter()
                    .map(CapabilityAuditWire::try_from)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(message)?;
                Ok(json!({ "generation": self.generation, "audits": audits }))
            }
            "TestCapabilityV2" => {
                let draft = capability_draft_arg(arguments, 1)?;
                let ssh_id = u64_arg(arguments, 2)?;
                let ssh = if ssh_id == 0 {
                    None
                } else {
                    Some(self.project_store()?.capability(ssh_id)?)
                };
                let diagnostic = run_capability_diagnostic(draft, ssh, self.endpoint.root.clone())?;
                Ok(json!({ "generation": self.generation, "diagnostic": diagnostic }))
            }
            _ => Err(unavailable(method)),
        }
    }

    fn revoke_capability(&self, id: u64) {
        if let Some(broker) = &self.broker {
            broker.revoke_capability(id);
        }
    }
}

impl DesktopWorkspace for BoundDesktopWorkspace {
    fn project(&self) -> WorkspaceProject {
        WorkspaceProject {
            name: self
                .endpoint
                .root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned(),
            root: self.endpoint.root.to_string_lossy().into_owned(),
            db_path: self.endpoint.database.to_string_lossy().into_owned(),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn invoke(&self, method: &str, arguments: &[Value]) -> AppResult<Value> {
        let _workspace_call = self.begin_workspace_call()?;
        match method {
            "GetBoard" => value(self.board(u64_arg(arguments, 0)?)?),
            "GetBoardV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                Ok(
                    json!({ "generation": self.generation, "board": self.board(u64_arg(arguments, 1)?)? }),
                )
            }
            "CreateFirstPlanV1" => {
                require_argument_count(method, arguments, 2)?;
                let generation = u64_arg(arguments, 0)?;
                self.require_exact_generation(generation)?;
                let title = first_run_title(string_arg(arguments, 1)?, "plan")?;
                let plan = self.project_store()?.create_first_plan(title)?;
                value(CreateFirstPlanResultV1 {
                    plan: first_plan_view(&plan),
                    state: self.first_run_state(),
                })
            }
            "CreateFirstTaskV1" => {
                require_argument_count(method, arguments, 3)?;
                let generation = u64_arg(arguments, 0)?;
                self.require_exact_generation(generation)?;
                let plan_id = u64_arg(arguments, 1)?;
                let title = first_run_title(string_arg(arguments, 2)?, "task")?;
                let task = self.project_store()?.create_first_task(plan_id, title)?;
                value(CreateFirstTaskResultV1 {
                    task: first_task_view(&task),
                    state: self.first_run_state(),
                })
            }
            "AddTask" | "AddTaskV2" => {
                let (generation, offset) = if method == "AddTaskV2" {
                    (u64_arg(arguments, 0)?, 1)
                } else {
                    (0, 0)
                };
                self.require_generation(generation)?;
                let title = trimmed_nonempty(
                    string_arg(arguments, offset + 1)?,
                    "task title cannot be empty",
                )?;
                let result = lock(&self.application).mutate(Mutation::AddTask {
                    plan_id: u64_arg(arguments, offset)?,
                    title,
                })?;
                let MutationResult::Task(task) = result else {
                    return Err(unavailable("task mutation"));
                };
                let card = task_card(&self.snapshot()?, &task);
                if method == "AddTaskV2" {
                    Ok(json!({ "generation": self.generation, "task": card }))
                } else {
                    value(card)
                }
            }
            "RenameTask" | "RenameTaskV2" => {
                let (generation, offset) = if method == "RenameTaskV2" {
                    (u64_arg(arguments, 0)?, 1)
                } else {
                    (0, 0)
                };
                self.require_generation(generation)?;
                let title = trimmed_nonempty(
                    string_arg(arguments, offset + 1)?,
                    "task title cannot be empty",
                )?;
                lock(&self.application).mutate(Mutation::SetTaskTitle {
                    id: u64_arg(arguments, offset)?,
                    title,
                })?;
                if method == "RenameTaskV2" {
                    Ok(json!({ "generation": self.generation }))
                } else {
                    Ok(Value::Null)
                }
            }
            "AddTaskNote" | "AddTaskNoteV2" => {
                let (generation, offset) = if method == "AddTaskNoteV2" {
                    (u64_arg(arguments, 0)?, 1)
                } else {
                    (0, 0)
                };
                self.require_generation(generation)?;
                let body = trimmed_nonempty(
                    string_arg(arguments, offset + 1)?,
                    "memory note cannot be empty",
                )?;
                let task_id = u64_arg(arguments, offset)?;
                if self.snapshot()?.task(task_id).is_none() {
                    return Err(AppError::Message(format!("task #{task_id} not found")));
                }
                lock(&self.application).mutate(Mutation::AddNote {
                    target: NoteTarget::Task,
                    target_id: task_id,
                    body,
                })?;
                if method == "AddTaskNoteV2" {
                    Ok(json!({ "generation": self.generation }))
                } else {
                    Ok(Value::Null)
                }
            }
            "MoveTask" | "MoveTaskV2" | "MoveTaskV3" => {
                let (generation, offset) = if method == "MoveTask" {
                    (0, 0)
                } else {
                    (u64_arg(arguments, 0)?, 1)
                };
                self.require_generation(generation)?;
                let task_id = u64_arg(arguments, offset)?;
                let status = parse_task_status(string_arg(arguments, offset + 1)?)?;
                let confirmation = if method == "MoveTaskV3" {
                    string_arg(arguments, offset + 2)?
                } else {
                    ""
                };
                let result = self.move_task_v3(task_id, status, confirmation)?;
                if method != "MoveTaskV3"
                    && !result
                        .get("applied")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                {
                    return Err(AppError::Message(
                        "task transition confirmation is required".to_owned(),
                    ));
                }
                if method == "MoveTaskV3" {
                    Ok(result)
                } else if method == "MoveTaskV2" {
                    Ok(json!({ "generation": self.generation }))
                } else {
                    Ok(Value::Null)
                }
            }
            "SearchV2" => value(search(&self.snapshot()?, string_arg(arguments, 0)?)),
            "StartFirstTaskV1" => {
                require_argument_count(method, arguments, 3)?;
                let generation = u64_arg(arguments, 0)?;
                self.require_exact_generation(generation)?;
                let task =
                    self.start_first_task_v1(u64_arg(arguments, 1)?, string_arg(arguments, 2)?)?;
                value(CreateFirstTaskResultV1 {
                    task: first_task_view(&task),
                    state: self.first_run_state(),
                })
            }
            "GetActivityHeatmapV2" => value(heatmap(&self.snapshot()?, i64_arg(arguments, 0)?)),
            "GetTaskDetailV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                value(self.task_detail(&self.snapshot()?, u64_arg(arguments, 1)?)?)
            }
            "GetWorkspaceSnapshot" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                let plan_id = u64_arg(arguments, 1)?;
                let deadline = Instant::now() + WORKSPACE_SNAPSHOT_TIMEOUT;
                let mut tracking = self.bounded_snapshot_tracking(plan_id, deadline)?;
                let git = capture_git_snapshot(&self.endpoint.root, deadline);
                ensure_snapshot_deadline(deadline)?;
                let agent_sections = self.agent.as_ref().map_or_else(
                    || Ok(None),
                    |agent| {
                        agent
                            .workspace_snapshot(self.generation, &git.agent, deadline)
                            .map(Some)
                    },
                )?;
                let candidates = agent_sections.as_ref().map_or_else(
                    || empty_agent_candidates(self.generation),
                    |sections| sections.runtime.clone(),
                );
                let projection = self.runtime_projection_with_agents_until(
                    &tracking.snapshot,
                    candidates,
                    Some(deadline),
                )?;
                apply_linked_runtime_to_board(&mut tracking.board, &projection);
                Self::workspace_snapshot(
                    self.generation,
                    &self.project(),
                    tracking,
                    &projection,
                    agent_sections.as_ref(),
                    &git.wire,
                    deadline,
                )
            }
            "GetRecentProjects" => {
                let projects = lock(&self.application).projects()?;
                let recent = projects
                    .into_iter()
                    .take(RECENT_PROJECT_LIMIT)
                    .map(|project| {
                        let available = Path::new(&project.path).is_dir()
                            && find_project_database(&project.path).is_ok();
                        json!({
                            "name": project.name,
                            "path": project.path,
                            "lastSeen": timestamp(project.last_seen),
                            "available": available
                        })
                    })
                    .collect::<Vec<_>>();
                Ok(Value::Array(recent))
            }
            "GetTerminalProfiles" | "GetTerminalProfilesV2" => {
                let terminal = self
                    .terminal
                    .as_ref()
                    .ok_or_else(|| unavailable("terminal manager"))?;
                let generation = if method == "GetTerminalProfilesV2" {
                    u64_arg(arguments, 0)?
                } else {
                    0
                };
                self.require_generation(generation)?;
                let profiles = terminal.profiles(self.generation)?;
                if method == "GetTerminalProfilesV2" {
                    value(profiles)
                } else {
                    value(profiles.profiles)
                }
            }
            "ValidateTerminalCWDsV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                let cwds = string_vec_arg(arguments, 1)?;
                value(
                    self.terminal
                        .as_ref()
                        .ok_or_else(|| unavailable("terminal manager"))?
                        .validate_cwds(self.generation, &cwds)?,
                )
            }
            "LaunchLinkedAgentV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                let _admission = self.begin_resource_admission()?;
                let profile_id = string_arg(arguments, 1)?;
                if profile_id.is_empty() || profile_id.trim() != profile_id {
                    return Err(AppError::Message(
                        "an installed agent profile is required".to_owned(),
                    ));
                }
                let pointer = association_pointer_arg(arguments, 5)?;
                let snapshot = self.snapshot()?;
                validate_association_pointer(&snapshot, pointer)?;
                let _transition = lock(&self.resource_transition);
                let terminal = self
                    .terminal
                    .as_ref()
                    .ok_or_else(|| unavailable("terminal manager"))?;
                let profiles = terminal.profiles(self.generation).map_err(|error| {
                    AppError::Message(format!("discover installed agent profiles: {error}"))
                })?;
                let profile = profiles
                    .profiles
                    .iter()
                    .find(|profile| profile.id == profile_id)
                    .ok_or_else(|| {
                        AppError::Message(format!(
                            "installed agent profile {profile_id:?} is unavailable"
                        ))
                    })?;
                if profile.kind != ptrack_terminal::ProfileKind::Agent {
                    return Err(AppError::Message(format!(
                        "terminal profile {profile_id:?} is not an agent"
                    )));
                }
                let agent = self
                    .agent
                    .as_ref()
                    .ok_or_else(|| unavailable("AgentRun registry"))?;
                let cwd_value = string_arg(arguments, 2)?;
                if cwd_value.len() > 4_096 {
                    return Err(AppError::Message(
                        "linked launch working directory is too long".to_owned(),
                    ));
                }
                let cwd = self.resolve_linked_launch_cwd(cwd_value)?;
                let context_store = ProjectLaunchContextStore {
                    root: &self.endpoint.root,
                    snapshot: &snapshot,
                };
                let host = AssociationHost::new(
                    &self.endpoint.root,
                    self.generation,
                    Some(&context_store),
                )
                .map_err(|error| AppError::Message(error.to_string()))?;
                let context = build_launch_context(
                    Some(&context_store),
                    Some(&host),
                    AgentAssociationPointer {
                        version: pointer.version,
                        plan_id: pointer.plan_id,
                        task_id: pointer.task_id,
                    },
                )
                .map_err(|error| AppError::Message(error.to_string()))?;
                let result = terminal.create_linked(
                    self.generation,
                    profile_id,
                    Some(&cwd),
                    u16_arg(arguments, 3)?,
                    u16_arg(arguments, 4)?,
                    pointer,
                    &context.text,
                )?;
                confirm_linked_launch(
                    agent.has_linked_terminal(self.generation, &result.session_id),
                    || terminal.rollback_failed_linked(self.generation, &result.session_id),
                )?;
                value(result)
            }
            "RollbackLinkedAgentLaunchV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                let session_id = string_arg(arguments, 1)?;
                let _transition = lock(&self.resource_transition);
                let agent = self
                    .agent
                    .as_ref()
                    .ok_or_else(|| unavailable("AgentRun registry"))?;
                if !agent.has_linked_terminal(self.generation, session_id)? {
                    return Err(AppError::Message(
                        "linked agent launch is unavailable".to_owned(),
                    ));
                }
                self.terminal
                    .as_ref()
                    .ok_or_else(|| unavailable("terminal manager"))?
                    .rollback_linked(self.generation, session_id)?;
                Ok(Value::Null)
            }
            "CreateTerminal" | "CreateTerminalV2" => {
                let (generation, offset) = if method == "CreateTerminalV2" {
                    (u64_arg(arguments, 0)?, 1)
                } else {
                    (0, 0)
                };
                self.require_generation(generation)?;
                let _admission = self.begin_resource_admission()?;
                let _transition = lock(&self.resource_transition);
                let cwd_value = string_arg(arguments, offset + 1)?;
                let cwd = if cwd_value.is_empty() {
                    None
                } else {
                    Some(Path::new(cwd_value))
                };
                let result = self
                    .terminal
                    .as_ref()
                    .ok_or_else(|| unavailable("terminal manager"))?
                    .create(
                        self.generation,
                        string_arg(arguments, offset)?,
                        cwd,
                        u16_arg(arguments, offset + 2)?,
                        u16_arg(arguments, offset + 3)?,
                    )?;
                if method == "CreateTerminalV2" {
                    value(result)
                } else {
                    Ok(json!({
                        "sessionId": result.session_id,
                        "profileId": result.profile_id,
                        "cwd": result.cwd,
                        "state": result.state,
                        "streamUrl": result.stream_url
                    }))
                }
            }
            "ResizeTerminal" | "ResizeTerminalV2" => {
                let (generation, offset) = if method == "ResizeTerminalV2" {
                    (u64_arg(arguments, 0)?, 1)
                } else {
                    (0, 0)
                };
                self.require_generation(generation)?;
                self.terminal
                    .as_ref()
                    .ok_or_else(|| unavailable("terminal manager"))?
                    .resize(
                        self.generation,
                        string_arg(arguments, offset)?,
                        // The renderer holds no lease to present until the
                        // pop-out window work wires one through; the manager
                        // names that gap and borrows the live lease meanwhile.
                        None,
                        u16_arg(arguments, offset + 1)?,
                        u16_arg(arguments, offset + 2)?,
                    )?;
                if method == "ResizeTerminalV2" {
                    Ok(json!({ "generation": self.generation }))
                } else {
                    Ok(Value::Null)
                }
            }
            // Fenced by the bound workspace generation, so a ticket can never
            // be minted for a session belonging to a superseded project.
            "ClaimTerminalStream" => {
                require_argument_count(method, arguments, 2)?;
                value(
                    self.terminal
                        .as_ref()
                        .ok_or_else(|| unavailable("terminal manager"))?
                        .claim_stream_ticket(
                            self.generation,
                            string_arg(arguments, 0)?,
                            u64_arg(arguments, 1)?,
                        )?,
                )
            }
            "CloseTerminal" | "CloseTerminalV2" => {
                let (generation, offset) = if method == "CloseTerminalV2" {
                    (u64_arg(arguments, 0)?, 1)
                } else {
                    (0, 0)
                };
                self.require_generation(generation)?;
                self.terminal
                    .as_ref()
                    .ok_or_else(|| unavailable("terminal manager"))?
                    .close(
                        self.generation,
                        string_arg(arguments, offset)?,
                        bool_arg(arguments, offset + 1)?,
                    )?;
                if method == "CloseTerminalV2" {
                    Ok(json!({ "generation": self.generation }))
                } else {
                    Ok(Value::Null)
                }
            }
            "AssociateTerminalV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                let pointer = association_pointer_arg(arguments, 2)?;
                validate_association_pointer(&self.snapshot()?, pointer)?;
                let _transition = lock(&self.resource_transition);
                if let Some(agent) = &self.agent
                    && agent.has_linked_terminal(self.generation, string_arg(arguments, 1)?)?
                {
                    return Err(AppError::Message(
                        "linked terminal association requires a revision-fenced mutation"
                            .to_owned(),
                    ));
                }
                let association = self
                    .terminal
                    .as_ref()
                    .ok_or_else(|| unavailable("terminal association manager"))?
                    .associate(self.generation, string_arg(arguments, 1)?, pointer)?;
                value(association)
            }
            "MutateTerminalAssociationV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                let detach = bool_arg(arguments, 3)?;
                let pointer = if detach {
                    TerminalAssociationPointer {
                        version: 1,
                        plan_id: 0,
                        task_id: 0,
                    }
                } else {
                    association_pointer_arg(arguments, 4)?
                };
                if !detach && pointer.plan_id == 0 {
                    return Err(AppError::Message(
                        "invalid association target: relink requires a plan or task".to_owned(),
                    ));
                }
                validate_association_pointer(&self.snapshot()?, pointer)?;
                let _transition = lock(&self.resource_transition);
                let session_id = string_arg(arguments, 1)?;
                let terminal = self
                    .terminal
                    .as_ref()
                    .ok_or_else(|| unavailable("terminal association manager"))?;
                let change = terminal.prepare_association_change(
                    self.generation,
                    session_id,
                    u64_arg(arguments, 2)?,
                    pointer,
                )?;
                if detach
                    && change
                        .previous
                        .as_ref()
                        .is_none_or(|value| value.pointer.plan_id == 0)
                {
                    return Err(AppError::Message(
                        "invalid association target: terminal is already detached".to_owned(),
                    ));
                }
                let previous = change.previous.as_ref().map(|association| {
                    agent_association(
                        &self.endpoint.root,
                        self.generation,
                        session_id,
                        association,
                    )
                });
                let next = agent_association(
                    &self.endpoint.root,
                    self.generation,
                    session_id,
                    &change.next,
                );
                let agent_change = if let Some(agent) = &self.agent {
                    agent.prepare_linked_association(
                        self.generation,
                        session_id,
                        previous.as_ref(),
                        &next,
                        AgentAssociationPointer {
                            version: pointer.version,
                            plan_id: pointer.plan_id,
                            task_id: pointer.task_id,
                        },
                    )?
                } else {
                    None
                };
                let _event_suppression = self
                    .agent
                    .as_ref()
                    .map(|agent| agent.suppress_runtime_event(self.generation))
                    .transpose()?;
                terminal.commit_association_change(self.generation, &change)?;
                if let Some(agent_change) = &agent_change
                    && let Some(agent) = &self.agent
                    && let Err(error) =
                        agent.commit_linked_association(self.generation, agent_change)
                {
                    let rollback = terminal.rollback_association_change(self.generation, &change);
                    return Err(AppError::Message(match rollback {
                        Ok(()) => error.to_string(),
                        Err(rollback) => format!("{error}\n{rollback}"),
                    }));
                }
                terminal.association_changed(self.generation)?;
                let mut result = json!({
                    "generation": self.generation,
                    "sessionId": session_id,
                    "revision": change.next.revision,
                    "detached": detach
                });
                if !detach {
                    result["pointer"] = serde_json::to_value(pointer).map_err(message)?;
                }
                Ok(result)
            }
            "PreviewTerminalWritebackV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                let session_id = string_arg(arguments, 1)?;
                let revision = u64_arg(arguments, 2)?;
                let kind = memory_kind_arg(arguments, 3)?;
                let content = validate_writeback_content(string_arg(arguments, 4)?)?;
                let association = self
                    .terminal
                    .as_ref()
                    .ok_or_else(|| unavailable("terminal write-back"))?
                    .live_association(self.generation, session_id, revision)?;
                let destination =
                    writeback_destination(&self.snapshot()?, association.pointer, kind)?;
                Ok(json!({
                    "generation": self.generation,
                    "sessionId": session_id,
                    "revision": association.revision,
                    "kind": kind.as_str(),
                    "content": content,
                    "contentBytes": content.len(),
                    "associationTarget": writeback_target_label(association.pointer),
                    "destination": destination,
                    "replacesSummary": kind == MemoryKind::Summary
                }))
            }
            "WriteTerminalMemoryV2" => {
                let generation = u64_arg(arguments, 0)?;
                self.require_generation(generation)?;
                let session_id = string_arg(arguments, 1)?;
                let revision = u64_arg(arguments, 2)?;
                let request_id = string_arg(arguments, 3)?;
                let kind = memory_kind_arg(arguments, 4)?;
                let content = validate_writeback_content(string_arg(arguments, 5)?)?;
                if kind == MemoryKind::Summary && !bool_arg(arguments, 6)? {
                    return Err(AppError::Message(
                        "summary replacement requires explicit confirmation".to_owned(),
                    ));
                }
                let association = self
                    .terminal
                    .as_ref()
                    .ok_or_else(|| unavailable("terminal write-back"))?
                    .live_association(self.generation, session_id, revision)?;
                let snapshot = self.snapshot()?;
                let destination = writeback_destination(&snapshot, association.pointer, kind)?;
                let (target, target_id, plan_id) = writeback_target(association.pointer, kind);
                let store = self.project_store()?;
                let result = store.write_memory(MemoryWriteRequest {
                    request_id: request_id.to_owned(),
                    kind,
                    body: content,
                    target,
                    target_id,
                    plan_id,
                    workspace_generation: self.generation,
                    session_id: session_id.to_owned(),
                    association_revision: association.revision,
                })?;
                Ok(json!({
                    "generation": self.generation,
                    "sessionId": session_id,
                    "revision": association.revision,
                    "requestId": request_id,
                    "kind": kind.as_str(),
                    "destination": destination,
                    "noteId": result.note.as_ref().map_or(0, |note| note.id),
                    "replayed": result.replayed
                }))
            }
            method if routes_to_capability(method) => self.invoke_capability(method, arguments),
            method if method.contains("Agent") => self.invoke_agent(method, arguments),
            _ => Err(unavailable(method)),
        }
    }

    fn active_resources(&self) -> AppResult<ActiveResourceSummary> {
        let (terminals, terminal_revision) =
            self.terminal.as_ref().map_or(Ok((0, 0)), |terminal| {
                Ok::<_, AppError>((
                    terminal.active_session_count(self.generation)?,
                    terminal.resource_revision(self.generation)?,
                ))
            })?;
        let (agent_runs, agent_revision) = self.agent.as_ref().map_or(Ok((0, 0)), |agent| {
            let state = agent.resource_state(self.generation)?;
            Ok::<_, AppError>((state.active_runs, state.resource_revision))
        })?;
        let admission = lock(&self.resource_admission.state);
        let pending_admissions = admission.pending;
        let admission_revision = admission.revision;
        drop(admission);
        Ok(ActiveResourceSummary {
            terminals,
            agent_runs,
            pending_admissions,
            resource_revision: terminal_revision
                .saturating_add(agent_revision)
                .saturating_add(admission_revision),
        })
    }

    fn fence_resource_admission(&self) -> AppResult<DesktopAdmissionFence> {
        {
            let mut state = lock(&self.resource_admission.state);
            state.fences = state.fences.saturating_add(1);
        }
        let resource = ResourceAdmissionFence(Arc::clone(&self.resource_admission));
        let agent = self
            .agent
            .as_ref()
            .map(|agent| agent.fence_admission(self.generation))
            .transpose()?;
        Ok(DesktopAdmissionFence {
            _resource: Some(resource),
            _agent: agent,
        })
    }

    fn drain_runtime_invalidations(&self) -> AppResult<bool> {
        self.agent.as_ref().map_or(Ok(false), |agent| {
            Ok(agent.drain_invalidations(self.generation)?.event_count != 0)
        })
    }

    fn shutdown(&self) -> AppResult<()> {
        let mut errors = Vec::new();
        if !self.close_workspace_calls() {
            errors.push("workspace runtime calls did not stop before cleanup deadline".to_owned());
        }
        if let Some(terminal) = &self.terminal
            && let Err(error) = shutdown_terminal(Arc::clone(terminal))
        {
            errors.push(error.to_string());
        }
        if let Some(agent) = &self.agent
            && let Err(error) = agent.shutdown()
        {
            errors.push(error.to_string());
        }
        if let Some(broker) = &self.broker {
            broker.shutdown();
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(AppError::Message(errors.join("\n")))
        }
    }
}

fn terminal_task_resources(
    snapshot: &ProjectSnapshot,
    task_id: u64,
    sessions: &[SessionInfo],
) -> Vec<TaskResource> {
    let plan_id = snapshot.task(task_id).map(|task| task.plan_id);
    sessions
        .iter()
        .filter(|session| {
            matches!(
                session.state,
                SessionState::Starting | SessionState::Running | SessionState::Closing
            )
        })
        .filter_map(|session| {
            let association = session.association.as_ref()?;
            (association.revision != 0
                && association.pointer.task_id == task_id
                && Some(association.pointer.plan_id) == plan_id)
                .then(|| TaskResource {
                    kind: "terminal",
                    id: session.id.clone(),
                    revision: association.revision,
                    state: session.state.to_string(),
                    process_state: String::new(),
                    lease_state: String::new(),
                    lifecycle_revision: 0,
                })
        })
        .collect()
}

fn agent_task_resources(
    snapshot: &ProjectSnapshot,
    project_root: &Path,
    generation: u64,
    task_id: u64,
    runs: &[Run],
) -> Vec<TaskResource> {
    let plan_id = snapshot.task(task_id).map(|task| task.plan_id);
    runs.iter()
        .filter(|run| agent_run_is_live(run))
        .filter_map(|run| {
            let association = run.association.as_ref()?;
            (association.version == 1
                && association.project_root == project_root.to_string_lossy()
                && association.generation == generation
                && association.live_id == run.id
                && association.revision != 0
                && association.target.task_id == task_id
                && Some(association.target.plan_id) == plan_id)
                .then(|| TaskResource {
                    kind: "agent",
                    id: run.id.clone(),
                    revision: association.revision,
                    state: run.state.as_str().to_owned(),
                    process_state: run.process_state.as_str().to_owned(),
                    lease_state: run.lease_state.as_str().to_owned(),
                    lifecycle_revision: run.lifecycle_revision,
                })
        })
        .collect()
}

fn agent_run_is_live(run: &Run) -> bool {
    if run.state != RunState::Running || run.process_state == ProcessState::Exited {
        return false;
    }
    match run.registration_kind {
        RegistrationKind::External => run.lease_state == LeaseState::Active,
        RegistrationKind::Launched => run.process_state == ProcessState::Running,
        RegistrationKind::Unset => false,
    }
}

fn agent_association(
    project_root: &Path,
    generation: u64,
    live_id: &str,
    association: &TerminalAssociation,
) -> ptrack_agent::Association {
    ptrack_agent::Association {
        version: association.pointer.version,
        project_root: project_root.to_string_lossy().into_owned(),
        generation,
        live_id: live_id.to_owned(),
        target: ptrack_agent::AssociationTarget {
            plan_id: association.pointer.plan_id,
            task_id: association.pointer.task_id,
        },
        revision: association.revision,
    }
}

fn task_transition_base(generation: u64, task: &Task, wanted: TaskStatus) -> Value {
    json!({
        "generation": generation,
        "taskId": task.id,
        "fromStatus": task.status.as_str(),
        "toStatus": wanted.as_str()
    })
}

fn task_transition_applied(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("applied".to_owned(), Value::Bool(true));
        object.insert("requiresConfirmation".to_owned(), Value::Bool(false));
    }
    value
}

fn invalid_task_confirmation() -> AppError {
    AppError::Message("task transition confirmation is invalid or stale".to_owned())
}

fn shutting_down() -> AppError {
    AppError::Message("terminal lifecycle is shutting down".to_owned())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct WorkspaceFileState {
    exists: bool,
    size: u64,
    modified: Option<SystemTime>,
}

fn workspace_file_state(path: &Path) -> WorkspaceFileState {
    fs::metadata(path).map_or_else(
        |_| WorkspaceFileState::default(),
        |metadata| WorkspaceFileState {
            exists: true,
            size: metadata.len(),
            modified: metadata.modified().ok(),
        },
    )
}

pub(super) fn watch_workspace_data(
    cancellation: &Receiver<()>,
    database: &Path,
    interval: Duration,
    debounce: Duration,
    mut emit: impl FnMut(),
) {
    let mut previous = workspace_file_state(database);
    let mut next_poll = Instant::now() + interval;
    let mut pending = None;
    loop {
        let deadline = pending.map_or(next_poll, |pending: Instant| pending.min(next_poll));
        let wait = deadline.saturating_duration_since(Instant::now());
        match cancellation.recv_timeout(wait) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        if now >= next_poll {
            next_poll = now + interval;
            let current = workspace_file_state(database);
            if current != previous {
                previous = current;
                pending = Some(now + debounce);
            }
        }
        if pending.is_some_and(|deadline| now >= deadline) {
            pending = None;
            emit();
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanSummaryView {
    id: u64,
    title: String,
    is_active: bool,
    tasks_total: usize,
    tasks_done: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskView {
    id: u64,
    title: String,
    status: String,
    updated_at: String,
    note_count: usize,
    commit_count: usize,
    issue_count: usize,
    latest_note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    linked_runtime: Option<TaskLinkedRuntimeSummaryView>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskLinkedRuntimeSummaryView {
    terminals: usize,
    live_terminals: usize,
    agents: usize,
    live_agents: usize,
    terminal_backed_runs: usize,
    external_runs: usize,
    truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalRuntimeSummaryView {
    session_id: String,
    profile_kind: ptrack_terminal::ProfileKind,
    state: SessionState,
    live: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    association: Option<RuntimeAssociation>,
}

struct RuntimeProjectionView {
    terminals: Vec<TerminalRuntimeSummaryView>,
    terminal_total: usize,
    agents: Vec<AgentRuntimeSummary>,
    agent_total: usize,
    sources_truncated: bool,
}

struct SnapshotTrackingBounds {
    plans: usize,
    tasks: usize,
    blockers: usize,
    notes: usize,
    activity: usize,
    issues: usize,
}

struct SnapshotTrackingCapture {
    snapshot: ProjectSnapshot,
    board: BoardView,
    blockers: Vec<TaskView>,
    bounds: SnapshotTrackingBounds,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskLinkedRuntimeDetailView {
    summary: TaskLinkedRuntimeSummaryView,
    terminals: Vec<TerminalRuntimeSummaryView>,
    agents: Vec<AgentRuntimeSummary>,
    terminal_rows_more: usize,
    agent_rows_more: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ColumnView {
    status: String,
    title: String,
    tasks: Vec<TaskView>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectStatsView {
    plan_tasks: usize,
    plan_tasks_done: usize,
    tasks_open: usize,
    tasks_blocked: usize,
    notes: usize,
    commits: usize,
    open_issues: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivityView {
    kind: String,
    title: String,
    detail: String,
    target: String,
    occurred_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct IssueView {
    id: u64,
    title: String,
    severity: String,
    task_id: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct BoardView {
    project_name: String,
    goal: String,
    summary: String,
    plans: Vec<PlanSummaryView>,
    plan_id: u64,
    plan_title: String,
    columns: Vec<ColumnView>,
    stats: ProjectStatsView,
    activity: Vec<ActivityView>,
    open_issues: Vec<IssueView>,
}

#[allow(clippy::too_many_lines)]
fn board_view(
    snapshot: &ProjectSnapshot,
    project_name: String,
    requested_plan: u64,
) -> AppResult<BoardView> {
    let plan_id = if requested_plan == 0 {
        snapshot.meta.active_plan
    } else {
        requested_plan
    };
    if plan_id == 0 {
        return Err(AppError::Message(
            "no active plan; set one with 'ptrack plan use <id>' or pass --plan".to_owned(),
        ));
    }
    let selected = snapshot
        .plan(plan_id)
        .ok_or_else(|| AppError::Message(format!("plan #{plan_id} not found")))?;
    let tasks = snapshot.tasks_for_plan(plan_id).collect::<Vec<_>>();
    let task_ids = tasks.iter().map(|task| task.id).collect::<BTreeSet<_>>();
    let mut note_counts = BTreeMap::new();
    let mut commit_counts = BTreeMap::new();
    let mut issue_counts = BTreeMap::new();
    let mut latest_notes = BTreeMap::new();
    for note in &snapshot.notes {
        if note.target == NoteTarget::Task && task_ids.contains(&note.target_id) {
            *note_counts.entry(note.target_id).or_insert(0) += 1;
            latest_notes.insert(note.target_id, note.body.clone());
        }
    }
    for commit in &snapshot.commits {
        if task_ids.contains(&commit.task_id) {
            *commit_counts.entry(commit.task_id).or_insert(0) += 1;
        }
    }
    let mut open_issues = Vec::new();
    for issue in &snapshot.issues {
        if issue.status == IssueStatus::Open {
            if task_ids.contains(&issue.task_id) {
                *issue_counts.entry(issue.task_id).or_insert(0) += 1;
            }
            open_issues.push(IssueView {
                id: issue.id,
                title: issue.title.clone(),
                severity: issue.severity.as_str().to_owned(),
                task_id: issue.task_id,
            });
        }
    }
    open_issues.truncate(5);
    let statuses = [
        (TaskStatus::Todo, "Todo"),
        (TaskStatus::Doing, "Doing"),
        (TaskStatus::Blocked, "Blocked"),
        (TaskStatus::Done, "Done"),
    ];
    let columns = statuses
        .into_iter()
        .map(|(status, title)| ColumnView {
            status: status.as_str().to_owned(),
            title: title.to_owned(),
            tasks: tasks
                .iter()
                .filter(|task| task.status == status)
                .map(|task| TaskView {
                    id: task.id,
                    title: task.title.clone(),
                    status: task.status.as_str().to_owned(),
                    updated_at: timestamp(task.updated_at),
                    note_count: *note_counts.get(&task.id).unwrap_or(&0),
                    commit_count: *commit_counts.get(&task.id).unwrap_or(&0),
                    issue_count: *issue_counts.get(&task.id).unwrap_or(&0),
                    latest_note: latest_notes.get(&task.id).cloned().unwrap_or_default(),
                    linked_runtime: None,
                })
                .collect(),
        })
        .collect();
    let counts = snapshot.counts();
    let mut progress = BTreeMap::<u64, (usize, usize)>::new();
    for task in &snapshot.tasks {
        let entry = progress.entry(task.plan_id).or_default();
        entry.0 += 1;
        if task.status == TaskStatus::Done {
            entry.1 += 1;
        }
    }
    let plans = snapshot
        .plans
        .iter()
        .map(|plan| {
            let entry = progress.get(&plan.id).copied().unwrap_or_default();
            PlanSummaryView {
                id: plan.id,
                title: plan.title.clone(),
                is_active: plan.id == snapshot.meta.active_plan,
                tasks_total: entry.0,
                tasks_done: entry.1,
            }
        })
        .collect();
    let plan_done = tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Done)
        .count();
    Ok(BoardView {
        project_name,
        goal: snapshot.meta.goal.clone(),
        summary: snapshot.meta.summary.clone(),
        plans,
        plan_id,
        plan_title: selected.title.clone(),
        columns,
        stats: ProjectStatsView {
            plan_tasks: tasks.len(),
            plan_tasks_done: plan_done,
            tasks_open: counts.tasks_open,
            tasks_blocked: counts.tasks_blocked,
            notes: counts.notes,
            commits: counts.commits,
            open_issues: counts.issues_open,
        },
        activity: recent_activity(snapshot, plan_id, &task_ids),
        open_issues,
    })
}

fn snapshot_board_view(
    snapshot: &ProjectSnapshot,
    project_name: String,
    plan_id: u64,
) -> AppResult<BoardView> {
    if plan_id != 0 {
        return board_view(snapshot, project_name, plan_id);
    }
    let counts = snapshot.counts();
    let mut progress = BTreeMap::<u64, (usize, usize)>::new();
    for task in &snapshot.tasks {
        let entry = progress.entry(task.plan_id).or_default();
        entry.0 += 1;
        if task.status == TaskStatus::Done {
            entry.1 += 1;
        }
    }
    let plans = snapshot
        .plans
        .iter()
        .map(|plan| {
            let entry = progress.get(&plan.id).copied().unwrap_or_default();
            PlanSummaryView {
                id: plan.id,
                title: plan.title.clone(),
                is_active: false,
                tasks_total: entry.0,
                tasks_done: entry.1,
            }
        })
        .collect();
    let task_ids = BTreeSet::new();
    Ok(BoardView {
        project_name,
        goal: snapshot.meta.goal.clone(),
        summary: snapshot.meta.summary.clone(),
        plans,
        plan_id: 0,
        plan_title: String::new(),
        columns: [
            (TaskStatus::Todo, "Todo"),
            (TaskStatus::Doing, "Doing"),
            (TaskStatus::Blocked, "Blocked"),
            (TaskStatus::Done, "Done"),
        ]
        .into_iter()
        .map(|(status, title)| ColumnView {
            status: status.as_str().to_owned(),
            title: title.to_owned(),
            tasks: Vec::new(),
        })
        .collect(),
        stats: ProjectStatsView {
            plan_tasks: 0,
            plan_tasks_done: 0,
            tasks_open: counts.tasks_open,
            tasks_blocked: counts.tasks_blocked,
            notes: counts.notes,
            commits: counts.commits,
            open_issues: counts.issues_open,
        },
        activity: recent_activity(snapshot, 0, &task_ids),
        open_issues: snapshot
            .issues
            .iter()
            .filter(|issue| issue.status == IssueStatus::Open)
            .take(5)
            .map(|issue| IssueView {
                id: issue.id,
                title: issue.title.clone(),
                severity: issue.severity.as_str().to_owned(),
                task_id: issue.task_id,
            })
            .collect(),
    })
}

fn recent_activity(
    snapshot: &ProjectSnapshot,
    plan_id: u64,
    task_ids: &BTreeSet<u64>,
) -> Vec<ActivityView> {
    let mut events = Vec::<(Option<i128>, ActivityView)>::new();
    for note in &snapshot.notes {
        let relevant = note.target == NoteTarget::Project
            || (note.target == NoteTarget::Plan && note.target_id == plan_id)
            || (note.target == NoteTarget::Task && task_ids.contains(&note.target_id));
        if !relevant {
            continue;
        }
        let target = match note.target {
            NoteTarget::Project => "Project".to_owned(),
            NoteTarget::Plan => format!("Plan #{}", note.target_id),
            NoteTarget::Task => format!("Task #{}", note.target_id),
        };
        let kind = if note.kind == MemoryKind::Legacy {
            "note"
        } else {
            note.kind.as_str()
        };
        let title = match note.kind {
            MemoryKind::Decision => "Decision recorded",
            MemoryKind::Blocker => "Blocker recorded",
            MemoryKind::Handoff => "Handoff recorded",
            _ => "Memory recorded",
        };
        events.push((
            note.created_at.unix_nanoseconds(),
            ActivityView {
                kind: kind.to_owned(),
                title: title.to_owned(),
                detail: note.body.clone(),
                target,
                occurred_at: timestamp(note.created_at),
            },
        ));
    }
    for commit in &snapshot.commits {
        if commit.plan_id != plan_id && !task_ids.contains(&commit.task_id) {
            continue;
        }
        let detail = commit.sha.chars().take(8).collect();
        events.push((
            commit.created_at.unix_nanoseconds(),
            ActivityView {
                kind: "commit".to_owned(),
                title: commit.subject.clone(),
                detail,
                target: if commit.task_id == 0 {
                    format!("Plan #{plan_id}")
                } else {
                    format!("Task #{}", commit.task_id)
                },
                occurred_at: timestamp(commit.created_at),
            },
        ));
    }
    events.sort_by_key(|event| std::cmp::Reverse(event.0));
    events
        .into_iter()
        .take(24)
        .map(|(_, event)| event)
        .collect()
}

fn task_card(snapshot: &ProjectSnapshot, task: &Task) -> TaskView {
    let notes = snapshot.notes_for_task(task.id).collect::<Vec<_>>();
    TaskView {
        id: task.id,
        title: task.title.clone(),
        status: task.status.as_str().to_owned(),
        updated_at: timestamp(task.updated_at),
        note_count: notes.len(),
        commit_count: snapshot
            .commits
            .iter()
            .filter(|commit| commit.task_id == task.id)
            .count(),
        issue_count: snapshot
            .issues
            .iter()
            .filter(|issue| issue.task_id == task.id)
            .count(),
        latest_note: notes
            .last()
            .map_or_else(String::new, |note| note.body.clone()),
        linked_runtime: None,
    }
}

fn snapshot_blocker_card(task: &Task) -> TaskView {
    TaskView {
        id: task.id,
        title: task.title.clone(),
        status: task.status.as_str().to_owned(),
        updated_at: timestamp(task.updated_at),
        note_count: 0,
        commit_count: 0,
        issue_count: 0,
        latest_note: String::new(),
        linked_runtime: None,
    }
}

fn terminal_runtime_summary(
    store: &ProjectStore,
    session: &SessionInfo,
) -> TerminalRuntimeSummaryView {
    let association = session.association.as_ref().and_then(|association| {
        let pointer = association.pointer;
        (pointer.version == 1
            && association.revision != 0
            && store
                .plan(pointer.plan_id)
                .ok()
                .zip(store.task(pointer.task_id).ok())
                .is_some_and(|(_, task)| task.plan_id == pointer.plan_id))
        .then_some(RuntimeAssociation {
            plan_id: pointer.plan_id,
            task_id: pointer.task_id,
            revision: association.revision,
        })
    });
    TerminalRuntimeSummaryView {
        session_id: session.id.clone(),
        profile_kind: session.profile_kind,
        state: session.state,
        live: matches!(
            session.state,
            SessionState::Starting | SessionState::Running | SessionState::Closing
        ),
        association,
    }
}

fn task_linked_runtime(
    projection: &RuntimeProjectionView,
    task_id: u64,
) -> TaskLinkedRuntimeDetailView {
    let terminal_candidates = projection
        .terminals
        .iter()
        .filter(|terminal| {
            terminal
                .association
                .is_some_and(|association| association.task_id == task_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    let agent_candidates = projection
        .agents
        .iter()
        .filter(|agent| {
            agent
                .association
                .is_some_and(|association| association.task_id == task_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    let summary = TaskLinkedRuntimeSummaryView {
        terminals: terminal_candidates.len(),
        live_terminals: terminal_candidates
            .iter()
            .filter(|terminal| terminal.live)
            .count(),
        agents: agent_candidates.len(),
        live_agents: agent_candidates.iter().filter(|agent| agent.live).count(),
        terminal_backed_runs: agent_candidates
            .iter()
            .filter(|agent| agent.terminal_backed)
            .count(),
        external_runs: agent_candidates
            .iter()
            .filter(|agent| agent.registration_kind == RegistrationKind::External)
            .count(),
        truncated: projection.sources_truncated,
    };
    let terminal_rows_more = terminal_candidates.len().saturating_sub(64);
    let agent_rows_more = agent_candidates.len().saturating_sub(64);
    TaskLinkedRuntimeDetailView {
        summary,
        terminals: terminal_candidates.into_iter().take(64).collect(),
        agents: agent_candidates.into_iter().take(64).collect(),
        terminal_rows_more,
        agent_rows_more,
    }
}

fn apply_linked_runtime_to_board(board: &mut BoardView, projection: &RuntimeProjectionView) {
    for column in &mut board.columns {
        for task in &mut column.tasks {
            let detail = task_linked_runtime(projection, task.id);
            if detail.summary.terminals != 0
                || detail.summary.agents != 0
                || detail.summary.truncated
            {
                task.linked_runtime = Some(detail.summary);
            }
        }
    }
}

fn agent_intelligence_for_task(
    run: &AgentRuntimeSummary,
    task_id: u64,
    intelligence: ptrack_agent::AgentIntelligenceV2,
) -> Option<ptrack_agent::AgentIntelligenceV2> {
    let expected = run.association?;
    let observed = intelligence.association?;
    (run.run_id == intelligence.run_id
        && expected.task_id == task_id
        && observed.task_id == task_id
        && expected.plan_id == observed.plan_id
        && expected.revision == observed.revision)
        .then_some(intelligence)
}

pub(super) fn agent_intelligence_for_task_result(
    run: &AgentRuntimeSummary,
    task_id: u64,
    result: AppResult<ptrack_agent::AgentIntelligenceV2>,
) -> AppResult<Option<ptrack_agent::AgentIntelligenceV2>> {
    match result {
        Ok(value) => Ok(agent_intelligence_for_task(run, task_id, value)),
        Err(error) if error.to_string() == "AgentRun not found" => Ok(None),
        Err(error) => Err(error),
    }
}

fn task_detail_value(
    generation: u64,
    snapshot: &ProjectSnapshot,
    task_id: u64,
    linked_runtime: &TaskLinkedRuntimeDetailView,
    agent_intelligence: &[ptrack_agent::AgentIntelligenceV2],
) -> AppResult<Value> {
    let task = snapshot
        .task(task_id)
        .ok_or_else(|| AppError::Message(format!("task #{task_id} not found")))?;
    let mut notes = snapshot
        .notes_for_task(task_id)
        .map(|note| {
            json!({
                "id": note.id,
                "kind": note.kind.as_str(),
                "body": note.body,
                "occurredAt": timestamp(note.created_at)
            })
        })
        .collect::<Vec<_>>();
    notes.reverse();
    let mut commits = snapshot
        .commits
        .iter()
        .filter(|commit| commit.task_id == task_id)
        .map(commit_view)
        .collect::<Vec<_>>();
    commits.reverse();
    let issues = snapshot
        .issues
        .iter()
        .filter(|issue| issue.task_id == task_id)
        .map(|issue| {
            json!({
                "id": issue.id,
                "title": issue.title,
                "severity": issue.severity.as_str(),
                "taskId": issue.task_id
            })
        })
        .collect::<Vec<_>>();
    let mut task = task_card(snapshot, task);
    if linked_runtime.summary.terminals != 0
        || linked_runtime.summary.agents != 0
        || linked_runtime.summary.truncated
    {
        task.linked_runtime = Some(linked_runtime.summary);
    }
    Ok(json!({
        "generation": generation,
        "task": task,
        "linkedRuntime": linked_runtime,
        "agentIntelligence": agent_intelligence,
        "notes": notes,
        "commits": commits,
        "issues": issues
    }))
}

fn commit_view(commit: &Commit) -> Value {
    json!({
        "id": commit.id,
        "sha": commit.sha,
        "subject": commit.subject,
        "occurredAt": timestamp(commit.created_at)
    })
}

fn search(snapshot: &ProjectSnapshot, query: &str) -> Vec<Value> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut results = Vec::new();
    for plan in &snapshot.plans {
        if plan.title.to_lowercase().contains(&needle) {
            results.push(json!({ "kind": "plan", "id": plan.id, "planId": plan.id, "title": plan.title, "snippet": "" }));
        }
        if results.len() == SEARCH_RESULT_LIMIT {
            return results;
        }
    }
    for task in &snapshot.tasks {
        if task.title.to_lowercase().contains(&needle) {
            results.push(json!({ "kind": "task", "id": task.id, "planId": task.plan_id, "title": task.title, "snippet": "" }));
        }
        if results.len() == SEARCH_RESULT_LIMIT {
            return results;
        }
    }
    for note in &snapshot.notes {
        if let Some(index) = note.body.to_lowercase().find(&needle) {
            results.push(json!({
                "kind": "note",
                "id": note.id,
                "planId": if note.target == NoteTarget::Plan { note.target_id } else { 0 },
                "title": note_title(note),
                "snippet": snippet(&note.body, index, needle.len())
            }));
        }
        if results.len() == SEARCH_RESULT_LIMIT {
            break;
        }
    }
    results
}

fn note_title(note: &Note) -> String {
    let prefix = if note.kind == MemoryKind::Legacy {
        String::new()
    } else {
        let mut chars = note.kind.as_str().chars();
        chars.next().map_or_else(String::new, |first| {
            format!("{}{} · ", first.to_uppercase(), chars.as_str())
        })
    };
    format!(
        "{prefix}{} note",
        match note.target {
            NoteTarget::Project => "Project",
            NoteTarget::Plan => "Plan",
            NoteTarget::Task => "Task",
        }
    )
}

fn snippet(body: &str, index: usize, needle_len: usize) -> String {
    let start = index.saturating_sub(SEARCH_SNIPPET_SPAN / 2);
    let end = (index + needle_len + SEARCH_SNIPPET_SPAN / 2).min(body.len());
    let mut result = body.get(start..end).unwrap_or(body).to_owned();
    if start != 0 {
        result.insert(0, '…');
    }
    if end != body.len() {
        result.push('…');
    }
    result
}

fn heatmap(snapshot: &ProjectSnapshot, requested_weeks: i64) -> Vec<Value> {
    heatmap_at(
        snapshot,
        requested_weeks,
        OffsetDateTime::now_utc(),
        |timestamp| UtcOffset::local_offset_at(timestamp).unwrap_or(UtcOffset::UTC),
    )
}

pub(super) fn heatmap_at(
    snapshot: &ProjectSnapshot,
    requested_weeks: i64,
    now: OffsetDateTime,
    local_offset_at: impl Fn(OffsetDateTime) -> UtcOffset,
) -> Vec<Value> {
    let weeks = if requested_weeks <= 0 {
        16
    } else {
        requested_weeks.min(52)
    };
    let days = usize::try_from(weeks * 7).unwrap_or(112);
    let today = now.to_offset(local_offset_at(now)).date();
    let mut counts = BTreeMap::<String, usize>::new();
    for at in snapshot
        .notes
        .iter()
        .map(|note| note.created_at)
        .chain(snapshot.commits.iter().map(|commit| commit.created_at))
    {
        if let Some(timestamp) = timestamp_datetime(at) {
            let date = timestamp.to_offset(local_offset_at(timestamp)).date();
            *counts.entry(date.to_string()).or_default() += 1;
        }
    }
    (0..days)
        .map(|offset| {
            let ago = i64::try_from(days - 1 - offset).unwrap_or_default();
            let date = today - time::Duration::days(ago);
            let key = date.to_string();
            json!({ "date": key, "count": counts.get(&key).copied().unwrap_or(0) })
        })
        .collect()
}

fn note_snapshot_view(note: &Note) -> Value {
    json!({
        "id": note.id,
        "target": note.target.as_str(),
        "targetId": note.target_id,
        "kind": note.kind.as_str(),
        "body": note.body,
        "occurredAt": timestamp(note.created_at)
    })
}

fn issue_snapshot_view(issue: &Issue) -> Value {
    json!({
        "id": issue.id,
        "title": issue.title,
        "severity": issue.severity.as_str(),
        "taskId": issue.task_id
    })
}

pub(super) fn project_storage(database: &str, meta: &Meta) -> Value {
    let metadata = fs::metadata(database);
    let (status, exists, size, error) = match metadata {
        Ok(metadata) => ("ready", true, metadata.len(), None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ("error", false, 0, Some("p-track database is missing"))
        }
        Err(_) => (
            "error",
            false,
            0,
            Some("p-track database status is unavailable"),
        ),
    };
    let mut value = json!({
        "status": status,
        "exists": exists,
        "dbPath": database,
        "sizeBytes": size,
        "formatVersion": meta.format_version,
        "lastWriteVersion": meta.last_write_version
    });
    if let Some(error) = error {
        value["error"] = Value::String(error.to_owned());
    }
    value
}

fn ensure_snapshot_deadline(deadline: Instant) -> AppResult<()> {
    if Instant::now() >= deadline {
        Err(AppError::Message("context deadline exceeded".to_owned()))
    } else {
        Ok(())
    }
}

pub(super) fn confirm_linked_launch(
    ownership: AppResult<bool>,
    cleanup: impl FnOnce() -> AppResult<()>,
) -> AppResult<()> {
    let primary = match ownership {
        Ok(true) => return Ok(()),
        Ok(false) => {
            AppError::Message("linked terminal and AgentRun associations differ".to_owned())
        }
        Err(error) => error,
    };
    match cleanup() {
        Ok(()) => Err(primary),
        Err(cleanup) => Err(AppError::Message(format!("{primary}\n{cleanup}"))),
    }
}

pub(super) struct CapturedGitSnapshot {
    pub(super) wire: Value,
    pub(super) agent: ptrack_agent::CoordinationGitSnapshot,
}

fn capture_git_snapshot(root: &Path, deadline: Instant) -> CapturedGitSnapshot {
    capture_git_snapshot_with(root.to_path_buf(), deadline, |cancellation, root| {
        ptrack_git::capture(cancellation, root)
    })
}

pub(super) fn capture_git_snapshot_with<C>(
    root: PathBuf,
    deadline: Instant,
    capture: C,
) -> CapturedGitSnapshot
where
    C: FnOnce(
            &ptrack_git::CancellationToken,
            &Path,
        ) -> Result<ptrack_git::Snapshot, ptrack_git::RepositoryError>
        + Send
        + 'static,
{
    let cancellation = ptrack_git::CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let (sender, receiver) = channel();
    let _worker = thread::Builder::new()
        .name("ptrack-workspace-git-snapshot".to_owned())
        .spawn(move || {
            let _ = sender.send(capture(&worker_cancellation, &root));
        });
    let remaining = deadline.saturating_duration_since(Instant::now());
    let result = if remaining.is_zero() {
        cancellation.cancel();
        Err(ptrack_git::RepositoryError::Cancelled)
    } else {
        match receiver.recv_timeout(remaining) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                cancellation.cancel();
                Err(ptrack_git::RepositoryError::Cancelled)
            }
            Err(RecvTimeoutError::Disconnected) => Err(ptrack_git::RepositoryError::Filesystem(
                "snapshot worker failed",
            )),
        }
    };
    match result {
        Ok(snapshot) => {
            let agent =
                crate::agent_runtime::map_git_snapshot(snapshot.clone()).unwrap_or_default();
            CapturedGitSnapshot {
                wire: json!({ "state": "ready", "snapshot": snapshot }),
                agent,
            }
        }
        Err(error) => CapturedGitSnapshot {
            wire: json!({
                "state": "error",
                "error": format!("Git snapshot unavailable: {error}"),
                "snapshot": ptrack_git::Snapshot::default()
            }),
            agent: ptrack_agent::CoordinationGitSnapshot::default(),
        },
    }
}

fn empty_agent_candidates(generation: u64) -> ptrack_agent::AgentRuntimeCandidatesV2 {
    ptrack_agent::AgentRuntimeCandidatesV2 {
        generation,
        runs: Vec::new(),
        bounds: ptrack_agent::BoundedSnapshot::new(0, 0),
        sources_truncated: false,
        analysis_incomplete: false,
    }
}

fn empty_agent_activity() -> Value {
    json!({
        "state": "ready",
        "items": [],
        "counts": { "running": 0, "waiting": 0, "blocked": 0, "completed": 0, "failed": 0, "stale": 0, "unknown": 0 },
        "bounds": bound(0, 0),
        "conflicts": [],
        "conflictBounds": bound(0, 0),
        "analysisIncomplete": false,
        "notifications": [],
        "notificationBounds": bound(0, 0),
        "notificationsIncomplete": false,
        "handoffs": { "items": [], "bounds": bound(0, 0), "incomplete": false },
        "worktrees": [],
        "worktreeBounds": bound(0, 0),
        "worktreesIncomplete": false,
        "workflows": { "items": [], "bounds": bound(0, 0), "incomplete": false, "notice": ptrack_agent::AGENT_WORKFLOW_NOTICE },
        "workflowTargets": [],
        "workflowTargetsIncomplete": false
    })
}

fn bound(shown: usize, total: usize) -> Value {
    json!({ "shown": shown, "total": total, "more": total.saturating_sub(shown) })
}

fn capability_view(capability: &Capability) -> AppResult<Value> {
    match normalize(capability) {
        Ok(preview) => {
            let state = if !preview.capability.enabled {
                "disabled"
            } else if timestamp_expired(preview.capability.expires_at) {
                "expired"
            } else {
                "enabled"
            };
            capability_preview_view(&preview.capability, &preview.effective_scope, state)
        }
        Err(error) => Ok(json!({
            "capability": CapabilityWire::try_from(capability).map_err(message)?,
            "effective_scope": "",
            "state": "invalid",
            "error": error.to_string()
        })),
    }
}

fn capability_preview_view(capability: &Capability, scope: &str, state: &str) -> AppResult<Value> {
    Ok(json!({
        "capability": CapabilityWire::try_from(capability).map_err(message)?,
        "effective_scope": scope,
        "state": state
    }))
}

fn timestamp_expired(timestamp: Timestamp) -> bool {
    timestamp
        .unix_nanoseconds()
        .is_some_and(|value| value <= OffsetDateTime::now_utc().unix_timestamp_nanos())
}

fn run_capability_diagnostic(
    draft: Capability,
    ssh: Option<Capability>,
    project_root: PathBuf,
) -> AppResult<ConnectionDiagnostic> {
    std::thread::Builder::new()
        .name("ptrack-capability-diagnostic".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(AppError::Io)?;
            let cancellation = CancellationToken::new();
            let tester = ConnectionTester;
            Ok(runtime.block_on(async {
                match draft.kind {
                    CapabilityKind::Http => tester.test_http(&cancellation, &draft).await,
                    CapabilityKind::Git => {
                        tester
                            .test_git(&cancellation, &draft, ssh.as_ref(), &project_root)
                            .await
                    }
                    CapabilityKind::Ssh => tester.test_ssh(&cancellation, &draft).await,
                }
            }))
        })
        .map_err(AppError::Io)?
        .join()
        .map_err(|_| AppError::Message("capability diagnostic worker failed".to_owned()))?
}

/// Applies a preferences patch and, when the patch turns the startup opt-in
/// on while a project is open, records that project as the one to reopen.
/// Without that the setting does nothing until the user happens to reopen the
/// same project once, so the next launch lands on Welcome instead. It takes
/// the store and the open root so the transition is provable against a
/// temporary store, while the command owns resolving the process-global home.
///
/// # Errors
/// Returns an error when the patch cannot be applied.
pub(super) fn apply_preferences(
    store: &GlobalStore,
    patch: &Value,
    open_root: Option<&str>,
) -> AppResult<PreferencesDocumentV1> {
    let opted_in = preferences(store).preferences.startup.restore_last_project;
    let document = set_preferences(store, patch)?;
    if !opted_in
        && document.preferences.startup.restore_last_project
        && let Some(root) = open_root
        && let Some(recorded) = record_last_project_in(store, &json!(root))
    {
        return Ok(recorded);
    }
    Ok(document)
}

/// Records, or with a null root clears, the project startup may reopen.
/// Only the write is gated on the opt-in — a filesystem path nobody asked us
/// to keep is not ours to persist — while the clear is unconditional, so an
/// explicit close never leaves behind a root a later opt-in would silently
/// reopen. Best effort: `None` means nothing was written.
pub(super) fn record_last_project_in(
    store: &GlobalStore,
    root: &Value,
) -> Option<PreferencesDocumentV1> {
    if !root.is_null() && !preferences(store).preferences.startup.restore_last_project {
        return None;
    }
    set_preferences(store, &json!({ "startup": { "lastProjectRoot": root } })).ok()
}

/// Deletes every app-scoped record and returns the manifest of what went. It
/// takes the store so the delete set is provable against a temporary one,
/// while the command owns resolving the process-global home.
pub(super) fn reset_application_records(store: &GlobalStore) -> AppResult<[&'static str; 4]> {
    reset_preferences(store)?;
    reset_window_layout(store)?;
    store.delete_config(crate::update_preference_key())?;
    Ok([
        "preferences",
        "updates.auto-check",
        "window-state",
        "layout-state",
    ])
}

fn shutdown_terminal(terminal: Arc<TerminalRuntime>) -> AppResult<()> {
    std::thread::Builder::new()
        .name("ptrack-terminal-desktop-shutdown".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(AppError::Io)?;
            runtime.block_on(terminal.shutdown())
        })
        .map_err(AppError::Io)?
        .join()
        .map_err(|_| AppError::Message("terminal shutdown worker failed".to_owned()))?
}

fn timestamp(value: Timestamp) -> String {
    timestamp_datetime(value).map_or_else(
        || "0001-01-01T00:00:00Z".to_owned(),
        |timestamp| timestamp.format(&Rfc3339).unwrap_or_default(),
    )
}

fn parse_first_run_timestamp(value: &str) -> AppResult<Timestamp> {
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| AppError::Message("first task timestamp is invalid".to_owned()))?;
    let timestamp = Timestamp::Fixed {
        seconds: parsed.unix_timestamp(),
        nanoseconds: parsed.nanosecond(),
        offset_seconds: parsed.offset().whole_seconds(),
    };
    if self::timestamp(timestamp) != value {
        return Err(AppError::Message(
            "first task timestamp is not canonical".to_owned(),
        ));
    }
    Ok(timestamp)
}

fn first_plan_view(plan: &Plan) -> FirstPlanV1 {
    FirstPlanV1 {
        id: plan.id,
        title: plan.title.clone(),
        status: plan.status.as_str().to_owned(),
        created_at: timestamp(plan.created_at),
        updated_at: timestamp(plan.updated_at),
    }
}

fn first_task_view(task: &Task) -> FirstTaskV1 {
    FirstTaskV1 {
        id: task.id,
        plan_id: task.plan_id,
        title: task.title.clone(),
        status: task.status.as_str().to_owned(),
        created_at: timestamp(task.created_at),
        updated_at: timestamp(task.updated_at),
    }
}

fn timestamp_datetime(value: Timestamp) -> Option<OffsetDateTime> {
    let Timestamp::Fixed {
        seconds,
        nanoseconds,
        ..
    } = value
    else {
        return None;
    };
    let nanos = i32::try_from(nanoseconds).unwrap_or_default();
    let mut timestamp = OffsetDateTime::from_unix_timestamp(seconds).ok()?;
    timestamp += time::Duration::nanoseconds(i64::from(nanos));
    Some(timestamp)
}

fn validate_request(request: &DesktopCommandRequest) -> AppResult<()> {
    if COMMANDS.binary_search(&request.method.as_str()).is_err() {
        return Err(AppError::Message(
            "desktop command is not allowed".to_owned(),
        ));
    }
    let bytes = serde_json::to_vec(request)
        .map_err(|error| AppError::Message(error.to_string()))?
        .len();
    if bytes > MAX_COMMAND_BYTES {
        return Err(AppError::Message(
            "desktop command exceeds its byte limit".to_owned(),
        ));
    }
    Ok(())
}

fn state_view(state: &RuntimeState, version: &str) -> WorkspaceState {
    WorkspaceState {
        status: state.status,
        generation: state.generation,
        version: version.to_owned(),
        project: state
            .workspace
            .as_ref()
            .map(|workspace| workspace.project()),
        error: state.error.clone(),
    }
}

fn help_destination(name: &str) -> AppResult<&'static str> {
    match name {
        "help-center" => Ok("https://ro-ag.github.io/ptrack/help/"),
        "keyboard-shortcuts" => Ok("https://ro-ag.github.io/ptrack/help/reference/shortcuts/"),
        "terminals" => Ok("https://ro-ag.github.io/ptrack/help/terminals/"),
        "project-recovery" => Ok("https://ro-ag.github.io/ptrack/help/troubleshooting/"),
        "capabilities" => {
            Ok("https://ro-ag.github.io/ptrack/help/agents-and-capabilities/#capability-model")
        }
        "report-issue" => Ok("https://github.com/ro-ag/ptrack/issues/new"),
        _ => Err(AppError::Message("unknown Help destination".to_owned())),
    }
}

fn random_token() -> AppResult<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| AppError::Message(error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn project_guide_unavailable() -> ProjectGuidePreviewV1 {
    ProjectGuidePreviewV1 {
        available: false,
        message: "Project guidance is not available on this platform yet".to_owned(),
        preview_token: String::new(),
        files: Vec::new(),
    }
}

fn parse_task_status(value: &str) -> AppResult<TaskStatus> {
    TaskStatus::from_name(value)
        .ok_or_else(|| AppError::Message(format!("invalid task status {value:?}")))
}

fn association_pointer_arg(
    arguments: &[Value],
    index: usize,
) -> AppResult<TerminalAssociationPointer> {
    serde_json::from_value(
        arguments
            .get(index)
            .cloned()
            .ok_or_else(|| missing_arg(index))?,
    )
    .map_err(|error| AppError::Message(error.to_string()))
}

fn validate_association_pointer(
    snapshot: &ProjectSnapshot,
    pointer: TerminalAssociationPointer,
) -> AppResult<()> {
    if pointer.version != 1 {
        return Err(AppError::Message(
            "invalid association target: unsupported pointer version".to_owned(),
        ));
    }
    if pointer.plan_id == 0 {
        if pointer.task_id != 0 {
            return Err(AppError::Message(
                "invalid association target: task requires a plan".to_owned(),
            ));
        }
        return Ok(());
    }
    if snapshot.plan(pointer.plan_id).is_none() {
        return Err(AppError::Message(format!(
            "invalid association target: plan #{} not found",
            pointer.plan_id
        )));
    }
    if pointer.task_id != 0
        && snapshot
            .task(pointer.task_id)
            .is_none_or(|task| task.plan_id != pointer.plan_id)
    {
        return Err(AppError::Message(format!(
            "invalid association target: task #{} is not in plan #{}",
            pointer.task_id, pointer.plan_id
        )));
    }
    Ok(())
}

fn memory_kind_arg(arguments: &[Value], index: usize) -> AppResult<MemoryKind> {
    let raw = string_arg(arguments, index)?;
    let kind = MemoryKind::from_name(raw).ok_or_else(|| {
        AppError::Message("write-back content is invalid: unsupported type".to_owned())
    })?;
    if matches!(
        kind,
        MemoryKind::Decision | MemoryKind::Blocker | MemoryKind::Handoff | MemoryKind::Summary
    ) {
        Ok(kind)
    } else {
        Err(AppError::Message(
            "write-back content is invalid: unsupported type".to_owned(),
        ))
    }
}

fn validate_writeback_content(raw: &str) -> AppResult<String> {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let normalized = normalized.trim().to_owned();
    if normalized.is_empty() {
        return Err(AppError::Message(
            "write-back content is invalid: content is required".to_owned(),
        ));
    }
    if normalized.len() > 8 * 1024
        || normalized.chars().count() > 4_000
        || normalized.lines().count() > 128
    {
        return Err(AppError::Message(
            "write-back content is invalid: content exceeds the hard limit".to_owned(),
        ));
    }
    if normalized
        .chars()
        .any(|value| value.is_control() && value != '\n' && value != '\t')
    {
        return Err(AppError::Message(
            "write-back content is invalid: content contains unsupported characters".to_owned(),
        ));
    }
    if contains_potential_credential(&normalized) {
        return Err(AppError::Message(
            "write-back content may contain a credential".to_owned(),
        ));
    }
    Ok(normalized)
}

fn writeback_target_label(pointer: TerminalAssociationPointer) -> String {
    if pointer.plan_id == 0 {
        "Project".to_owned()
    } else if pointer.task_id == 0 {
        format!("Plan #{}", pointer.plan_id)
    } else {
        format!("Task #{}", pointer.task_id)
    }
}

fn writeback_destination(
    snapshot: &ProjectSnapshot,
    pointer: TerminalAssociationPointer,
    kind: MemoryKind,
) -> AppResult<String> {
    validate_association_pointer(snapshot, pointer)?;
    Ok(if kind == MemoryKind::Summary {
        "Project rolling summary".to_owned()
    } else {
        writeback_target_label(pointer)
    })
}

fn writeback_target(
    pointer: TerminalAssociationPointer,
    kind: MemoryKind,
) -> (NoteTarget, u64, u64) {
    if kind == MemoryKind::Summary || pointer.plan_id == 0 {
        (NoteTarget::Project, 0, 0)
    } else if pointer.task_id == 0 {
        (NoteTarget::Plan, pointer.plan_id, pointer.plan_id)
    } else {
        (NoteTarget::Task, pointer.task_id, pointer.plan_id)
    }
}

fn parse_workflow_kind(value: &str) -> AppResult<AgentWorkflowKind> {
    match value {
        "validation" => Ok(AgentWorkflowKind::Validation),
        "commit" => Ok(AgentWorkflowKind::Commit),
        "pullRequest" => Ok(AgentWorkflowKind::PullRequest),
        "merge" => Ok(AgentWorkflowKind::Merge),
        _ => Err(AppError::Message(format!(
            "unsupported workflow kind {value:?}"
        ))),
    }
}

fn capability_draft_arg(arguments: &[Value], index: usize) -> AppResult<Capability> {
    let wire: CapabilityDraftWire = serde_json::from_value(
        arguments
            .get(index)
            .cloned()
            .ok_or_else(|| missing_arg(index))?,
    )
    .map_err(|error| AppError::Message(error.to_string()))?;
    wire.try_into().map_err(message)
}

fn value<T: Serialize>(value: T) -> AppResult<Value> {
    serde_json::to_value(value).map_err(|error| AppError::Message(error.to_string()))
}

fn path_arg(arguments: &[Value], index: usize) -> AppResult<PathBuf> {
    Ok(PathBuf::from(string_arg(arguments, index)?))
}

fn typed_arg<T: for<'de> Deserialize<'de>>(arguments: &[Value], index: usize) -> AppResult<T> {
    serde_json::from_value(
        arguments
            .get(index)
            .cloned()
            .ok_or_else(|| missing_arg(index))?,
    )
    .map_err(|_| missing_arg(index))
}

fn string_arg(arguments: &[Value], index: usize) -> AppResult<&str> {
    arguments
        .get(index)
        .and_then(Value::as_str)
        .ok_or_else(|| missing_arg(index))
}

fn string_vec_arg(arguments: &[Value], index: usize) -> AppResult<Vec<String>> {
    let values = arguments
        .get(index)
        .and_then(Value::as_array)
        .ok_or_else(|| missing_arg(index))?;
    values
        .iter()
        .enumerate()
        .map(|(offset, value)| {
            value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                AppError::Message(format!(
                    "desktop command argument {index}[{offset}] is invalid"
                ))
            })
        })
        .collect()
}

fn u64_arg(arguments: &[Value], index: usize) -> AppResult<u64> {
    arguments
        .get(index)
        .and_then(Value::as_u64)
        .ok_or_else(|| missing_arg(index))
}

fn i64_arg(arguments: &[Value], index: usize) -> AppResult<i64> {
    arguments
        .get(index)
        .and_then(Value::as_i64)
        .ok_or_else(|| missing_arg(index))
}

fn u16_arg(arguments: &[Value], index: usize) -> AppResult<u16> {
    u16::try_from(u64_arg(arguments, index)?)
        .map_err(|_| AppError::Message(format!("desktop command argument {index} is invalid")))
}

fn bool_arg(arguments: &[Value], index: usize) -> AppResult<bool> {
    arguments
        .get(index)
        .and_then(Value::as_bool)
        .ok_or_else(|| missing_arg(index))
}

fn missing_arg(index: usize) -> AppError {
    AppError::Message(format!("desktop command argument {index} is invalid"))
}

/// A terminal-window tab from two adjacent arguments: the session list and
/// the shape, which must be a JSON object — the frontend re-hydrates a split
/// tree from it, and anything else could only ever render nothing.
fn tab_args(method: &str, arguments: &[Value], index: usize) -> AppResult<TerminalWindowTab> {
    let sessions = string_vec_arg(arguments, index)?;
    let shape = arguments
        .get(index + 1)
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| {
            AppError::Message(format!("{method} requires an object tab shape"))
        })?;
    Ok(TerminalWindowTab { sessions, shape })
}

fn trimmed_nonempty(value: &str, error: &str) -> AppResult<String> {
    let value = value.trim();
    if value.is_empty() {
        Err(AppError::Message(error.to_owned()))
    } else {
        Ok(value.to_owned())
    }
}

fn first_run_title(value: &str, kind: &str) -> AppResult<String> {
    let title = value.trim();
    if title.is_empty() || title.len() > FIRST_RUN_TITLE_MAX_BYTES {
        return Err(AppError::Message(format!(
            "{kind} title must contain 1 to {FIRST_RUN_TITLE_MAX_BYTES} UTF-8 bytes"
        )));
    }
    Ok(title.to_owned())
}

fn require_argument_count(method: &str, arguments: &[Value], expected: usize) -> AppResult<()> {
    if arguments.len() == expected {
        Ok(())
    } else {
        Err(AppError::Message(format!(
            "{method} requires exactly {expected} arguments"
        )))
    }
}

fn recent_identifier_arg(arguments: &[Value], index: usize) -> AppResult<&str> {
    let value = string_arg(arguments, index)?;
    if value.len() == RECENT_PROJECT_TOKEN_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(value)
    } else {
        Err(missing_arg(index))
    }
}

fn recent_optional_token_arg(arguments: &[Value], index: usize) -> AppResult<&str> {
    let value = string_arg(arguments, index)?;
    if value.is_empty() {
        Ok(value)
    } else {
        recent_identifier_arg(arguments, index)
    }
}

fn recent_path_arg(arguments: &[Value], index: usize) -> AppResult<PathBuf> {
    let value = string_arg(arguments, index)?;
    if value.is_empty() || value.len() > RECENT_PROJECT_PATH_LIMIT {
        return Err(missing_arg(index));
    }
    Ok(PathBuf::from(value))
}

fn sanitize_recent_open_error(error: AppError) -> AppError {
    match error {
        AppError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            AppError::Message("recent-project-folder-not-found".to_owned())
        }
        AppError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            AppError::Message("recent-project-permission-required".to_owned())
        }
        AppError::Message(message) if message == "invalid or expired workspace confirmation" => {
            AppError::Message(message)
        }
        AppError::NoProject
        | AppError::NotImplemented(_)
        | AppError::Io(_)
        | AppError::Message(_) => AppError::Message("recent-project-open-failed".to_owned()),
    }
}

/// Whether a workspace method is answered by the capability broker.
///
/// The stem is `Capabilit`, not `Capability`: `GetCapabilitiesV2` is the one
/// method in the allowlist that pluralizes, and the longer stem skipped it into
/// the unavailable arm, leaving its handler unreachable.
pub(crate) fn routes_to_capability(method: &str) -> bool {
    method.contains("Capabilit")
}

fn unavailable(feature: &str) -> AppError {
    AppError::Message(format!("{feature} is unavailable"))
}

fn message(error: impl std::fmt::Display) -> AppError {
    AppError::Message(error.to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
