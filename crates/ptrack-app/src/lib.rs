//! UI-neutral p-track application services.
//!
//! The service owns explicit roots, activation bindings, and writer identity.
//! Every operation opens its required store, verifies the binding, performs
//! one use case, and drops the handle before returning. No caller can retain a
//! redb handle or accidentally hold the writer lock while idle.

mod agent_runtime;
mod desktop_runtime;
mod diagnostics_report;
mod identity;
mod layout_state;
mod preferences;
mod production;
mod service;
mod shell_command;
mod terminal_runtime;
mod terminal_windows;
mod update_runtime;
pub mod window_state;

pub use agent_runtime::{
    AgentAdmissionFence, AgentIntegration, AgentIntegrationFactory, AgentInvalidationV2,
    AgentMutationOutcome, AgentNotificationsV2, AgentResourceStateV2, AgentRuntime,
    AgentRuntimeConfig, AgentRuntimeEventSuppression, AgentRuntimeService, AgentWorkflowTargetsV2,
    LaunchedEventAuthority, LinkedAgentAssociationChange, LinkedAgentRuntimeHooks,
    ProductionAgentIntegrationFactory, ProjectCoordinationStore, PtrackCoordinationGit,
};
pub use desktop_runtime::{
    ActiveResourceSummary, BoundDesktopWorkspace, CreateFirstPlanResultV1, CreateFirstTaskResultV1,
    DesktopAdmissionFence, DesktopAgentRuntime, DesktopCommandRequest, DesktopEvent,
    DesktopEventSink, DesktopInitializationService, DesktopNativeActionLease, DesktopRuntime,
    DesktopRuntimeConfig, DesktopTerminalEventSink, DesktopUpdateEventSink, DesktopWorkspace,
    DesktopWorkspaceFactory, FirstPlanV1, FirstRunWorkspaceStateV1, FirstTaskV1,
    ForgetRecentProjectResultV1, InitializationCheckpointV1, InitializationOutcomeV1,
    InitializationStatusV1, InitializeProjectRequestV1, InitializeProjectResultV1,
    NoDesktopInitializationService, NoDesktopWorkspaceFactory, NoRecentProjectsProvider,
    OpenRecentProjectResultV1, PendingInitializationV1, ProjectGuideChoiceV1,
    ProjectGuideFileActionV1, ProjectGuideFilePreviewV1, ProjectGuidePreviewRequestV1,
    ProjectGuidePreviewV1, ProjectTargetKindV1, ProjectTargetValidationV1,
    RecentProjectAvailabilityV1, RecentProjectOpenAuthorizationV1, RecentProjectRegistryCommitV1,
    RecentProjectRegistryStatusV1, RecentProjectResolutionV1, RecentProjectV1,
    RecentProjectsProvider, RecentProjectsV1, ResetApplicationStateResultV1,
    ResolvedRecentProjectV1, WorkspaceChangeResult, WorkspaceProject, WorkspaceState,
    WorkspaceStatus, allowed_desktop_commands,
};

pub use identity::{IDENTITY_CONFIG_KEY, load_identity, set_identity_name};
pub use production::{
    ActiveRuntime, ProductionDesktopAuthority, ProductionDesktopWorkspaceFactory,
    ProductionRecentProjects, RoutedApplication, RuntimeBindingState, StartupProjectV1,
    production_desktop_runtime, resolve_global_home, resolved_startup_project, startup_project,
};
pub use ptrack_store::ActorIdentity;
/// Re-exported so a presentation layer can strip the store's hold-refusal
/// prefix off an [`AppError`] without depending on `ptrack-store` itself.
pub use ptrack_store::{INVALID_CLAIM_PREFIX, INVALID_HOLD_PREFIX};
pub use service::{
    AppError, AppResult, ApplicationPort, CapabilityCancellation, CapabilityMcpOutcome,
    CapabilitySessionEnvironment, GuideAction, HookAction, HookResult, InitRequest, InitResult,
    LocalApplication, Mutation, MutationResult, ProcessOutput, ProjectEndpoint,
    UnavailableApplication, WorkspaceBindings,
};
pub use shell_command::{ShellCommandInstallResult, install_shell_command};
pub use terminal_runtime::{
    PreparedTerminalIdentity, ProductionTerminalIdentityAuthority, TerminalAgentAuthority,
    TerminalCwdValidation, TerminalCwdValidationsV2, TerminalEventSink, TerminalExitV2,
    TerminalIdentityAuthority, TerminalProfileView, TerminalProfilesV2, TerminalRuntime,
    TerminalRuntimeConfig, TerminalSessionV2, TerminalStatusV2,
};
pub use terminal_windows::{TERMINAL_WINDOW_PREFIX, TerminalWindows};
pub use update_runtime::{
    DesktopUpdateService, GlobalStoreUpdatePreferences, NoUpdatePreferences,
    UnavailableUpdateService, UpdateEventSink, UpdatePhase, UpdatePreferences, UpdateRelease,
    UpdateRuntime, UpdateState, update_preference_key,
};

#[cfg(test)]
mod agent_runtime_test;
#[cfg(test)]
mod desktop_runtime_test;
#[cfg(test)]
mod diagnostics_report_test;
#[cfg(test)]
mod identity_test;
#[cfg(test)]
mod layout_state_test;
#[cfg(test)]
mod preferences_test;
#[cfg(test)]
mod production_test;
#[cfg(test)]
mod service_test;
#[cfg(test)]
mod shell_command_test;
#[cfg(test)]
mod terminal_runtime_test;
#[cfg(test)]
mod terminal_windows_test;
#[cfg(test)]
mod update_runtime_test;
#[cfg(test)]
mod window_state_test;
