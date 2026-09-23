//! The desktop bridge runtime.
//!
//! - `wire` holds the request envelope, the exact command allowlist, and
//!   every serialized response and event.
//! - `command` parses one allowlisted request, once, into a typed command;
//!   `args` is the positional reader it is built on.
//! - `ports` are the services the coordinator is built from.
//! - `coordinator` is [`DesktopRuntime`]: the workspace lifecycle and every
//!   application-scoped command.
//! - `workspace` is [`BoundDesktopWorkspace`], the production project
//!   workspace, with one handler per command group.
//! - `projection`, `search`, and `stack` build the read views.

use std::time::Duration;

mod admission;
mod args;
mod command;
mod coordinator;
mod ports;
mod projection;
mod search;
mod stack;
mod support;
mod wire;
mod workspace;

#[cfg(test)]
mod command_test;

pub use admission::DesktopAdmissionFence;
pub use coordinator::{DesktopNativeActionLease, DesktopRuntime};
pub use ports::{
    DesktopEventSink, DesktopInitializationService, DesktopRuntimeConfig, DesktopTerminalEventSink,
    DesktopUpdateEventSink, DesktopWorkspace, DesktopWorkspaceFactory,
    NoDesktopInitializationService, NoDesktopWorkspaceFactory, NoRecentProjectsProvider,
    RecentProjectsProvider,
};
pub use wire::{
    ActiveResourceSummary, CreateFirstPlanResultV1, CreateFirstTaskResultV1, DesktopCommandRequest,
    DesktopEvent, DesktopNotificationEventV1, DesktopNotificationKindV1,
    DesktopNotificationSnapshotV1, FirstPlanV1, FirstRunWorkspaceStateV1, FirstTaskV1,
    ForgetRecentProjectResultV1, InitializationCheckpointV1, InitializationOutcomeV1,
    InitializationStatusV1, InitializeProjectRequestV1, InitializeProjectResultV1,
    OpenRecentProjectResultV1, PendingInitializationV1, ProjectGuideChoiceV1,
    ProjectGuideFileActionV1, ProjectGuideFilePreviewV1, ProjectGuidePreviewRequestV1,
    ProjectGuidePreviewV1, ProjectTargetKindV1, ProjectTargetValidationV1,
    RecentProjectAvailabilityV1, RecentProjectLanguageV1, RecentProjectOpenAuthorizationV1,
    RecentProjectRegistryCommitV1, RecentProjectRegistryStatusV1, RecentProjectResolutionV1,
    RecentProjectStackV1, RecentProjectV1, RecentProjectsV1, ResetApplicationStateResultV1,
    ResolvedRecentProjectV1, ShutdownOutcome, WorkspaceChangeResult, WorkspaceProject,
    WorkspaceState, WorkspaceStatus, allowed_desktop_commands, allowed_terminal_window_commands,
    scope_request_to_window,
};
pub use workspace::{BoundDesktopWorkspace, DesktopAgentRuntime};

#[cfg(test)]
pub(crate) use coordinator::{
    apply_preferences, record_last_project_in, reset_application_records, watch_workspace_data,
};
#[cfg(test)]
pub(crate) use projection::{
    agent_intelligence_for_task_result, board_view, capture_git_snapshot_with, project_storage,
    snapshot_board_view,
};
#[cfg(test)]
pub(crate) use search::{find_case_insensitive, heatmap_at, search};
#[cfg(test)]
pub(crate) use stack::{StackScanOutcome, stack_scan_due, stack_scan_outcome};
#[cfg(test)]
pub(crate) use workspace::confirm_linked_launch;

const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const DEFAULT_CONFIRMATION_TTL: Duration = Duration::from_secs(60);
const RUNTIME_CALL_TIMEOUT: Duration = Duration::from_millis(250);
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
const SHUTDOWN_RETRY_INTERVAL: Duration = Duration::from_millis(50);

pub const FIRST_RUN_GOAL_MAX_BYTES: usize = 4_096;
