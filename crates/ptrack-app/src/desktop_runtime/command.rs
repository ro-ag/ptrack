//! Typed desktop commands.
//!
//! Every allowlisted bridge method is parsed here, once, from its method name
//! and positional JSON arguments. The parse owns every arity and JSON-type
//! check and keeps the exact error text the bridge has always returned, so the
//! handlers receive named, typed fields and never index an argument list.
//! Checks that depend on live state or on what a string means — the
//! generation fence, title rules, status names — stay with the handlers.
//!
//! The coordinator answers [`DesktopCommand`]s itself and forwards
//! [`DesktopCommand::Workspace`] untouched: the open workspace parses its own
//! [`WorkspaceCommand`], so a request that reaches no workspace still reports
//! that no workspace is open rather than a complaint about its arguments.

use std::path::PathBuf;

use ptrack_terminal::TerminalAssociationPointer;
use serde_json::Value;

use super::args::Args;
use super::support::{message, unavailable};
use super::wire::{InitializeProjectRequestV1, ProjectGuidePreviewRequestV1};
use crate::terminal_windows::TerminalWindowTab;
use crate::{AppResult, ScratchpadV1};

/// One allowlisted request, as the coordinator routes it.
#[derive(Debug)]
pub(super) enum DesktopCommand<'a> {
    /// Application-scoped settings, answered with no project open.
    Settings(SettingsCommand<'a>),
    /// A popped-out terminal window's assignment.
    TerminalWindow(TerminalWindowCommand<'a>),
    /// First-run project initialization.
    Initialization(InitializationCommand<'a>),
    /// Opening, closing, and reading the workspace itself.
    Lifecycle(LifecycleCommand<'a>),
    /// The updater.
    Update(UpdateCommand<'a>),
    /// Native shell integrations.
    Native(NativeCommand<'a>),
    /// The recent-projects registry and global overview.
    Recent(RecentCommand<'a>),
    /// Served by the open project workspace; see [`WorkspaceCommand`].
    Workspace,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum SettingsCommand<'a> {
    GetPreferences,
    SetPreferences { patch: &'a Value },
    ResetPreferences,
    GetDiagnosticsReport,
    GetLayoutState,
    SetLayoutState { state: &'a Value },
    ResetWindowLayout,
    ResetApplicationState,
}

#[derive(Debug)]
pub(super) enum TerminalWindowCommand<'a> {
    Open {
        tab: TerminalWindowTab,
    },
    GetTab {
        label: &'a str,
    },
    SetTab {
        label: &'a str,
        tab: TerminalWindowTab,
    },
}

#[derive(Debug)]
pub(super) enum InitializationCommand<'a> {
    Status {
        operation_id: &'a str,
    },
    Pending,
    Initialize {
        request: InitializeProjectRequestV1,
    },
    PreviewGuide {
        request: ProjectGuidePreviewRequestV1,
    },
    ValidateTarget {
        selected: PathBuf,
    },
}

#[derive(Debug)]
pub(super) enum LifecycleCommand<'a> {
    GetState,
    Open { root: PathBuf, token: &'a str },
    Close { token: &'a str },
    CancelChange { token: &'a str },
}

#[derive(Clone, Copy, Debug)]
pub(super) enum UpdateCommand<'a> {
    GetState,
    CancelOperation,
    SetAutomaticChecks { enabled: bool },
    Check,
    Download { expected_version: &'a str },
    Apply { expected_version: &'a str },
}

#[derive(Clone, Copy, Debug)]
pub(super) enum NativeCommand<'a> {
    OpenHelpDestination { destination: &'a str },
    PickProjectDirectory,
    InstallShellCommand,
}

#[derive(Debug)]
pub(super) enum RecentCommand<'a> {
    GlobalOverview,
    RefreshGlobalOverview,
    List,
    Resolve {
        entry_id: &'a str,
        base: &'a str,
        candidate: PathBuf,
    },
    Forget {
        entry_id: &'a str,
        base: &'a str,
    },
    Open {
        workspace_token: &'a str,
        /// Kept unparsed until the handler runs: a malformed target must still
        /// cancel the workspace confirmation `workspace_token` names.
        target: AppResult<RecentOpenTarget<'a>>,
    },
}

#[derive(Debug)]
pub(super) struct RecentOpenTarget<'a> {
    pub(super) entry_id: &'a str,
    pub(super) base: &'a str,
    pub(super) canonical_root: PathBuf,
    pub(super) relocation_token: &'a str,
}

impl<'a> DesktopCommand<'a> {
    /// Parses one allowlisted method. Every method this does not name belongs
    /// to the workspace.
    ///
    /// # Errors
    /// Returns the bridge's arity or argument error for a malformed request.
    pub(super) fn parse(method: &'a str, arguments: &'a [Value]) -> AppResult<Self> {
        let args = Args::new(method, arguments);
        if let Some(command) = SettingsCommand::parse(method, args)? {
            return Ok(Self::Settings(command));
        }
        if let Some(command) = TerminalWindowCommand::parse(method, args)? {
            return Ok(Self::TerminalWindow(command));
        }
        if let Some(command) = InitializationCommand::parse(method, args)? {
            return Ok(Self::Initialization(command));
        }
        if let Some(command) = LifecycleCommand::parse(method, args)? {
            return Ok(Self::Lifecycle(command));
        }
        if let Some(command) = UpdateCommand::parse(method, args)? {
            return Ok(Self::Update(command));
        }
        if let Some(command) = NativeCommand::parse(method, args)? {
            return Ok(Self::Native(command));
        }
        if let Some(command) = RecentCommand::parse(method, args)? {
            return Ok(Self::Recent(command));
        }
        Ok(Self::Workspace)
    }
}

impl<'a> SettingsCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        let command = match method {
            "GetPreferences" => Self::GetPreferences,
            "SetPreferences" => {
                args.exact(1)?;
                return Ok(Some(Self::SetPreferences {
                    patch: args.value(0)?,
                }));
            }
            "ResetPreferences" => Self::ResetPreferences,
            "GetDiagnosticsReport" => Self::GetDiagnosticsReport,
            "GetLayoutState" => Self::GetLayoutState,
            "SetLayoutState" => {
                args.exact(1)?;
                return Ok(Some(Self::SetLayoutState {
                    state: args.value(0)?,
                }));
            }
            "ResetWindowLayout" => Self::ResetWindowLayout,
            "ResetApplicationState" => Self::ResetApplicationState,
            _ => return Ok(None),
        };
        args.exact(0)?;
        Ok(Some(command))
    }
}

impl<'a> TerminalWindowCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        Ok(Some(match method {
            "OpenTerminalWindow" => {
                args.exact(2)?;
                Self::Open { tab: args.tab(0)? }
            }
            "GetTerminalWindowTab" => {
                args.exact(1)?;
                Self::GetTab {
                    label: args.string(0)?,
                }
            }
            "SetTerminalWindowTab" => {
                args.exact(3)?;
                Self::SetTab {
                    label: args.string(0)?,
                    tab: args.tab(1)?,
                }
            }
            _ => return Ok(None),
        }))
    }
}

impl<'a> InitializationCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        Ok(Some(match method {
            "GetInitializationStatusV1" => Self::Status {
                operation_id: args.string(0)?,
            },
            "GetPendingInitializationV1" => {
                args.exact(0)?;
                Self::Pending
            }
            "InitializeProjectV1" => Self::Initialize {
                request: args.typed(0)?,
            },
            "PreviewProjectGuideV1" => Self::PreviewGuide {
                request: args.typed(0)?,
            },
            "ValidateProjectTargetV1" => Self::ValidateTarget {
                selected: args.path(0)?,
            },
            _ => return Ok(None),
        }))
    }
}

impl<'a> LifecycleCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        Ok(Some(match method {
            "GetWorkspaceState" => Self::GetState,
            "OpenProject" => Self::Open {
                root: args.path(0)?,
                token: args.string(1)?,
            },
            "CloseProject" => Self::Close {
                token: args.string(0)?,
            },
            "CancelWorkspaceChange" => Self::CancelChange {
                token: args.string(0)?,
            },
            _ => return Ok(None),
        }))
    }
}

impl<'a> UpdateCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        Ok(Some(match method {
            "GetUpdateState" => Self::GetState,
            "CancelUpdateOperation" => Self::CancelOperation,
            "SetAutomaticUpdateChecks" => Self::SetAutomaticChecks {
                enabled: args.bool(0)?,
            },
            "CheckForUpdates" => Self::Check,
            "DownloadUpdate" => Self::Download {
                expected_version: args.string(0)?,
            },
            "ApplyUpdate" => Self::Apply {
                expected_version: args.string(0)?,
            },
            _ => return Ok(None),
        }))
    }
}

impl<'a> NativeCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        Ok(Some(match method {
            "OpenHelpDestination" => Self::OpenHelpDestination {
                destination: args.string(0)?,
            },
            "PickProjectDirectory" => Self::PickProjectDirectory,
            "InstallShellCommand" => Self::InstallShellCommand,
            _ => return Ok(None),
        }))
    }
}

impl<'a> RecentCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        Ok(Some(match method {
            "GetGlobalOverviewV1" => {
                args.exact(0)?;
                Self::GlobalOverview
            }
            "RefreshGlobalOverviewV1" => {
                args.exact(0)?;
                Self::RefreshGlobalOverview
            }
            "GetRecentProjectsV1" => {
                args.exact(0)?;
                Self::List
            }
            "ResolveRecentProjectV1" => {
                args.exact(3)?;
                Self::Resolve {
                    entry_id: args.recent_identifier(0)?,
                    base: args.recent_identifier(1)?,
                    candidate: args.recent_path(2)?,
                }
            }
            "ForgetRecentProjectV1" => {
                args.exact(2)?;
                Self::Forget {
                    entry_id: args.recent_identifier(0)?,
                    base: args.recent_identifier(1)?,
                }
            }
            "OpenRecentProjectV1" => {
                args.exact(5)?;
                Self::Open {
                    workspace_token: args.recent_optional_token(4)?,
                    target: RecentOpenTarget::parse(args),
                }
            }
            _ => return Ok(None),
        }))
    }
}

impl<'a> RecentOpenTarget<'a> {
    fn parse(args: Args<'a>) -> AppResult<Self> {
        Ok(Self {
            entry_id: args.recent_identifier(0)?,
            base: args.recent_identifier(1)?,
            canonical_root: args.recent_path(2)?,
            relocation_token: args.recent_optional_token(3)?,
        })
    }
}

/// One command the open project workspace serves.
#[derive(Debug)]
pub(super) enum WorkspaceCommand<'a> {
    Plan(PlanCommand<'a>),
    Task(TaskCommand<'a>),
    Issue(IssueCommand<'a>),
    Project(ProjectCommand<'a>),
    Scratchpad(ScratchpadCommand),
    Terminal(TerminalCommand<'a>),
    Agent(AgentCommand<'a>),
}

#[derive(Clone, Copy, Debug)]
pub(super) enum PlanCommand<'a> {
    Add {
        generation: u64,
        title: &'a str,
    },
    CreateFirst {
        generation: u64,
        title: &'a str,
    },
    Rename {
        generation: u64,
        plan_id: u64,
        title: &'a str,
    },
    Complete {
        generation: u64,
        plan_id: u64,
    },
    Hold {
        generation: u64,
        plan_id: u64,
        reason: &'a str,
    },
    Resume {
        generation: u64,
        plan_id: u64,
    },
    Delete {
        generation: u64,
        plan_id: u64,
        confirm: bool,
        preview_revision: &'a str,
    },
    Move {
        generation: u64,
        plan_id: u64,
        to: &'a str,
        rename: &'a str,
    },
    Reopen {
        generation: u64,
        plan_id: u64,
    },
    /// Makes `plan_id` the caller's current plan; `0` clears it.
    SetActive {
        generation: u64,
        plan_id: u64,
    },
    Copy {
        generation: u64,
        plan_id: u64,
        to: &'a str,
        rename: &'a str,
    },
}

#[derive(Clone, Copy, Debug)]
pub(super) enum TaskCommand<'a> {
    CreateFirst {
        generation: u64,
        plan_id: u64,
        title: &'a str,
    },
    StartFirst {
        generation: u64,
        task_id: u64,
        expected_updated_at: &'a str,
    },
    Add {
        generation: u64,
        plan_id: u64,
        title: &'a str,
    },
    Rename {
        generation: u64,
        task_id: u64,
        title: &'a str,
    },
    AddNote {
        generation: u64,
        task_id: u64,
        body: &'a str,
    },
    Move {
        generation: u64,
        task_id: u64,
        status: &'a str,
        confirmation_token: &'a str,
    },
    Detail {
        generation: u64,
        task_id: u64,
    },
}

#[derive(Clone, Copy, Debug)]
pub(super) enum IssueCommand<'a> {
    List {
        generation: u64,
        filter: &'a str,
        offset: u64,
    },
    Detail {
        generation: u64,
        issue_id: u64,
        query: &'a str,
    },
    Add {
        generation: u64,
        title: &'a str,
        body: &'a str,
        severity: &'a str,
    },
    Update {
        generation: u64,
        issue_id: u64,
        title: &'a str,
        body: &'a str,
        severity: &'a str,
        status: &'a str,
        expected_updated_at: &'a str,
    },
    SetTask {
        generation: u64,
        issue_id: u64,
        expected_task_id: u64,
        task_id: u64,
    },
    MoveTask {
        generation: u64,
        issue_id: u64,
        expected_task_id: u64,
        expected_plan_id: u64,
        plan_id: u64,
    },
    Schedule {
        generation: u64,
        issue_id: u64,
        plan_id: u64,
        title: &'a str,
    },
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ProjectCommand<'a> {
    ListProjects {
        generation: u64,
    },
    Search {
        query: &'a str,
    },
    ActivityHeatmap {
        weeks: i64,
    },
    Timeline,
    StackProfile {
        force: bool,
    },
    Snapshot {
        generation: u64,
        plan_id: Option<u64>,
    },
}

#[derive(Debug)]
pub(super) enum ScratchpadCommand {
    Get {
        generation: u64,
    },
    Set {
        generation: u64,
        revision: u64,
        scratchpad: ScratchpadV1,
    },
}

#[derive(Debug)]
pub(super) enum TerminalCommand<'a> {
    /// `GetTerminalProfiles` carries no generation and answers the bare
    /// profile list; `GetTerminalProfilesV2` is fenced and answers the whole
    /// profile document.
    Profiles {
        generation: Option<u64>,
    },
    ValidateCwds {
        generation: u64,
        cwds: Vec<String>,
    },
    Create {
        generation: u64,
        profile_id: &'a str,
        cwd: &'a str,
        rows: u16,
        columns: u16,
    },
    Resize {
        generation: u64,
        session_id: &'a str,
        rows: u16,
        columns: u16,
    },
    ClaimStream {
        session_id: &'a str,
        from_sequence: u64,
    },
    Close {
        generation: u64,
        session_id: &'a str,
        force: bool,
    },
    MutateAssociation {
        generation: u64,
        session_id: &'a str,
        expected_revision: u64,
        detach: bool,
        /// The relink target; a detach carries the empty project pointer.
        pointer: TerminalAssociationPointer,
    },
    PreviewWriteback {
        generation: u64,
        session_id: &'a str,
        revision: u64,
        kind: &'a str,
        content: &'a str,
    },
    WriteMemory {
        generation: u64,
        session_id: &'a str,
        revision: u64,
        request_id: &'a str,
        kind: &'a str,
        content: &'a str,
        /// Required only when `kind` is a summary replacement.
        confirm_summary: Option<bool>,
    },
}

#[derive(Clone, Copy, Debug)]
pub(super) enum AgentCommand<'a> {
    /// A terminal-backed agent launch linked to a plan or task, and its
    /// rollback.
    Linked(LinkedAgentCommand<'a>),
    /// Coordination with the runs the `AgentRun` registry tracks.
    Registry(AgentRegistryCommand<'a>),
}

#[derive(Clone, Copy, Debug)]
pub(super) enum LinkedAgentCommand<'a> {
    Launch {
        generation: u64,
        profile_id: &'a str,
        cwd: &'a str,
        rows: u16,
        columns: u16,
        pointer: TerminalAssociationPointer,
    },
    Rollback {
        generation: u64,
        session_id: &'a str,
    },
}

#[derive(Clone, Copy, Debug)]
pub(super) enum AgentRegistryCommand<'a> {
    PreviewHandoff {
        generation: u64,
        run_id: &'a str,
    },
    SendHandoff {
        generation: u64,
        source_run_id: &'a str,
        target_run_id: &'a str,
        expected_source_revision: u64,
        expected_target_revision: u64,
    },
    AcknowledgeHandoff {
        generation: u64,
        id: &'a str,
        target_run_id: &'a str,
    },
    SetTaskOwnership {
        generation: u64,
        run_id: &'a str,
        expected_association_revision: u64,
        owned: bool,
    },
    SetWorktree {
        generation: u64,
        run_id: &'a str,
        expected_association_revision: u64,
        root: &'a str,
        associated: bool,
    },
    PrepareWorkflow {
        generation: u64,
        run_id: &'a str,
        expected_association_revision: u64,
        kind: &'a str,
        target_branch: &'a str,
    },
    ApproveWorkflow {
        generation: u64,
        id: &'a str,
    },
    DismissWorkflow {
        generation: u64,
        id: &'a str,
    },
}

impl<'a> WorkspaceCommand<'a> {
    /// Parses one workspace method.
    ///
    /// Arguments are read in the order the handlers historically read them,
    /// so a request with several bad arguments still reports the same one.
    ///
    /// # Errors
    /// Returns `"{method} is unavailable"` for a method no workspace serves,
    /// else the bridge's arity or argument error.
    pub(super) fn parse(method: &'a str, arguments: &'a [Value]) -> AppResult<Self> {
        let args = Args::new(method, arguments);
        if let Some(command) = PlanCommand::parse(method, args)? {
            return Ok(Self::Plan(command));
        }
        if let Some(command) = TaskCommand::parse(method, args)? {
            return Ok(Self::Task(command));
        }
        if let Some(command) = IssueCommand::parse(method, args)? {
            return Ok(Self::Issue(command));
        }
        if let Some(command) = ProjectCommand::parse(method, args)? {
            return Ok(Self::Project(command));
        }
        if let Some(command) = ScratchpadCommand::parse(method, args)? {
            return Ok(Self::Scratchpad(command));
        }
        if let Some(command) = TerminalCommand::parse(method, args)? {
            return Ok(Self::Terminal(command));
        }
        if let Some(command) = AgentCommand::parse(method, args)? {
            return Ok(Self::Agent(command));
        }
        Err(unavailable(method))
    }
}

impl<'a> PlanCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        Ok(Some(match method {
            "AddPlanV1" => {
                args.exact(2)?;
                Self::Add {
                    generation: args.u64(0)?,
                    title: args.string(1)?,
                }
            }
            "CreateFirstPlanV1" => {
                args.exact(2)?;
                Self::CreateFirst {
                    generation: args.u64(0)?,
                    title: args.string(1)?,
                }
            }
            "RenamePlanV1" => {
                args.exact(3)?;
                let generation = args.u64(0)?;
                let title = args.string(2)?;
                Self::Rename {
                    generation,
                    plan_id: args.u64(1)?,
                    title,
                }
            }
            "CompletePlanV1" => {
                args.exact(2)?;
                Self::Complete {
                    generation: args.u64(0)?,
                    plan_id: args.u64(1)?,
                }
            }
            "HoldPlanV1" => {
                args.exact(3)?;
                let generation = args.u64(0)?;
                let reason = args.string(2)?;
                Self::Hold {
                    generation,
                    plan_id: args.u64(1)?,
                    reason,
                }
            }
            "ResumePlanV1" => {
                args.exact(2)?;
                Self::Resume {
                    generation: args.u64(0)?,
                    plan_id: args.u64(1)?,
                }
            }
            "DeletePlanV1" => Self::parse_delete(args)?,
            "MovePlanV1" => {
                args.exact(4)?;
                Self::Move {
                    generation: args.u64(0)?,
                    plan_id: args.u64(1)?,
                    to: args.string(2)?,
                    rename: args.string(3)?,
                }
            }
            "ReopenPlanV1" => {
                args.exact(2)?;
                Self::Reopen {
                    generation: args.u64(0)?,
                    plan_id: args.u64(1)?,
                }
            }
            "SetActivePlanV1" => {
                args.exact(2)?;
                Self::SetActive {
                    generation: args.u64(0)?,
                    plan_id: args.u64(1)?,
                }
            }
            "CopyPlanV1" => {
                args.exact(4)?;
                Self::Copy {
                    generation: args.u64(0)?,
                    plan_id: args.u64(1)?,
                    to: args.string(2)?,
                    rename: args.string(3)?,
                }
            }
            _ => return Ok(None),
        }))
    }

    fn parse_delete(args: Args<'a>) -> AppResult<Self> {
        if args.len() != 3 && args.len() != 4 {
            return Err(message(
                "plan delete expects generation, plan ID, confirmation, and optional preview revision",
            ));
        }
        let generation = args.u64(0)?;
        let preview_revision = if args.len() == 4 { args.string(3)? } else { "" };
        Ok(Self::Delete {
            generation,
            plan_id: args.u64(1)?,
            confirm: args.bool(2)?,
            preview_revision,
        })
    }
}

impl<'a> TaskCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        Ok(Some(match method {
            "CreateFirstTaskV1" => {
                args.exact(3)?;
                Self::CreateFirst {
                    generation: args.u64(0)?,
                    plan_id: args.u64(1)?,
                    title: args.string(2)?,
                }
            }
            "StartFirstTaskV1" => {
                args.exact(3)?;
                Self::StartFirst {
                    generation: args.u64(0)?,
                    task_id: args.u64(1)?,
                    expected_updated_at: args.string(2)?,
                }
            }
            "AddTaskV2" => {
                let generation = args.u64(0)?;
                let title = args.string(2)?;
                Self::Add {
                    generation,
                    plan_id: args.u64(1)?,
                    title,
                }
            }
            "RenameTaskV2" => {
                let generation = args.u64(0)?;
                let title = args.string(2)?;
                Self::Rename {
                    generation,
                    task_id: args.u64(1)?,
                    title,
                }
            }
            "AddTaskNoteV2" => {
                let generation = args.u64(0)?;
                let body = args.string(2)?;
                Self::AddNote {
                    generation,
                    task_id: args.u64(1)?,
                    body,
                }
            }
            "MoveTaskV3" => Self::Move {
                generation: args.u64(0)?,
                task_id: args.u64(1)?,
                status: args.string(2)?,
                confirmation_token: args.string(3)?,
            },
            "GetTaskDetailV2" => Self::Detail {
                generation: args.u64(0)?,
                task_id: args.u64(1)?,
            },
            _ => return Ok(None),
        }))
    }
}

impl<'a> IssueCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        Ok(Some(match method {
            "GetIssuesV1" => {
                args.exact(3)?;
                Self::List {
                    generation: args.u64(0)?,
                    filter: args.string(1)?,
                    offset: args.u64(2)?,
                }
            }
            "GetIssueDetailV1" => Self::parse_detail(args)?,
            "AddIssueV1" => {
                args.exact(4)?;
                let generation = args.u64(0)?;
                let title = args.string(1)?;
                let severity = args.string(3)?;
                Self::Add {
                    generation,
                    title,
                    body: args.string(2)?,
                    severity,
                }
            }
            "UpdateIssueV1" => Self::parse_update(args)?,
            "SetIssueTaskV1" => {
                args.exact(4)?;
                Self::SetTask {
                    generation: args.u64(0)?,
                    issue_id: args.u64(1)?,
                    expected_task_id: args.u64(2)?,
                    task_id: args.u64(3)?,
                }
            }
            "MoveIssueTaskV1" => {
                args.exact(5)?;
                let generation = args.u64(0)?;
                let expected_task_id = args.u64(2)?;
                Self::MoveTask {
                    generation,
                    issue_id: args.u64(1)?,
                    expected_task_id,
                    expected_plan_id: args.u64(3)?,
                    plan_id: args.u64(4)?,
                }
            }
            "ScheduleIssueV1" => {
                args.exact(4)?;
                Self::Schedule {
                    generation: args.u64(0)?,
                    issue_id: args.u64(1)?,
                    plan_id: args.u64(2)?,
                    title: args.string(3)?,
                }
            }
            _ => return Ok(None),
        }))
    }

    fn parse_detail(args: Args<'a>) -> AppResult<Self> {
        if args.len() != 2 && args.len() != 3 {
            return Err(message(
                "issue detail expects generation, issue ID, and optional target search",
            ));
        }
        let generation = args.u64(0)?;
        let query = if args.len() == 3 { args.string(2)? } else { "" };
        Ok(Self::Detail {
            generation,
            issue_id: args.u64(1)?,
            query,
        })
    }

    fn parse_update(args: Args<'a>) -> AppResult<Self> {
        args.exact(7)?;
        let generation = args.u64(0)?;
        let issue_id = args.u64(1)?;
        let expected_updated_at = args.string(6)?;
        Ok(Self::Update {
            generation,
            issue_id,
            title: args.string(2)?,
            body: args.string(3)?,
            severity: args.string(4)?,
            status: args.string(5)?,
            expected_updated_at,
        })
    }
}

impl<'a> ProjectCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        Ok(Some(match method {
            "ListProjectsV1" => {
                args.exact(1)?;
                Self::ListProjects {
                    generation: args.u64(0)?,
                }
            }
            "SearchV2" => Self::Search {
                query: args.string(0)?,
            },
            "GetActivityHeatmapV2" => Self::ActivityHeatmap {
                weeks: args.i64(0)?,
            },
            "GetProjectTimelineV1" => {
                args.exact(0)?;
                Self::Timeline
            }
            "GetStackProfileV1" => {
                args.exact(1)?;
                Self::StackProfile {
                    force: args.bool(0)?,
                }
            }
            "GetWorkspaceSnapshot" => Self::Snapshot {
                generation: args.u64(0)?,
                plan_id: if args.is_null(1) {
                    None
                } else {
                    Some(args.u64(1)?)
                },
            },
            _ => return Ok(None),
        }))
    }
}

impl ScratchpadCommand {
    fn parse(method: &str, args: Args<'_>) -> AppResult<Option<Self>> {
        Ok(Some(match method {
            "GetScratchpadV1" => Self::Get {
                generation: args.u64(0)?,
            },
            // The fence is the revision argument, never the revision inside
            // the payload: the caller states the revision it read, and the
            // runtime stamps the next one.
            "SetScratchpadV1" => Self::Set {
                generation: args.u64(0)?,
                revision: args.u64(1)?,
                scratchpad: args.typed(2)?,
            },
            _ => return Ok(None),
        }))
    }
}

impl<'a> TerminalCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        Ok(Some(match method {
            "GetTerminalProfiles" => Self::Profiles { generation: None },
            "GetTerminalProfilesV2" => Self::Profiles {
                generation: Some(args.u64(0)?),
            },
            "ValidateTerminalCWDsV2" => Self::ValidateCwds {
                generation: args.u64(0)?,
                cwds: args.strings(1)?,
            },
            "CreateTerminalV2" => {
                let generation = args.u64(0)?;
                let cwd = args.string(2)?;
                Self::Create {
                    generation,
                    profile_id: args.string(1)?,
                    cwd,
                    rows: args.u16(3)?,
                    columns: args.u16(4)?,
                }
            }
            "ResizeTerminalV2" => Self::Resize {
                generation: args.u64(0)?,
                session_id: args.string(1)?,
                rows: args.u16(2)?,
                columns: args.u16(3)?,
            },
            "ClaimTerminalStream" => {
                args.exact(2)?;
                Self::ClaimStream {
                    session_id: args.string(0)?,
                    from_sequence: args.u64(1)?,
                }
            }
            "CloseTerminalV2" => Self::Close {
                generation: args.u64(0)?,
                session_id: args.string(1)?,
                force: args.bool(2)?,
            },
            "MutateTerminalAssociationV2" => Self::parse_association(args)?,
            "PreviewTerminalWritebackV2" => Self::PreviewWriteback {
                generation: args.u64(0)?,
                session_id: args.string(1)?,
                revision: args.u64(2)?,
                kind: args.string(3)?,
                content: args.string(4)?,
            },
            "WriteTerminalMemoryV2" => Self::WriteMemory {
                generation: args.u64(0)?,
                session_id: args.string(1)?,
                revision: args.u64(2)?,
                request_id: args.string(3)?,
                kind: args.string(4)?,
                content: args.string(5)?,
                confirm_summary: args.optional_bool(6),
            },
            _ => return Ok(None),
        }))
    }

    fn parse_association(args: Args<'a>) -> AppResult<Self> {
        let generation = args.u64(0)?;
        let detach = args.bool(3)?;
        let pointer = if detach {
            TerminalAssociationPointer {
                version: 1,
                plan_id: 0,
                task_id: 0,
            }
        } else {
            args.pointer(4)?
        };
        Ok(Self::MutateAssociation {
            generation,
            session_id: args.string(1)?,
            expected_revision: args.u64(2)?,
            detach,
            pointer,
        })
    }
}

impl<'a> AgentCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        if let Some(command) = LinkedAgentCommand::parse(method, args)? {
            return Ok(Some(Self::Linked(command)));
        }
        Ok(AgentRegistryCommand::parse(method, args)?.map(Self::Registry))
    }
}

impl<'a> LinkedAgentCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        Ok(Some(match method {
            "LaunchLinkedAgentV2" => {
                let generation = args.u64(0)?;
                let profile_id = args.string(1)?;
                let pointer = args.pointer(5)?;
                Self::Launch {
                    generation,
                    profile_id,
                    cwd: args.string(2)?,
                    rows: args.u16(3)?,
                    columns: args.u16(4)?,
                    pointer,
                }
            }
            "RollbackLinkedAgentLaunchV2" => Self::Rollback {
                generation: args.u64(0)?,
                session_id: args.string(1)?,
            },
            _ => return Ok(None),
        }))
    }
}

impl<'a> AgentRegistryCommand<'a> {
    fn parse(method: &str, args: Args<'a>) -> AppResult<Option<Self>> {
        Ok(Some(match method {
            "PreviewAgentHandoffV2" => Self::PreviewHandoff {
                generation: args.u64(0)?,
                run_id: args.string(1)?,
            },
            "SendAgentHandoffV2" => Self::SendHandoff {
                generation: args.u64(0)?,
                source_run_id: args.string(1)?,
                target_run_id: args.string(2)?,
                expected_source_revision: args.u64(3)?,
                expected_target_revision: args.u64(4)?,
            },
            "AcknowledgeAgentHandoffV2" => Self::AcknowledgeHandoff {
                generation: args.u64(0)?,
                id: args.string(1)?,
                target_run_id: args.string(2)?,
            },
            "SetAgentTaskOwnershipV2" => Self::SetTaskOwnership {
                generation: args.u64(0)?,
                run_id: args.string(1)?,
                expected_association_revision: args.u64(2)?,
                owned: args.bool(3)?,
            },
            "SetAgentWorktreeV2" => Self::SetWorktree {
                generation: args.u64(0)?,
                run_id: args.string(1)?,
                expected_association_revision: args.u64(2)?,
                root: args.string(3)?,
                associated: args.bool(4)?,
            },
            "PrepareAgentWorkflowV2" => {
                let generation = args.u64(0)?;
                let kind = args.string(3)?;
                Self::PrepareWorkflow {
                    generation,
                    run_id: args.string(1)?,
                    expected_association_revision: args.u64(2)?,
                    kind,
                    target_branch: args.string(4)?,
                }
            }
            "ApproveAgentWorkflowV2" => Self::ApproveWorkflow {
                generation: args.u64(0)?,
                id: args.string(1)?,
            },
            "DismissAgentWorkflowV2" => Self::DismissWorkflow {
                generation: args.u64(0)?,
                id: args.string(1)?,
            },
            _ => return Ok(None),
        }))
    }
}
