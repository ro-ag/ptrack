//! Wire shapes of the desktop bridge: the request envelope, the exact command
//! allowlist and its per-window scoping, and every serialized response and
//! event.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::MAX_COMMAND_BYTES;
use crate::{AppError, AppResult};

const COMMANDS: [&str; 88] = [
    "AcknowledgeAgentHandoffV2",
    "AddIssueV1",
    "AddPlanV1",
    "AddTaskNoteV2",
    "AddTaskV2",
    "ApplyUpdate",
    "ApproveAgentWorkflowV2",
    "CancelUpdateOperation",
    "CancelWorkspaceChange",
    "CheckForUpdates",
    "ClaimTerminalStream",
    "CloseProject",
    "CloseTerminalV2",
    "CompletePlanV1",
    "CopyPlanV1",
    "CreateFirstPlanV1",
    "CreateFirstTaskV1",
    "CreateTerminalV2",
    "DeletePlanV1",
    "DismissAgentWorkflowV2",
    "DownloadUpdate",
    "ForgetRecentProjectV1",
    "GetActivityHeatmapV2",
    "GetDiagnosticsReport",
    "GetGlobalOverviewV1",
    "GetInitializationStatusV1",
    "GetIssueDetailV1",
    "GetIssuesV1",
    "GetLayoutState",
    "GetPendingInitializationV1",
    "GetPreferences",
    "GetProjectTimelineV1",
    "GetRecentProjectsV1",
    "GetScratchpadV1",
    "GetStackProfileV1",
    "GetTaskDetailV2",
    "GetTerminalProfiles",
    "GetTerminalProfilesV2",
    "GetTerminalWindowTab",
    "GetUpdateState",
    "GetWorkspaceSnapshot",
    "GetWorkspaceState",
    "HoldPlanV1",
    "InitializeProjectV1",
    "InstallShellCommand",
    "LaunchLinkedAgentV2",
    "ListProjectsV1",
    "MoveIssueTaskV1",
    "MovePlanV1",
    "MoveTaskV3",
    "MutateTerminalAssociationV2",
    "OpenHelpDestination",
    "OpenProject",
    "OpenRecentProjectV1",
    "OpenTerminalWindow",
    "PickProjectDirectory",
    "PrepareAgentWorkflowV2",
    "PreviewAgentHandoffV2",
    "PreviewProjectGuideV1",
    "PreviewTerminalWritebackV2",
    "RefreshGlobalOverviewV1",
    "RenamePlanV1",
    "RenameTaskV2",
    "ReopenPlanV1",
    "ResetApplicationState",
    "ResetPreferences",
    "ResetWindowLayout",
    "ResizeTerminalV2",
    "ResolveRecentProjectV1",
    "ResumePlanV1",
    "RollbackLinkedAgentLaunchV2",
    "ScheduleIssueV1",
    "SearchV2",
    "SendAgentHandoffV2",
    "SetActivePlanV1",
    "SetAgentTaskOwnershipV2",
    "SetAgentWorktreeV2",
    "SetAutomaticUpdateChecks",
    "SetIssueTaskV1",
    "SetLayoutState",
    "SetPreferences",
    "SetScratchpadV1",
    "SetTerminalWindowTab",
    "StartFirstTaskV1",
    "UpdateIssueV1",
    "ValidateProjectTargetV1",
    "ValidateTerminalCWDsV2",
    "WriteTerminalMemoryV2",
];

/// The commands a popped-out terminal window actually sends, sorted. A
/// terminal window renders one tab of sessions and nothing else, so every
/// project, plan, task, and update mutation stays reachable from the main
/// window only.
const TERMINAL_WINDOW_COMMANDS: [&str; 12] = [
    "ClaimTerminalStream",
    "CloseTerminalV2",
    "CreateTerminalV2",
    "GetPreferences",
    // The window's scratchpad panel reads and writes the project's one
    // scratchpad, generation- and revision-fenced exactly as the dock's is.
    "GetScratchpadV1",
    "GetTerminalProfiles",
    "GetTerminalWindowTab",
    "GetWorkspaceState",
    "ResizeTerminalV2",
    // The terminal window's own theme toggle writes the shared preference.
    "SetPreferences",
    "SetScratchpadV1",
    "SetTerminalWindowTab",
];

/// The commands that address the calling terminal window's own assignment.
const TERMINAL_WINDOW_SELF_COMMANDS: [&str; 2] = ["GetTerminalWindowTab", "SetTerminalWindowTab"];

/// How one bounded runtime teardown ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShutdownOutcome {
    /// The runtime shut down; these terminal windows go with it.
    Completed(Vec<String>),
    /// The runtime refused to close and stays fully usable, updates included.
    Refused(String),
    /// The bound elapsed while the teardown was still running.
    TimedOut,
}

/// Exact desktop bridge command allowlist.
#[must_use]
pub const fn allowed_desktop_commands() -> &'static [&'static str] {
    &COMMANDS
}

/// Exact subset of [`allowed_desktop_commands`] a terminal window may send.
#[must_use]
pub const fn allowed_terminal_window_commands() -> &'static [&'static str] {
    &TERMINAL_WINDOW_COMMANDS
}

/// Scopes one bridge request to the window that sent it.
///
/// The main window reaches every allowlisted command except the two that
/// address a terminal window's own assignment. A terminal window reaches only
/// the commands it uses, and those two always address the caller: the label
/// in the payload is replaced by the caller's, so one terminal window can never
/// read or rewrite another's tab. Any other label is refused.
///
/// # Errors
/// Returns an error when the calling window may not send the method.
pub fn scope_request_to_window(
    window_label: &str,
    mut request: DesktopCommandRequest,
) -> AppResult<DesktopCommandRequest> {
    let method = request.method.as_str();
    let self_addressed = TERMINAL_WINDOW_SELF_COMMANDS.contains(&method);
    if window_label == crate::window_state::MAIN_WINDOW_LABEL {
        if self_addressed {
            return Err(window_refusal(method));
        }
        return Ok(request);
    }
    let terminal_window = window_label
        .strip_prefix(crate::terminal_windows::TERMINAL_WINDOW_PREFIX)
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        });
    if !terminal_window || TERMINAL_WINDOW_COMMANDS.binary_search(&method).is_err() {
        return Err(window_refusal(method));
    }
    if self_addressed && let Some(label) = request.arguments.first_mut() {
        *label = Value::String(window_label.to_owned());
    }
    Ok(request)
}

fn window_refusal(method: &str) -> AppError {
    AppError::Message(format!("{method} is not available to this window"))
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

/// Native-shell notification categories. They intentionally exclude provider
/// text, paths, handoff previews, and terminal output.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DesktopNotificationKindV1 {
    HandoffArrival,
    RunFailure,
    RunDrift,
    RunCompletion,
}

/// One stable, identifier-only event for native notification policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopNotificationEventV1 {
    pub id: String,
    pub kind: DesktopNotificationKindV1,
    pub run_id: String,
    pub plan_id: u64,
    pub task_id: u64,
}

/// Bounded current notification state for one project generation.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopNotificationSnapshotV1 {
    pub generation: u64,
    pub events: Vec<DesktopNotificationEventV1>,
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
    pub(super) const fn requires_confirmation(self) -> bool {
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
    /// Stack label for the card, absent until this project has been scanned by
    /// a build carrying stack discovery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<RecentProjectStackV1>,
}

/// The card's stack label: languages with tracked-file counts, never a size.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProjectStackV1 {
    pub languages: Vec<RecentProjectLanguageV1>,
    pub tracked_files: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProjectLanguageV1 {
    pub language: String,
    pub files: u32,
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
    #[serde(rename = "scratchpad:changed")]
    ScratchpadChanged(ScratchpadChangedV1),
}

/// A scratchpad write that reached the store. Every window showing the
/// project's scratchpad (the dock and any terminal window) re-reads it unless
/// it holds an unsaved edit of its own, which then meets the revision check.
/// Content-free: the record itself is fetched through the fenced read.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScratchpadChangedV1 {
    pub generation: u64,
    pub revision: u64,
}

impl ScratchpadChangedV1 {
    /// The change a successful `SetScratchpadV1` reply describes, or `None`
    /// for any other reply shape.
    #[must_use]
    pub fn from_reply(reply: &Value) -> Option<Self> {
        Some(Self {
            generation: reply.get("generation")?.as_u64()?,
            revision: reply.get("revision")?.as_u64()?,
        })
    }
}

pub(super) fn validate_request(request: &DesktopCommandRequest) -> AppResult<()> {
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
