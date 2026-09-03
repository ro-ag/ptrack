use std::ffi::OsString;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use ptrack_agent::{AgentHandoffInbox, AgentObservationClient, AgentRunObservationV1, AgentRunsV2};
use ptrack_capability::{
    McpCancellation, McpServeOutcome, ToolCall, client_for_project, serve_mcp,
    validate_session_environment,
};
use ptrack_core::{
    CheckpointView, Commit, Issue, IssueStatus, Milestone, MilestoneStatus, Note, NoteTarget, Plan,
    PlanStatus, ProjectRef, ProjectSnapshot, Severity, Task, TaskStatus, Timestamp, checkpoint,
    id_list, render_guide,
};
use ptrack_store::{
    ActiveBinding, ActorIdentity, GlobalStore, PinnedProjectDirectory, PlanDeleteSummary,
    ProjectStore,
};

const NO_PROJECT: &str = "no ptrack project found (run 'ptrack init')";
const HOOK_BEGIN: &str = "# ptrack:begin";
const HOOK_END: &str = "# ptrack:end";
const HOOK_BODY: &str = "command -v ptrack >/dev/null 2>&1 && ptrack commit record --sha \"$(git rev-parse HEAD)\" --subject \"$(git log -1 --pretty=%s)\" >/dev/null 2>&1 || true";

#[derive(Debug)]
pub enum AppError {
    NoProject,
    NotImplemented(&'static str),
    Message(String),
    Io(std::io::Error),
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoProject => formatter.write_str(NO_PROJECT),
            Self::NotImplemented(feature) => write!(formatter, "{feature} is not implemented"),
            Self::Message(message) => formatter.write_str(message),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::NoProject | Self::NotImplemented(_) | Self::Message(_) => None,
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ptrack_store::StoreError> for AppError {
    fn from(error: ptrack_store::StoreError) -> Self {
        Self::Message(error.to_string())
    }
}

impl From<ptrack_capability::ServerError> for AppError {
    fn from(error: ptrack_capability::ServerError) -> Self {
        Self::Message(error.to_string())
    }
}

#[cfg(unix)]
fn from_errno(error: rustix::io::Errno) -> AppError {
    AppError::Io(std::io::Error::from(error))
}

pub type AppResult<T> = Result<T, AppError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectEndpoint {
    pub root: PathBuf,
    pub database: PathBuf,
    pub binding: ActiveBinding,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceBindings {
    pub current_dir: PathBuf,
    pub project: Option<ProjectEndpoint>,
    pub global_database: PathBuf,
    pub global_binding: ActiveBinding,
    pub global_home: PathBuf,
    pub writer_version: String,
}

/// Explicit capability authority injected by the session host.
///
/// The token intentionally has no `Debug` implementation so diagnostics
/// cannot accidentally disclose it.
#[derive(Clone, Eq, PartialEq)]
pub struct CapabilitySessionEnvironment {
    token: String,
    project: Option<PathBuf>,
    generation: Option<String>,
}

impl CapabilitySessionEnvironment {
    #[must_use]
    pub fn new(token: String, project: Option<PathBuf>, generation: Option<String>) -> Self {
        Self {
            token,
            project,
            generation,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InitRequest {
    pub root: Option<PathBuf>,
    pub goal: String,
    pub force: bool,
    pub no_guide: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitResult {
    pub database: PathBuf,
    pub already_initialized: bool,
    pub guide_files: Vec<PathBuf>,
}

/// Re-registers a project store whose folder was physically moved on disk.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RelocateRequest {
    /// The moved project root; the current directory when absent.
    pub root: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelocateResult {
    pub root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Mutation {
    SetGoal(String),
    SetSummary(String),
    AddMilestone {
        title: String,
        due: Timestamp,
    },
    SetMilestoneStatus {
        id: u64,
        status: MilestoneStatus,
    },
    SetMilestoneDue {
        id: u64,
        due: Timestamp,
    },
    SetMilestoneTitle {
        id: u64,
        title: String,
    },
    AddPlan {
        title: String,
        milestone_id: u64,
    },
    SetPlanStatus {
        id: u64,
        status: PlanStatus,
    },
    /// `Some` holds the plan with that reason; `None` resumes it.
    SetPlanHold {
        id: u64,
        reason: Option<String>,
    },
    SetActivePlan(u64),
    /// Takes over a plan claimed by someone else and makes it active.
    StealPlan(u64),
    /// Gives up the caller's own claim on a plan.
    ReleasePlanClaim(u64),
    SetPlanTitle {
        id: u64,
        title: String,
    },
    /// Records that plan `id` depends on plan `dep_id`.
    AddPlanDep {
        id: u64,
        dep_id: u64,
    },
    /// Removes the plan `id` -> `dep_id` dependency edge.
    RemovePlanDep {
        id: u64,
        dep_id: u64,
    },
    AddTask {
        plan_id: u64,
        title: String,
    },
    SetTaskStatus {
        id: u64,
        status: TaskStatus,
    },
    /// `Some` holds the task with that reason; `None` resumes it.
    SetTaskHold {
        id: u64,
        reason: Option<String>,
    },
    SetTaskTitle {
        id: u64,
        title: String,
    },
    SetTaskPlan {
        id: u64,
        plan_id: u64,
    },
    /// Records that task `id` depends on task `dep_id`.
    AddTaskDep {
        id: u64,
        dep_id: u64,
    },
    /// Removes the task `id` -> `dep_id` dependency edge.
    RemoveTaskDep {
        id: u64,
        dep_id: u64,
    },
    ConvertTaskToPlan(u64),
    AddIssue {
        title: String,
        body: String,
        severity: Option<Severity>,
        task_id: u64,
    },
    SetIssueStatus {
        id: u64,
        status: IssueStatus,
    },
    SetIssueSeverity {
        id: u64,
        severity: Severity,
    },
    SetIssueTitle {
        id: u64,
        title: String,
    },
    AddNote {
        target: NoteTarget,
        target_id: u64,
        body: String,
    },
    AddCommit {
        sha: String,
        subject: String,
        plan_id: u64,
        task_id: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationResult {
    None,
    Milestone(Milestone),
    Plan(Plan),
    Task(Task),
    Issue(Issue),
    Note(Note),
    Commit(Commit),
}

/// Receipt for the shared agent-facing task completion use case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompleteTaskResult {
    pub task_id: u64,
    pub linked_commits: usize,
    pub closeout_note: Option<Note>,
    pub override_note: Option<Note>,
}

/// Receipt for the shared plan completion use case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletePlanResult {
    pub plan_id: u64,
    pub checkpoint: CheckpointView,
    pub override_note: Option<Note>,
}

/// Completes one plan only after all of its tasks are closed, then computes
/// the same whole-project checkpoint shown by the CLI.
///
/// `force` is retained for CLI compatibility. Forced completion records the
/// exact open task IDs before changing the terminal plan status.
///
/// # Errors
/// Returns an application error when the plan is absent or inaccessible,
/// open tasks remain without `force`, or an audit/status mutation fails.
pub fn complete_plan(
    application: &mut dyn ApplicationPort,
    plan_id: u64,
    force: bool,
) -> AppResult<CompletePlanResult> {
    let snapshot = application.snapshot()?;
    let open_tasks = snapshot
        .tasks_for_plan(plan_id)
        .filter(|task| task.status.is_open())
        .map(|task| task.id)
        .collect::<Vec<_>>();
    if !open_tasks.is_empty() && !force {
        return Err(AppError::Message(format!(
            "cannot close plan #{plan_id}: open tasks remain ({}); finish them or pass --force",
            id_list(&open_tasks)
        )));
    }
    expect_no_mutation_result(&application.mutate(Mutation::SetPlanStatus {
        id: plan_id,
        status: PlanStatus::Done,
    })?)?;
    let override_note = if open_tasks.is_empty() {
        None
    } else {
        Some(expect_note_result(application.mutate(
            Mutation::AddNote {
                target: NoteTarget::Plan,
                target_id: plan_id,
                body: format!(
                    "override: closed via --force with open tasks {}",
                    id_list(&open_tasks)
                ),
            },
        )?)?)
    };
    Ok(CompletePlanResult {
        plan_id,
        checkpoint: checkpoint(&application.snapshot()?, Some(plan_id)),
        override_note,
    })
}

/// Completes one task while enforcing the agent workflow's evidence gate.
///
/// A nonblank summary and at least one linked commit are required unless
/// `force` is set. Forced omissions are recorded as an override note.
///
/// # Errors
/// Returns an application error when evidence is missing, the task is absent or
/// inaccessible, or either the status or audit-note mutation fails.
pub fn complete_task(
    application: &mut dyn ApplicationPort,
    task_id: u64,
    summary: Option<String>,
    force: bool,
) -> AppResult<CompleteTaskResult> {
    let summary = summary
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let snapshot = application.snapshot()?;
    let linked_commits = snapshot
        .commits
        .iter()
        .filter(|commit| commit.task_id == task_id)
        .count();
    let mut missing = Vec::new();
    if summary.is_none() {
        missing.push("--summary \"what changed, where it is wired in, what remains\" is required");
    }
    if linked_commits == 0 {
        missing.push(
            "no commit is linked: put #<task-id> in the commit message \
             (ptrack hook install records it) or run ptrack commit record",
        );
    }
    if !missing.is_empty() && !force {
        return Err(AppError::Message(format!(
            "cannot close task #{task_id}: {} (or pass --force)",
            missing.join("; ")
        )));
    }

    let closeout_note = summary
        .map(|summary| {
            expect_note_result(application.mutate(Mutation::AddNote {
                target: NoteTarget::Task,
                target_id: task_id,
                body: format!("closeout: {summary}"),
            })?)
        })
        .transpose()?;
    let override_note = if missing.is_empty() {
        None
    } else {
        Some(expect_note_result(application.mutate(
            Mutation::AddNote {
                target: NoteTarget::Task,
                target_id: task_id,
                body: format!("override: closed via --force ({})", missing.join("; ")),
            },
        )?)?)
    };
    // Required audit evidence must exist before the irreversible status
    // transition. A note-write failure therefore leaves the task open.
    expect_no_mutation_result(&application.mutate(Mutation::SetTaskStatus {
        id: task_id,
        status: TaskStatus::Done,
    })?)?;
    Ok(CompleteTaskResult {
        task_id,
        linked_commits,
        closeout_note,
        override_note,
    })
}

fn expect_no_mutation_result(result: &MutationResult) -> AppResult<()> {
    if matches!(result, &MutationResult::None) {
        Ok(())
    } else {
        Err(AppError::Message(
            "internal mutation result mismatch".to_owned(),
        ))
    }
}

fn expect_note_result(result: MutationResult) -> AppResult<Note> {
    if let MutationResult::Note(note) = result {
        Ok(note)
    } else {
        Err(AppError::Message(
            "internal mutation result mismatch".to_owned(),
        ))
    }
}

/// A plan lifecycle operation: destructive delete, or a transfer of the whole
/// plan subtree into another project (or back into this one, as a copy).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanLifecycleRequest {
    DeletePreview {
        plan_id: u64,
    },
    Delete {
        plan_id: u64,
    },
    Move {
        plan_id: u64,
        to: String,
        rename: Option<String>,
    },
    Copy {
        plan_id: u64,
        to: Option<String>,
        rename: Option<String>,
    },
}

/// What a completed move or copy actually carried, for the receipt a caller
/// prints.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanTransferSummary {
    pub source_plan_id: u64,
    pub new_plan_id: u64,
    pub title: String,
    pub source_project: String,
    pub target_project: String,
    pub moved: bool,
    pub tasks: usize,
    pub notes: usize,
    pub issues: usize,
    pub commits: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanLifecycleOutcome {
    Preview(PlanDeleteSummary),
    Deleted(PlanDeleteSummary),
    Transferred(PlanTransferSummary),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuideAction {
    Print,
    Install,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HookAction {
    Install,
    Uninstall,
    Status,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HookResult {
    Installed { path: PathBuf, changed: bool },
    Removed,
    Missing,
    Status { path: PathBuf, installed: bool },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityMcpOutcome {
    Complete,
    Cancelled,
}

pub type CapabilityCancellation = McpCancellation;

/// The single use-case seam consumed by CLI, TUI, and Tauri adapters.
#[allow(clippy::missing_errors_doc)]
pub trait ApplicationPort {
    fn initialize(&mut self, request: InitRequest) -> AppResult<InitResult>;
    /// Re-registers a moved project store. Only the marker-owning routed
    /// application can do this; everywhere else the default refusal applies.
    fn relocate(&mut self, request: RelocateRequest) -> AppResult<RelocateResult> {
        let _ = request;
        Err(AppError::Message(
            "project relocation is unavailable in this context".to_owned(),
        ))
    }
    fn snapshot(&mut self) -> AppResult<ProjectSnapshot>;
    fn mutate(&mut self, mutation: Mutation) -> AppResult<MutationResult>;
    fn plan_lifecycle(&mut self, request: PlanLifecycleRequest) -> AppResult<PlanLifecycleOutcome>;
    fn projects(&mut self) -> AppResult<Vec<ProjectRef>>;
    fn identity(&mut self) -> AppResult<Option<ActorIdentity>>;
    fn set_identity(&mut self, name: &str) -> AppResult<ActorIdentity>;
    fn backup(&mut self) -> AppResult<PathBuf>;
    fn guide(&mut self, action: GuideAction) -> AppResult<(String, Vec<PathBuf>)>;
    fn hook(&mut self, action: HookAction) -> AppResult<HookResult>;
    fn git_show(&mut self, reference: &str, stat: bool) -> AppResult<ProcessOutput>;
    fn capability_call(&mut self, tool: &str, arguments: &str) -> AppResult<Vec<u8>>;
    fn capability_mcp(
        &mut self,
        input: Box<dyn Read + Send>,
        output: &mut dyn Write,
        cancellation: &CapabilityCancellation,
    ) -> AppResult<CapabilityMcpOutcome>;
    fn agent_runs(&mut self) -> AppResult<AgentRunsV2> {
        Err(no_coordination_host())
    }
    fn agent_run(&mut self, _run_id: &str) -> AppResult<AgentRunObservationV1> {
        Err(no_coordination_host())
    }
    fn agent_inbox(&mut self) -> AppResult<AgentHandoffInbox> {
        Err(no_coordination_host())
    }
}

/// Fail-closed process placeholder used until the activation-marker owner has
/// supplied explicit bindings. Help, version, completion, and launch routing
/// remain available; any data operation is refused.
#[derive(Default)]
pub struct UnavailableApplication;

impl ApplicationPort for UnavailableApplication {
    fn initialize(&mut self, _request: InitRequest) -> AppResult<InitResult> {
        Err(unavailable())
    }

    fn snapshot(&mut self) -> AppResult<ProjectSnapshot> {
        Err(unavailable())
    }

    fn mutate(&mut self, _mutation: Mutation) -> AppResult<MutationResult> {
        Err(unavailable())
    }

    fn plan_lifecycle(
        &mut self,
        _request: PlanLifecycleRequest,
    ) -> AppResult<PlanLifecycleOutcome> {
        Err(unavailable())
    }

    fn projects(&mut self) -> AppResult<Vec<ProjectRef>> {
        Err(unavailable())
    }

    fn identity(&mut self) -> AppResult<Option<ActorIdentity>> {
        Err(unavailable())
    }

    fn set_identity(&mut self, _name: &str) -> AppResult<ActorIdentity> {
        Err(unavailable())
    }

    fn backup(&mut self) -> AppResult<PathBuf> {
        Err(unavailable())
    }

    fn guide(&mut self, _action: GuideAction) -> AppResult<(String, Vec<PathBuf>)> {
        Err(unavailable())
    }

    fn hook(&mut self, _action: HookAction) -> AppResult<HookResult> {
        Err(unavailable())
    }

    fn git_show(&mut self, _reference: &str, _stat: bool) -> AppResult<ProcessOutput> {
        Err(unavailable())
    }

    fn capability_call(&mut self, _tool: &str, _arguments: &str) -> AppResult<Vec<u8>> {
        Err(unavailable())
    }

    fn capability_mcp(
        &mut self,
        _input: Box<dyn Read + Send>,
        _output: &mut dyn Write,
        _cancellation: &CapabilityCancellation,
    ) -> AppResult<CapabilityMcpOutcome> {
        Err(unavailable())
    }
}

fn unavailable() -> AppError {
    AppError::Message("active runtime binding is unavailable".to_owned())
}

fn no_coordination_host() -> AppError {
    AppError::Message("no active agent coordination host for this project".to_owned())
}

pub struct LocalApplication {
    bindings: WorkspaceBindings,
    capability_environment: Option<CapabilitySessionEnvironment>,
}

impl LocalApplication {
    #[must_use]
    pub const fn new(bindings: WorkspaceBindings) -> Self {
        Self {
            bindings,
            capability_environment: None,
        }
    }

    #[must_use]
    pub fn with_capability_environment(
        mut self,
        environment: CapabilitySessionEnvironment,
    ) -> Self {
        self.capability_environment = Some(environment);
        self
    }

    fn capability_environment(&self) -> AppResult<CapabilitySessionEnvironment> {
        if let Some(environment) = &self.capability_environment {
            return Ok(environment.clone());
        }
        let token = std::env::var("PTRACK_CAPABILITY_TOKEN").map_err(|_| {
            AppError::Message(
                "capability broker token is unavailable; launch this command from an agent terminal in p-track"
                    .to_owned(),
            )
        })?;
        Ok(CapabilitySessionEnvironment {
            token,
            project: std::env::var_os("PTRACK_CAPABILITY_PROJECT").map(PathBuf::from),
            generation: std::env::var("PTRACK_CAPABILITY_GENERATION").ok(),
        })
    }

    fn project(&self) -> AppResult<&ProjectEndpoint> {
        self.bindings.project.as_ref().ok_or(AppError::NoProject)
    }

    fn agent_client(&self) -> AppResult<AgentObservationClient> {
        let endpoint = self.project()?;
        AgentObservationClient::for_project(&self.bindings.global_home, &endpoint.root).map_err(
            |error| {
                if error.to_string() == "no active agent coordination host" {
                    no_coordination_host()
                } else {
                    AppError::Message(error.to_string())
                }
            },
        )
    }

    fn with_project<R>(
        &self,
        operation: impl FnOnce(&ProjectStore) -> AppResult<R>,
    ) -> AppResult<R> {
        let endpoint = self.project()?;
        let actor = self.with_global(crate::identity::load_identity)?;
        let store = ProjectStore::open_existing(
            &endpoint.database,
            &endpoint.binding,
            &self.bindings.writer_version,
        )?
        .with_actor(actor);
        let result = operation(&store);
        drop(store);
        if result.is_ok() {
            self.register_project_best_effort(endpoint);
        }
        result
    }

    fn with_global<R>(&self, operation: impl FnOnce(&GlobalStore) -> AppResult<R>) -> AppResult<R> {
        let store = GlobalStore::open_existing(
            &self.bindings.global_database,
            &self.bindings.global_binding,
        )?;
        let result = operation(&store);
        drop(store);
        result
    }

    fn register_project_best_effort(&self, endpoint: &ProjectEndpoint) {
        let Ok(store) = GlobalStore::open_existing(
            &self.bindings.global_database,
            &self.bindings.global_binding,
        ) else {
            return;
        };
        let name = endpoint
            .root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let _ = store.register_project(name, &endpoint.root);
    }

    /// Finds a registered target project by name or path, exactly as
    /// `ptrack projects` prints them. Registry-only: no marker resolution here,
    /// so "is this the current project?" can be answered without an active
    /// runtime lookup.
    ///
    /// A path wins outright. Names are directory basenames and therefore not
    /// unique, so an ambiguous name is refused rather than resolved by
    /// registry order — silently picking the most recently seen `web` would
    /// land a destructive move in a project the caller never named.
    fn lookup_registered_project(&self, to: &str) -> AppResult<ProjectRef> {
        let projects = self.with_global(|store| Ok(store.projects()?))?;
        if let Some(exact) = projects
            .iter()
            .find(|project| Path::new(&project.path) == Path::new(to))
        {
            return Ok(exact.clone());
        }
        let mut by_name = projects.into_iter().filter(|project| project.name == to);
        let first = by_name.next().ok_or_else(|| {
            AppError::Message(format!(
                "unknown target project {to:?}; run 'ptrack projects' for registered names and paths"
            ))
        })?;
        let mut paths = vec![first.path.clone()];
        paths.extend(by_name.map(|project| project.path));
        if paths.len() == 1 {
            return Ok(first);
        }
        Err(AppError::Message(format!(
            "target project {to:?} is ambiguous ({}); name it by path",
            paths.join(", ")
        )))
    }

    /// Resolves a registered project to an openable endpoint through the
    /// active-generation marker. Only called for a project other than the
    /// current one.
    fn endpoint_for_registered(&self, project: &ProjectRef) -> AppResult<ProjectEndpoint> {
        let runtime =
            crate::ActiveRuntime::load(&self.bindings.global_home, &self.bindings.writer_version)?
                .ok_or_else(|| {
                    AppError::Message("active runtime binding is unavailable".to_owned())
                })?;
        let bindings = runtime
            .bindings_for_exact_root(Path::new(&project.path))
            .map_err(|error| match error {
                AppError::NoProject => AppError::Message(format!(
                    "target project {} has no active database binding; run 'ptrack init' inside it once",
                    project.path
                )),
                // A stale registry row whose directory moved or vanished
                // surfaces as a bare io error otherwise, naming nothing.
                other => AppError::Message(format!(
                    "cannot resolve target project {}: {other}",
                    project.path
                )),
            })?;
        bindings.project.ok_or(AppError::NoProject)
    }

    fn transfer_plan(
        &self,
        plan_id: u64,
        to: Option<&str>,
        rename: Option<String>,
        is_move: bool,
    ) -> AppResult<PlanLifecycleOutcome> {
        let source = self.project()?.clone();
        let target_ref = to
            .map(|to| self.lookup_registered_project(to))
            .transpose()?;
        let same_project = target_ref
            .as_ref()
            .is_none_or(|project| Path::new(&project.path) == source.root.as_path());
        if is_move && same_project {
            return Err(AppError::Message(
                "target project is the current project; rename it in place with 'ptrack plan rename'"
                    .to_owned(),
            ));
        }
        if !is_move && same_project && rename.is_none() {
            return Err(AppError::Message(
                "copying into the same project requires --as <new title>".to_owned(),
            ));
        }
        let target = if same_project {
            None
        } else {
            Some(
                self.endpoint_for_registered(
                    target_ref
                        .as_ref()
                        .expect("cross-project transfer has a registry entry"),
                )?,
            )
        };
        let actor = self.with_global(crate::identity::load_identity)?;
        let writer_version = self.bindings.writer_version.clone();
        let source_label = project_label(&source.root);
        self.with_project(|store| {
            let subtree = store.export_plan_subtree(plan_id)?;
            let (tasks, notes, issues, commits) = (
                subtree.tasks.len(),
                subtree.notes.len(),
                subtree.issues.len(),
                subtree.commits.len(),
            );
            let (new_plan, target_label) = if same_project {
                (
                    store.import_plan_subtree(&subtree, rename)?,
                    source_label.clone(),
                )
            } else {
                let endpoint = target.as_ref().expect("cross-project transfer has a target");
                let target_store = ProjectStore::open_existing(
                    &endpoint.database,
                    &endpoint.binding,
                    &writer_version,
                )
                .map_err(|error| target_open_error(&endpoint.root, &error))?
                .with_actor(actor.clone());
                let plan = target_store.import_plan_subtree(&subtree, rename)?;
                drop(target_store);
                (plan, project_label(&endpoint.root))
            };
            if is_move {
                // Only after the target transaction has committed. Issues that
                // traveled are deleted here, not detached — they follow their
                // task. A failure here leaves a visible duplicate rather than a
                // lost plan, so the refusal has to name both sides.
                store.delete_plan_for_move(plan_id).map_err(|error| {
                    AppError::Message(format!(
                        "plan #{plan_id} was copied into {target_label} as #{} but could not be removed from {source_label}: {error}; the plan now exists in both projects — remove the source copy with 'ptrack plan delete'",
                        new_plan.id
                    ))
                })?;
            }
            Ok(PlanLifecycleOutcome::Transferred(PlanTransferSummary {
                source_plan_id: plan_id,
                new_plan_id: new_plan.id,
                title: new_plan.title,
                source_project: source_label.clone(),
                target_project: target_label,
                moved: is_move,
                tasks,
                notes,
                issues,
                commits,
            }))
        })
    }

    fn verified_root(&self) -> AppResult<PathBuf> {
        self.with_project(|_| Ok(self.project()?.root.clone()))
    }

    fn guide_extra(&self) -> AppResult<String> {
        let path = self.bindings.global_home.join("guide.md");
        Ok(read_regular(&path, "guide template")?.map_or_else(String::new, |file| file.content))
    }
}

impl ApplicationPort for LocalApplication {
    fn initialize(&mut self, request: InitRequest) -> AppResult<InitResult> {
        let endpoint = self.project()?.clone();
        let target = request.root.as_deref().unwrap_or(&endpoint.root);
        let target = fs::canonicalize(target)?;
        if target != endpoint.root {
            if !request.force {
                return Err(AppError::Message(format!(
                    "already inside ptrack project at {}; run 'ptrack guide' to refresh docs, or 'ptrack init --force' to nest a new project",
                    endpoint.root.display()
                )));
            }
            return Err(AppError::Message(
                "explicit active binding for the nested project is unavailable".to_owned(),
            ));
        }
        let already_initialized = endpoint.database.exists();
        if already_initialized {
            let store = ProjectStore::open_existing(
                &endpoint.database,
                &endpoint.binding,
                &self.bindings.writer_version,
            )?;
            if !request.goal.is_empty() {
                store.set_goal(request.goal)?;
            }
            drop(store);
        } else {
            fs::create_dir_all(
                endpoint.database.parent().ok_or_else(|| {
                    AppError::Message("project database has no parent".to_owned())
                })?,
            )?;
            let store = ProjectStore::create_new(
                &endpoint.database,
                endpoint.binding.clone(),
                &self.bindings.writer_version,
            )?;
            if !request.goal.is_empty() {
                store.set_goal(request.goal)?;
            }
            drop(store);
        }
        self.register_project_best_effort(&endpoint);
        let guide_files = if request.no_guide {
            Vec::new()
        } else {
            self.guide(GuideAction::Install)?.1
        };
        Ok(InitResult {
            database: endpoint.database,
            already_initialized,
            guide_files,
        })
    }

    fn snapshot(&mut self) -> AppResult<ProjectSnapshot> {
        self.with_project(|store| Ok(store.snapshot()?))
    }

    fn agent_runs(&mut self) -> AppResult<AgentRunsV2> {
        self.agent_client()?
            .runs()
            .map_err(|error| AppError::Message(error.to_string()))
    }

    fn agent_run(&mut self, run_id: &str) -> AppResult<AgentRunObservationV1> {
        self.agent_client()?
            .run(run_id)
            .map_err(|error| AppError::Message(error.to_string()))
    }

    fn agent_inbox(&mut self) -> AppResult<AgentHandoffInbox> {
        self.agent_client()?
            .inbox()
            .map_err(|error| AppError::Message(error.to_string()))
    }

    // One flat arm per mutation; splitting it would only hide the dispatch.
    #[allow(clippy::too_many_lines)]
    fn mutate(&mut self, mutation: Mutation) -> AppResult<MutationResult> {
        self.with_project(|store| {
            let result = match mutation {
                Mutation::SetGoal(value) => {
                    store.set_goal(value)?;
                    MutationResult::None
                }
                Mutation::SetSummary(value) => {
                    store.set_summary(value)?;
                    MutationResult::None
                }
                Mutation::AddMilestone { title, due } => {
                    let value = store.add_milestone(title)?;
                    if !due.is_zero() {
                        store.set_milestone_due(value.id, due)?;
                    }
                    MutationResult::Milestone(value)
                }
                Mutation::SetMilestoneStatus { id, status } => {
                    store.set_milestone_status(id, status)?;
                    MutationResult::None
                }
                Mutation::SetMilestoneDue { id, due } => {
                    store.set_milestone_due(id, due)?;
                    MutationResult::None
                }
                Mutation::SetMilestoneTitle { id, title } => {
                    store.set_milestone_title(id, title)?;
                    MutationResult::None
                }
                Mutation::AddPlan {
                    title,
                    milestone_id,
                } => MutationResult::Plan(store.add_plan(title, milestone_id)?),
                Mutation::SetPlanStatus { id, status } => {
                    store.set_plan_status(id, status)?;
                    MutationResult::None
                }
                Mutation::SetPlanHold { id, reason } => {
                    store.set_plan_hold(id, reason)?;
                    MutationResult::None
                }
                Mutation::SetActivePlan(id) => {
                    store.set_active_plan(id)?;
                    MutationResult::None
                }
                Mutation::StealPlan(id) => {
                    store.use_plan(id, true)?;
                    MutationResult::None
                }
                Mutation::ReleasePlanClaim(id) => {
                    store.release_plan(id)?;
                    MutationResult::None
                }
                Mutation::SetPlanTitle { id, title } => {
                    store.set_plan_title(id, title)?;
                    MutationResult::None
                }
                Mutation::AddPlanDep { id, dep_id } => {
                    store.add_plan_dep(id, dep_id)?;
                    MutationResult::None
                }
                Mutation::RemovePlanDep { id, dep_id } => {
                    store.remove_plan_dep(id, dep_id)?;
                    MutationResult::None
                }
                Mutation::AddTask { plan_id, title } => {
                    MutationResult::Task(store.add_task(plan_id, title)?)
                }
                Mutation::SetTaskStatus { id, status } => {
                    store.set_task_status(id, status)?;
                    MutationResult::None
                }
                Mutation::SetTaskHold { id, reason } => {
                    store.set_task_hold(id, reason)?;
                    MutationResult::None
                }
                Mutation::SetTaskTitle { id, title } => {
                    store.set_task_title(id, title)?;
                    MutationResult::None
                }
                Mutation::SetTaskPlan { id, plan_id } => {
                    store.set_task_plan(id, plan_id)?;
                    MutationResult::None
                }
                Mutation::AddTaskDep { id, dep_id } => {
                    store.add_task_dep(id, dep_id)?;
                    MutationResult::None
                }
                Mutation::RemoveTaskDep { id, dep_id } => {
                    store.remove_task_dep(id, dep_id)?;
                    MutationResult::None
                }
                Mutation::ConvertTaskToPlan(id) => {
                    MutationResult::Plan(store.convert_task_to_plan(id)?)
                }
                Mutation::AddIssue {
                    title,
                    body,
                    severity,
                    task_id,
                } => MutationResult::Issue(store.add_issue(title, body, severity, task_id)?),
                Mutation::SetIssueStatus { id, status } => {
                    store.set_issue_status(id, status)?;
                    MutationResult::None
                }
                Mutation::SetIssueSeverity { id, severity } => {
                    store.set_issue_severity(id, severity)?;
                    MutationResult::None
                }
                Mutation::SetIssueTitle { id, title } => {
                    store.set_issue_title(id, title)?;
                    MutationResult::None
                }
                Mutation::AddNote {
                    target,
                    target_id,
                    body,
                } => MutationResult::Note(store.add_note(target, target_id, body)?),
                Mutation::AddCommit {
                    sha,
                    subject,
                    plan_id,
                    task_id,
                } => MutationResult::Commit(store.add_commit(sha, subject, plan_id, task_id)?),
            };
            Ok(result)
        })
    }

    fn plan_lifecycle(&mut self, request: PlanLifecycleRequest) -> AppResult<PlanLifecycleOutcome> {
        match request {
            PlanLifecycleRequest::DeletePreview { plan_id } => self.with_project(|store| {
                Ok(PlanLifecycleOutcome::Preview(
                    store.plan_delete_preview(plan_id)?,
                ))
            }),
            PlanLifecycleRequest::Delete { plan_id } => self.with_project(|store| {
                Ok(PlanLifecycleOutcome::Deleted(store.delete_plan(plan_id)?))
            }),
            PlanLifecycleRequest::Move {
                plan_id,
                to,
                rename,
            } => self.transfer_plan(plan_id, Some(&to), rename, true),
            PlanLifecycleRequest::Copy {
                plan_id,
                to,
                rename,
            } => self.transfer_plan(plan_id, to.as_deref(), rename, false),
        }
    }

    fn projects(&mut self) -> AppResult<Vec<ProjectRef>> {
        self.with_global(|store| Ok(store.projects()?))
    }

    fn identity(&mut self) -> AppResult<Option<ActorIdentity>> {
        self.with_global(crate::identity::load_identity)
    }

    fn set_identity(&mut self, name: &str) -> AppResult<ActorIdentity> {
        self.with_global(|store| crate::identity::set_identity_name(store, name))
    }

    fn backup(&mut self) -> AppResult<PathBuf> {
        let endpoint = self.project()?.clone();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| AppError::Message(error.to_string()))?
            .as_secs();
        let name = endpoint
            .root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("project");
        let destination = self
            .bindings
            .global_home
            .join("backups")
            .join(format!("{name}-{timestamp}.db"));
        self.with_project(|store| Ok(store.backup_to(&destination)?))?;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(i64::MAX, |value| {
                i64::try_from(value.as_nanos()).unwrap_or(i64::MAX)
            });
        let _ = self.with_global(|store| {
            store.record_backup(nanos, &endpoint.root, &destination)?;
            Ok(())
        });
        Ok(destination)
    }

    fn guide(&mut self, action: GuideAction) -> AppResult<(String, Vec<PathBuf>)> {
        let extra = self.guide_extra()?;
        if action == GuideAction::Print {
            return Ok((render_guide(&extra), Vec::new()));
        }
        let root = self.verified_root()?;
        let root_identity = PinnedProjectDirectory::identify_root(&root)?;
        let directory_identity = PinnedProjectDirectory::identify_directory(&root)?;
        let publication = PinnedProjectDirectory::prepare_expected_identities(
            &root,
            root_identity,
            directory_identity,
        )?;
        let written = crate::production::install_project_guide_pinned(&publication, &extra)?
            .into_iter()
            .map(|name| root.join(name))
            .collect();
        Ok((String::new(), written))
    }

    fn hook(&mut self, action: HookAction) -> AppResult<HookResult> {
        let root = self.verified_root()?;
        let git_directory = root.join(".git");
        let metadata = fs::symlink_metadata(&git_directory).map_err(|_| {
            AppError::Message(format!(
                ".git is not a directory at {} — install the hook manually",
                git_directory.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(AppError::Message(format!(
                ".git is not a directory at {} — install the hook manually",
                git_directory.display()
            )));
        }
        let path = git_directory.join("hooks").join("post-commit");
        match action {
            HookAction::Install => {
                ensure_directory(
                    path.parent().expect("hook path has parent"),
                    "hook directory",
                )?;
                let existing = read_regular(&path, "post-commit hook")?;
                let (updated, changed) =
                    upsert_hook(existing.as_ref().map_or("", |file| &file.content));
                if changed {
                    atomic_publish(&path, &updated, existing.as_ref(), 0o755, "hook")?;
                }
                Ok(HookResult::Installed { path, changed })
            }
            HookAction::Uninstall => {
                let Some(existing) = read_regular(&path, "post-commit hook")? else {
                    return Ok(HookResult::Missing);
                };
                let stripped = strip_hook(&existing.content);
                if matches!(stripped.trim(), "" | "#!/bin/sh") {
                    remove_pinned(&path, &existing)?;
                } else if stripped != existing.content {
                    atomic_publish(&path, &stripped, Some(&existing), 0o755, "hook")?;
                }
                Ok(HookResult::Removed)
            }
            HookAction::Status => {
                let installed = read_regular(&path, "post-commit hook")?
                    .is_some_and(|file| file.content.contains(HOOK_BEGIN));
                Ok(HookResult::Status { path, installed })
            }
        }
    }

    fn git_show(&mut self, reference: &str, stat: bool) -> AppResult<ProcessOutput> {
        let root = self.verified_root()?;
        let mut command = Command::new("git");
        command.arg("-C").arg(root).arg("show");
        if stat {
            command.arg("--stat");
        }
        command
            .arg(reference)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("GIT_OPTIONAL_LOCKS", "0");
        let output = command.output()?;
        Ok(ProcessOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.status.code(),
        })
    }

    fn capability_call(&mut self, tool: &str, arguments: &str) -> AppResult<Vec<u8>> {
        let endpoint = self.project()?;
        let environment = self.capability_environment()?;
        let client = client_for_project(&self.bindings.global_home, &endpoint.root)?;
        validate_session_environment(
            client.descriptor(),
            environment.project.as_deref(),
            environment.generation.as_deref(),
        )?;
        let arguments = serde_json::from_str(arguments)
            .map_err(|_| AppError::Message("tool arguments are invalid".to_owned()))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| {
                AppError::Message("capability client runtime is unavailable".to_owned())
            })?;
        let result = runtime.block_on(client.call(
            &environment.token,
            &ToolCall {
                name: tool.to_owned(),
                arguments,
            },
        ))?;
        serde_json::to_vec(&result)
            .map_err(|_| AppError::Message("capability response could not be encoded".to_owned()))
    }

    fn capability_mcp(
        &mut self,
        input: Box<dyn Read + Send>,
        output: &mut dyn Write,
        cancellation: &CapabilityCancellation,
    ) -> AppResult<CapabilityMcpOutcome> {
        let endpoint = self.project()?;
        let environment = self.capability_environment()?;
        let client = client_for_project(&self.bindings.global_home, &endpoint.root)?;
        validate_session_environment(
            client.descriptor(),
            environment.project.as_deref(),
            environment.generation.as_deref(),
        )?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| {
                AppError::Message("capability client runtime is unavailable".to_owned())
            })?;
        let outcome = serve_mcp(input, output, cancellation, |cancellation, call| {
            runtime
                .block_on(client.call_cancellable(cancellation, &environment.token, &call))
                .map_err(|error| error.to_string())
        })
        .map_err(|error| AppError::Message(error.to_string()))?;
        Ok(match outcome {
            McpServeOutcome::Complete => CapabilityMcpOutcome::Complete,
            McpServeOutcome::Cancelled => CapabilityMcpOutcome::Cancelled,
        })
    }
}

#[derive(Clone)]
struct EntryIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    length: u64,
    #[cfg(not(unix))]
    modified: Option<SystemTime>,
}

impl EntryIdentity {
    fn capture(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                length: metadata.len(),
                modified: metadata.modified().ok(),
            }
        }
    }

    fn matches(&self, metadata: &fs::Metadata) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            self.device == metadata.dev() && self.inode == metadata.ino()
        }
        #[cfg(not(unix))]
        {
            self.length == metadata.len() && self.modified == metadata.modified().ok()
        }
    }
}

struct RegularFile {
    content: String,
    #[cfg_attr(not(unix), allow(dead_code))]
    identity: EntryIdentity,
    #[cfg_attr(not(unix), allow(dead_code))]
    mode: u32,
}

#[cfg_attr(not(unix), allow(dead_code))]
struct PinnedDirectory {
    path: PathBuf,
    identity: EntryIdentity,
    handle: fs::File,
}

impl PinnedDirectory {
    fn capture(path: &Path, label: &str) -> AppResult<Self> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(AppError::Message(format!(
                "{label} is not a directory: {}",
                path.display()
            )));
        }
        let handle = fs::File::open(path)?;
        let handle_metadata = handle.metadata()?;
        let identity = EntryIdentity::capture(&metadata);
        if !identity.matches(&handle_metadata) {
            return Err(AppError::Message(format!(
                "{label} changed while opening: {}",
                path.display()
            )));
        }
        Ok(Self {
            path: fs::canonicalize(path)?,
            identity,
            handle,
        })
    }

    #[cfg(unix)]
    fn verify(&self, label: &str) -> AppResult<()> {
        let metadata = fs::symlink_metadata(&self.path)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || !self.identity.matches(&metadata)
            || !self.identity.matches(&self.handle.metadata()?)
        {
            return Err(AppError::Message(format!(
                "{label} changed during operation: {}",
                self.path.display()
            )));
        }
        Ok(())
    }
}

fn read_regular(path: &Path, label: &str) -> AppResult<Option<RegularFile>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        return Err(AppError::Message(format!(
            "{label} is a symbolic link: {}",
            path.display()
        )));
    }
    if !metadata.is_file() {
        return Err(AppError::Message(format!(
            "{label} is not a regular file: {}",
            path.display()
        )));
    }
    let identity = EntryIdentity::capture(&metadata);
    let mut file = OpenOptions::new().read(true).open(path)?;
    if !identity.matches(&file.metadata()?) {
        return Err(AppError::Message(format!(
            "{label} changed while opening: {}",
            path.display()
        )));
    }
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    if !identity.matches(&file.metadata()?) {
        return Err(AppError::Message(format!(
            "{label} changed while reading: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    let mode = metadata.permissions().mode() & 0o7777;
    #[cfg(not(unix))]
    let mode = 0;
    Ok(Some(RegularFile {
        content,
        identity,
        mode,
    }))
}

fn ensure_directory(path: &Path, label: &str) -> AppResult<PinnedDirectory> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(path)?,
        Err(error) => return Err(error.into()),
    }
    PinnedDirectory::capture(path, label)
}

#[cfg(unix)]
fn destination_unchanged(
    path: &Path,
    existing: Option<&RegularFile>,
    label: &str,
) -> AppResult<()> {
    match (fs::symlink_metadata(path), existing) {
        (Err(error), None) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        (Ok(metadata), Some(existing))
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && existing.identity.matches(&metadata) =>
        {
            Ok(())
        }
        (Err(error), _) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Err(AppError::Message(format!(
            "{label} changed before publication: {}",
            path.display()
        ))),
    }
}

fn atomic_publish(
    path: &Path,
    content: &str,
    existing: Option<&RegularFile>,
    default_mode: u32,
    stem: &str,
) -> AppResult<()> {
    #[cfg(not(unix))]
    {
        let _ = (path, content, existing, default_mode, stem);
        Err(AppError::Message(
            "descriptor-relative guide and hook publication is unavailable on this platform"
                .to_owned(),
        ))
    }
    #[cfg(unix)]
    {
        atomic_publish_unix(path, content, existing, default_mode, stem)
    }
}

#[cfg(unix)]
fn atomic_publish_unix(
    path: &Path,
    content: &str,
    existing: Option<&RegularFile>,
    default_mode: u32,
    stem: &str,
) -> AppResult<()> {
    use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, statat, unlinkat};

    let parent = path
        .parent()
        .ok_or_else(|| AppError::Message(format!("{stem} destination has no parent")))?;
    let parent = PinnedDirectory::capture(parent, &format!("{stem} parent"))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::Message(format!("{stem} destination has no filename")))?;
    let mut temporary = None;
    for sequence in 0..32_u8 {
        let candidate = format!(
            ".{}.ptrack-{stem}-{}-{sequence}.tmp",
            file_name.to_string_lossy(),
            std::process::id()
        );
        match openat(
            &parent.handle,
            candidate.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(platform_raw_mode(
                existing.map_or(default_mode, |file| file.mode) & 0o7777,
            )),
        ) {
            Ok(descriptor) => {
                let mut file = fs::File::from(descriptor);
                let prepared = (|| -> AppResult<()> {
                    file.write_all(content.as_bytes())?;
                    file.set_permissions(fs::Permissions::from_mode(
                        existing.map_or(default_mode, |value| value.mode),
                    ))?;
                    file.sync_all()?;
                    Ok(())
                })();
                drop(file);
                if let Err(error) = prepared {
                    let _ = unlinkat(&parent.handle, candidate.as_str(), AtFlags::empty());
                    return Err(error);
                }
                temporary = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(from_errno(error)),
        }
    }
    let temporary = temporary
        .ok_or_else(|| AppError::Message(format!("could not allocate a {stem} temporary file")))?;
    if let Err(error) = parent
        .verify(&format!("{stem} parent"))
        .and_then(|()| destination_unchanged(path, existing, stem))
    {
        let _ = unlinkat(&parent.handle, temporary.as_str(), AtFlags::empty());
        return Err(error);
    }
    let unchanged = match (
        statat(&parent.handle, file_name, AtFlags::SYMLINK_NOFOLLOW),
        existing,
    ) {
        (Err(error), None) if error == rustix::io::Errno::NOENT => true,
        (Ok(stat), Some(existing)) => stat_identity_matches(&existing.identity, &stat),
        _ => false,
    };
    if !unchanged {
        let _ = unlinkat(&parent.handle, temporary.as_str(), AtFlags::empty());
        return Err(AppError::Message(format!(
            "{stem} changed before publication: {}",
            path.display()
        )));
    }
    if let Err(error) = renameat(
        &parent.handle,
        temporary.as_str(),
        &parent.handle,
        file_name,
    ) {
        let _ = unlinkat(&parent.handle, temporary.as_str(), AtFlags::empty());
        return Err(from_errno(error));
    }
    parent.handle.sync_all()?;
    Ok(())
}

fn remove_pinned(path: &Path, existing: &RegularFile) -> AppResult<()> {
    #[cfg(not(unix))]
    {
        let _ = (path, existing);
        Err(AppError::Message(
            "descriptor-relative hook removal is unavailable on this platform".to_owned(),
        ))
    }
    #[cfg(unix)]
    {
        remove_pinned_unix(path, existing)
    }
}

#[cfg(unix)]
fn remove_pinned_unix(path: &Path, existing: &RegularFile) -> AppResult<()> {
    use rustix::fs::{AtFlags, renameat, statat, unlinkat};

    let parent_path = path
        .parent()
        .ok_or_else(|| AppError::Message("hook destination has no parent".to_owned()))?;
    let parent = PinnedDirectory::capture(parent_path, "hook parent")?;
    destination_unchanged(path, Some(existing), "post-commit hook")?;
    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::Message("hook destination has no filename".to_owned()))?;
    let quarantine = format!(
        ".{}.ptrack-remove-{}.tmp",
        file_name.to_string_lossy(),
        std::process::id()
    );
    if statat(
        &parent.handle,
        quarantine.as_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .is_ok()
    {
        return Err(AppError::Message(
            "hook removal quarantine already exists".to_owned(),
        ));
    }
    renameat(
        &parent.handle,
        file_name,
        &parent.handle,
        quarantine.as_str(),
    )
    .map_err(from_errno)?;
    let removed = (|| -> AppResult<()> {
        let moved = statat(
            &parent.handle,
            quarantine.as_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(from_errno)?;
        if !stat_identity_matches(&existing.identity, &moved) {
            return Err(AppError::Message(
                "post-commit hook changed during removal".to_owned(),
            ));
        }
        unlinkat(&parent.handle, quarantine.as_str(), AtFlags::empty()).map_err(from_errno)?;
        Ok(())
    })();
    if let Err(error) = removed {
        let rollback = renameat(
            &parent.handle,
            quarantine.as_str(),
            &parent.handle,
            file_name,
        )
        .map_err(from_errno);
        let _ = parent.handle.sync_all();
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(AppError::Message(format!(
                "{error}; hook removal rollback failed: {rollback_error}"
            ))),
        };
    }
    parent.handle.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn stat_identity_matches(identity: &EntryIdentity, stat: &rustix::fs::Stat) -> bool {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let device_matches = stat.st_dev == identity.device;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let device_matches = u64::try_from(stat.st_dev).is_ok_and(|device| device == identity.device);
    device_matches && stat.st_ino == identity.inode
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const fn platform_raw_mode(mode: u32) -> u32 {
    mode
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn platform_raw_mode(mode: u32) -> u16 {
    u16::try_from(mode).expect("mode bits fit the platform raw mode")
}

fn hook_block() -> String {
    format!("{HOOK_BEGIN}\n{HOOK_BODY}\n{HOOK_END}\n")
}

fn upsert_hook(content: &str) -> (String, bool) {
    let block = hook_block();
    if let (Some(begin), Some(end)) = (content.find(HOOK_BEGIN), content.find(HOOK_END))
        && end > begin
    {
        let before = &content[..begin];
        let after = content[end + HOOK_END.len()..]
            .strip_prefix('\n')
            .unwrap_or(&content[end + HOOK_END.len()..]);
        let updated = format!("{before}{block}{after}");
        let changed = updated != content;
        return (updated, changed);
    }
    if content.trim().is_empty() {
        return (format!("#!/bin/sh\n{block}"), true);
    }
    (
        format!("{}\n\n{block}", content.trim_end_matches('\n')),
        true,
    )
}

fn strip_hook(content: &str) -> String {
    let (Some(begin), Some(end)) = (content.find(HOOK_BEGIN), content.find(HOOK_END)) else {
        return content.to_owned();
    };
    if end <= begin {
        return content.to_owned();
    }
    let before = content[..begin].trim_end_matches('\n');
    let after = content[end + HOOK_END.len()..]
        .strip_prefix('\n')
        .unwrap_or(&content[end + HOOK_END.len()..]);
    match (before.is_empty(), after.is_empty()) {
        (true, _) => after.to_owned(),
        (_, true) => format!("{before}\n"),
        _ => format!("{before}\n{after}"),
    }
}

/// A registered project's short display label: its directory name, falling
/// back to the whole path when it has none.
fn project_label(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| root.display().to_string(), str::to_owned)
}

/// Fail-closed target-open refusal: the store's own manifest/schema message,
/// plus the upgrade hint the spec requires when the target was written by a
/// newer build.
pub(crate) fn target_open_error(root: &Path, error: &ptrack_store::StoreError) -> AppError {
    let hint = if matches!(
        error,
        ptrack_store::StoreError::UnsupportedSchemaVersion { .. }
            | ptrack_store::StoreError::InvalidManifest(_)
    ) {
        "; upgrade ptrack for that project and try again"
    } else {
        ""
    };
    AppError::Message(format!(
        "cannot open target project {}: {error}{hint}",
        root.display()
    ))
}

#[allow(dead_code)]
fn _git_environment() -> Vec<(OsString, OsString)> {
    // Reserved for the bounded git adapter; keeping this service API free of
    // ambient remote/process authority is part of the capability cutover.
    Vec::new()
}
