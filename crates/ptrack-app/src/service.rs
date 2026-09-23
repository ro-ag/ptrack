use std::ffi::OsString;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use ptrack_agent::{AgentHandoffInbox, AgentObservationClient, AgentRunObservationV1, AgentRunsV2};
use ptrack_core::{
    CheckpointView, Commit, Issue, IssueStatus, Milestone, MilestoneStatus, Note, NoteTarget, Plan,
    PlanStatus, ProjectRef, ProjectSnapshot, Scratchpad, ScratchpadSnippet, Severity, Task,
    TaskStatus, Timestamp, Validate, check_summary, checkpoint, id_list, render_guide,
};
use ptrack_store::{
    ActiveBinding, ActorIdentity, Clock, GlobalStore, PinnedProjectDirectory, PlanDeleteSummary,
    ProjectStore, SystemClock,
};
use serde::{Deserialize, Serialize};

const NO_PROJECT: &str = "no ptrack project found (run 'ptrack init')";
const HOOK_BEGIN: &str = "# ptrack:begin";
const HOOK_END: &str = "# ptrack:end";
// `--flag=value` form, so a subject that starts with `-` is still a value.
const HOOK_BODY: &str = "command -v ptrack >/dev/null 2>&1 && ptrack commit record --sha=\"$(git rev-parse HEAD)\" --subject=\"$(git log -1 --pretty=%s)\" >/dev/null 2>&1 || true";

/// The exact conflict message every layer renders for a fenced scratchpad
/// write. Exported so no presentation layer keeps a copy that can drift.
pub const SCRATCHPAD_CONFLICT: &str = "scratchpad revision conflict";

#[derive(Debug)]
pub enum AppError {
    NoProject,
    NotImplemented(&'static str),
    Message(String),
    /// A scratchpad write stated a stale revision. It carries the stored
    /// record so the caller reloads in the same round trip instead of racing
    /// the writer that overtook it.
    ScratchpadConflict(Box<Scratchpad>),
    Io(std::io::Error),
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoProject => formatter.write_str(NO_PROJECT),
            Self::NotImplemented(feature) => write!(formatter, "{feature} is not implemented"),
            Self::Message(message) => formatter.write_str(message),
            Self::ScratchpadConflict(_) => formatter.write_str(SCRATCHPAD_CONFLICT),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl AppError {
    /// Renders this error the way the desktop bridge sends it.
    ///
    /// Almost every error is a bare message string, which is what a caller
    /// turns into an `Error`. An error that carries state a caller would
    /// otherwise have to fetch again becomes an object instead: `message` is
    /// the same text [`fmt::Display`] produces, and the extra fields sit
    /// beside it.
    #[must_use]
    pub fn to_bridge_value(&self) -> serde_json::Value {
        match self {
            Self::ScratchpadConflict(stored) => serde_json::json!({
                "message": self.to_string(),
                "stored": ScratchpadV1::from(stored.as_ref()),
            }),
            _ => serde_json::Value::String(self.to_string()),
        }
    }
}

/// The owned form of [`AppError::to_bridge_value`], so the desktop host can
/// map a failed call straight into what it sends back.
impl From<AppError> for serde_json::Value {
    fn from(error: AppError) -> Self {
        error.to_bridge_value()
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::NoProject
            | Self::NotImplemented(_)
            | Self::Message(_)
            | Self::ScratchpadConflict(_) => None,
        }
    }
}

/// One scratchpad snippet on the desktop bridge.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScratchpadSnippetV1 {
    pub id: u64,
    pub text: String,
    #[serde(default)]
    pub pinned: bool,
    /// Unix milliseconds; zero is the unset timestamp.
    #[serde(default)]
    pub created_at: i64,
}

/// The scratchpad on the desktop bridge.
///
/// `revision` and `updated_at` are informational on the way in: an accepted
/// write stamps both, so a caller cannot backdate a record or skip the fence
/// by claiming a revision inside the payload. The fence is the separate
/// `revision` argument of `SetScratchpadV1`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScratchpadV1 {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub snippets: Vec<ScratchpadSnippetV1>,
    #[serde(default)]
    pub revision: u64,
    /// Unix milliseconds; zero is the unset timestamp.
    #[serde(default)]
    pub updated_at: i64,
}

impl From<&Scratchpad> for ScratchpadV1 {
    fn from(value: &Scratchpad) -> Self {
        Self {
            text: value.text.clone(),
            snippets: value
                .snippets
                .iter()
                .map(|snippet| ScratchpadSnippetV1 {
                    id: snippet.id,
                    text: snippet.text.clone(),
                    pinned: snippet.pinned,
                    created_at: unix_milliseconds(snippet.created_at),
                })
                .collect(),
            revision: value.revision,
            updated_at: unix_milliseconds(value.updated_at),
        }
    }
}

impl ScratchpadV1 {
    /// Converts a bridge payload into the persisted record, leaving `revision`
    /// and `updated_at` at their defaults for the runtime to stamp.
    #[must_use]
    pub fn into_model(self) -> Scratchpad {
        Scratchpad {
            text: self.text,
            snippets: self
                .snippets
                .into_iter()
                .map(|snippet| ScratchpadSnippet {
                    id: snippet.id,
                    text: snippet.text,
                    pinned: snippet.pinned,
                    created_at: from_unix_milliseconds(snippet.created_at),
                })
                .collect(),
            revision: 0,
            updated_at: Timestamp::Zero,
        }
    }
}

/// Renders a timestamp as unix milliseconds, with the unset timestamp as zero.
///
/// The nanosecond form is a 128-bit value, so the millisecond result is
/// clamped rather than wrapped: an absurd stored instant reads as the extreme
/// one instead of silently changing sign. The split floors, matching
/// [`from_unix_milliseconds`], so the pair is an exact inverse on both sides
/// of the epoch instead of only after it.
fn unix_milliseconds(value: Timestamp) -> i64 {
    value.unix_nanoseconds().map_or(0, |nanoseconds| {
        let milliseconds = nanoseconds
            .div_euclid(1_000_000)
            .clamp(i128::from(i64::MIN), i128::from(i64::MAX));
        i64::try_from(milliseconds).unwrap_or_default()
    })
}

/// Reads unix milliseconds back, with zero as the unset timestamp. Splitting
/// with Euclidean division keeps a negative instant's nanoseconds in range.
fn from_unix_milliseconds(value: i64) -> Timestamp {
    if value == 0 {
        return Timestamp::Zero;
    }
    Timestamp::Fixed {
        seconds: value.div_euclid(1_000),
        nanoseconds: u32::try_from(value.rem_euclid(1_000)).unwrap_or_default() * 1_000_000,
        offset_seconds: 0,
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
    /// Marks a plan done in one store transaction whose open-task check reads
    /// what the status change commits against; `force` closes over open
    /// tasks and records them in an override note in the same write. Returns
    /// [`MutationResult::Notes`] holding that note, if any.
    CompletePlan {
        id: u64,
        force: bool,
    },
    /// Changes a plan's status and appends `notes` to it in one store
    /// transaction: the audit notes and the status land together or not at
    /// all. Returns [`MutationResult::Notes`].
    SetPlanStatusWithNotes {
        id: u64,
        status: PlanStatus,
        notes: Vec<String>,
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
    /// Changes a task's status and appends `notes` to it in one store
    /// transaction: the audit notes and the status land together or not at
    /// all. Returns [`MutationResult::Notes`].
    SetTaskStatusWithNotes {
        id: u64,
        status: TaskStatus,
        notes: Vec<String>,
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
    UpdateIssue {
        id: u64,
        expected_updated_at: Timestamp,
        title: String,
        body: String,
        severity: Severity,
        status: IssueStatus,
    },
    SetIssueTask {
        id: u64,
        expected_task_id: u64,
        task_id: u64,
    },
    MoveIssueTask {
        id: u64,
        expected_task_id: u64,
        expected_plan_id: u64,
        plan_id: u64,
    },
    ScheduleIssue {
        id: u64,
        plan_id: u64,
        task_title: String,
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
    ScheduledIssue {
        issue: Issue,
        task: Task,
    },
    Note(Note),
    /// The notes a status-with-notes mutation wrote, in the order given.
    Notes(Vec<Note>),
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
/// exact open task IDs in an override note that commits with the status.
///
/// # Errors
/// Returns an application error when the plan is absent or inaccessible,
/// open tasks remain without `force`, or the status-and-note write fails (in
/// which case neither landed).
pub fn complete_plan(
    application: &mut dyn ApplicationPort,
    plan_id: u64,
    force: bool,
) -> AppResult<CompletePlanResult> {
    let open_tasks = open_plan_tasks(application, plan_id)?;
    if !open_tasks.is_empty() && !force {
        return Err(AppError::Message(format!(
            "cannot close plan #{plan_id}: open tasks remain ({}); finish them or pass --force",
            id_list(&open_tasks)
        )));
    }
    // The store re-checks open tasks inside the closing transaction and
    // writes the override note there, so status and audit land together.
    let override_note =
        expect_notes_result(application.mutate(Mutation::CompletePlan { id: plan_id, force })?)?
            .pop();
    Ok(CompletePlanResult {
        plan_id,
        checkpoint: checkpoint(&application.snapshot()?, Some(plan_id)),
        override_note,
    })
}

fn open_plan_tasks(application: &mut dyn ApplicationPort, plan_id: u64) -> AppResult<Vec<u64>> {
    Ok(application
        .snapshot()?
        .tasks_for_plan(plan_id)
        .filter(|task| task.status.is_open())
        .map(|task| task.id)
        .collect())
}

/// The closing-evidence gaps for a task: a missing summary and no linked
/// commit, each as the sentence the refusal prints.
fn missing_evidence(
    application: &mut dyn ApplicationPort,
    task_id: u64,
    summary: Option<&str>,
) -> AppResult<(usize, Vec<&'static str>)> {
    let linked_commits = application
        .snapshot()?
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
    Ok((linked_commits, missing))
}

/// Completes one task while enforcing the agent workflow's evidence gate.
///
/// A nonblank summary and at least one linked commit are required unless
/// `force` is set. Forced omissions are recorded as an override note. The
/// closeout note, the override note, and the status change commit in one
/// transaction, so a refused or failed close leaves no orphan notes behind
/// and a retry never duplicates them.
///
/// # Errors
/// Returns an application error when evidence is missing, the task is absent or
/// inaccessible, or the status-and-notes write fails.
pub fn complete_task(
    application: &mut dyn ApplicationPort,
    task_id: u64,
    summary: Option<String>,
    force: bool,
) -> AppResult<CompleteTaskResult> {
    let summary = summary
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let (linked_commits, missing) = missing_evidence(application, task_id, summary.as_deref())?;
    if !missing.is_empty() && !force {
        return Err(AppError::Message(format!(
            "cannot close task #{task_id}: {} (or pass --force)",
            missing.join("; ")
        )));
    }
    let mut notes = Vec::new();
    if let Some(summary) = &summary {
        notes.push(format!("closeout: {summary}"));
    }
    if !missing.is_empty() {
        notes.push(format!(
            "override: closed via --force ({})",
            missing.join("; ")
        ));
    }
    let mut written =
        set_task_status_with_notes(application, task_id, TaskStatus::Done, notes)?.into_iter();
    let closeout_note = summary.and_then(|_| written.next());
    let override_note = written.next();
    Ok(CompleteTaskResult {
        task_id,
        linked_commits,
        closeout_note,
        override_note,
    })
}

/// Sets a task's status and writes `notes` on it atomically; see
/// [`Mutation::SetTaskStatusWithNotes`].
///
/// # Errors
/// Returns the store refusal (missing task, claim gate, write failure); on
/// error neither the status nor any note was written.
pub fn set_task_status_with_notes(
    application: &mut dyn ApplicationPort,
    id: u64,
    status: TaskStatus,
    notes: Vec<String>,
) -> AppResult<Vec<Note>> {
    expect_notes_result(application.mutate(Mutation::SetTaskStatusWithNotes {
        id,
        status,
        notes,
    })?)
}

/// Sets a plan's status and writes `notes` on it atomically; see
/// [`Mutation::SetPlanStatusWithNotes`].
///
/// # Errors
/// Returns the store refusal; on error neither the status nor any note was
/// written.
pub fn set_plan_status_with_notes(
    application: &mut dyn ApplicationPort,
    id: u64,
    status: PlanStatus,
    notes: Vec<String>,
) -> AppResult<Vec<Note>> {
    expect_notes_result(application.mutate(Mutation::SetPlanStatusWithNotes {
        id,
        status,
        notes,
    })?)
}

/// A human surface (TUI, desktop) that may close work without the agent
/// evidence gate. Humans are exempt from the gate, but every such close is
/// recorded, so the audit trail names where it happened.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiSurface {
    Tui,
    Desktop,
}

impl UiSurface {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tui => "TUI",
            Self::Desktop => "desktop",
        }
    }
}

/// Marks a task done from a human surface. Allowed without a closeout summary
/// or a linked commit; when either is missing an override note naming the
/// surface ("override: closed from TUI without evidence (…)") commits in the
/// same transaction as the status. Returns that note, if one was written.
///
/// # Errors
/// Returns the store refusal; on error nothing was written.
pub fn close_task_from_ui(
    application: &mut dyn ApplicationPort,
    surface: UiSurface,
    task_id: u64,
) -> AppResult<Option<Note>> {
    let notes = ui_close_override_notes(&application.snapshot()?, surface, task_id);
    Ok(set_task_status_with_notes(application, task_id, TaskStatus::Done, notes)?.pop())
}

/// The override note a human close of `task_id` from `surface` records: none
/// when the task has both a closeout summary and a linked commit, otherwise
/// one "override: closed from <surface> without evidence (…)" note naming what
/// is missing. Shared by every UI close so the audit text has one spelling.
#[must_use]
pub fn ui_close_override_notes(
    snapshot: &ProjectSnapshot,
    surface: UiSurface,
    task_id: u64,
) -> Vec<String> {
    let mut missing = Vec::new();
    if !snapshot.notes.iter().any(|note| {
        note.target == NoteTarget::Task
            && note.target_id == task_id
            && note.body.starts_with("closeout:")
    }) {
        missing.push("no closeout summary");
    }
    if !snapshot
        .commits
        .iter()
        .any(|commit| commit.task_id == task_id)
    {
        missing.push("no linked commit");
    }
    if missing.is_empty() {
        Vec::new()
    } else {
        vec![format!(
            "override: closed from {} without evidence ({})",
            surface.as_str(),
            missing.join("; ")
        )]
    }
}

/// Marks a plan done from a human surface. Allowed with open tasks; when any
/// remain an override note naming the surface and the open task IDs ("override:
/// plan completed from TUI with 2 open tasks #3, #4") commits in the same
/// transaction as the status. Returns that note, if one was written.
///
/// # Errors
/// Returns the store refusal; on error nothing was written.
pub fn complete_plan_from_ui(
    application: &mut dyn ApplicationPort,
    surface: UiSurface,
    plan_id: u64,
) -> AppResult<Option<Note>> {
    let open_tasks = open_plan_tasks(application, plan_id)?;
    let notes = if open_tasks.is_empty() {
        Vec::new()
    } else {
        vec![format!(
            "override: plan completed from {} with {} open task{} {}",
            surface.as_str(),
            open_tasks.len(),
            if open_tasks.len() == 1 { "" } else { "s" },
            id_list(&open_tasks)
        )]
    };
    Ok(set_plan_status_with_notes(application, plan_id, PlanStatus::Done, notes)?.pop())
}

fn expect_notes_result(result: MutationResult) -> AppResult<Vec<Note>> {
    if let MutationResult::Notes(notes) = result {
        Ok(notes)
    } else {
        Err(AppError::Message(
            "internal mutation result mismatch".to_owned(),
        ))
    }
}

/// Title prefix shared by both forms of the integration task.
const INTEGRATION_TASK_PREFIX: &str = "Integrate and verify against";

/// The title `ptrack plan add` gives the integration task it appends to a new
/// plan, so the creator and [`integration_task_id`] share one spelling.
#[must_use]
pub fn integration_task_title(goal: &str) -> String {
    if goal.is_empty() {
        format!("{INTEGRATION_TASK_PREFIX} the project goal")
    } else {
        format!("{INTEGRATION_TASK_PREFIX} goal: {goal}")
    }
}

/// The integration task `ptrack plan add` appended to `plan_id`, when it is
/// still there.
///
/// No record field marks it, so it is recognised by how it was created: `plan
/// add` writes it as the plan's first-born task (the lowest ID among the tasks
/// created no earlier than the plan; a task created before the plan was moved
/// in later) under the integration title. A title alone is not enough — a
/// person may name any task that way — and a renamed integration task is
/// simply ordinary work again.
#[must_use]
pub fn integration_task_id(snapshot: &ProjectSnapshot, plan_id: u64) -> Option<u64> {
    let plan_born = snapshot.plan(plan_id)?.created_at.unix_nanoseconds();
    let first_born = snapshot
        .tasks_for_plan(plan_id)
        .filter(
            |task| match (plan_born, task.created_at.unix_nanoseconds()) {
                (Some(plan), Some(task)) => task >= plan,
                _ => true,
            },
        )
        .min_by_key(|task| task.id)?;
    first_born
        .title
        .starts_with(INTEGRATION_TASK_PREFIX)
        .then_some(first_born.id)
}

/// `ptrack next` for every surface: the core selection, except that the
/// active plan's integration task waits until it is the only unheld work left.
///
/// `plan add` creates that task first, so by order it would be handed out
/// before any real work; the goal-anchoring spec makes it the plan's final
/// task. Once it is already started it is selected like any other task.
///
/// # Errors
/// Returns the core report error when the active plan pointer is dangling.
pub fn next_task(
    snapshot: &ProjectSnapshot,
) -> Result<ptrack_core::NextView, ptrack_core::ReportError> {
    let plan_id = snapshot.meta.active_plan;
    let Some(integration) = integration_task_id(snapshot, plan_id) else {
        return ptrack_core::next(snapshot);
    };
    let waiting = snapshot
        .task(integration)
        .is_some_and(|task| task.status == TaskStatus::Todo);
    let other_work = snapshot.tasks_for_plan(plan_id).any(|task| {
        task.id != integration
            && task.hold_reason.is_none()
            && matches!(task.status, TaskStatus::Todo | TaskStatus::Doing)
    });
    if !(waiting && other_work) {
        return ptrack_core::next(snapshot);
    }
    let mut deferred = snapshot.clone();
    deferred.tasks.retain(|task| task.id != integration);
    ptrack_core::next(&deferred)
}

/// Accepts only a 4–64 digit hexadecimal object name, the shape of every SHA
/// `git rev-parse` prints. Anything else — an option such as `--output=…`, a
/// revision expression, a path — could make a later `git show` do something
/// other than show one commit, so it is refused before it is stored.
///
/// # Errors
/// Returns a message naming the rejected value's problem.
pub fn check_commit_sha(sha: &str) -> AppResult<()> {
    if (4..=64).contains(&sha.len()) && sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::Message(format!(
            "invalid commit sha {sha:?}: want 4-64 hexadecimal digits"
        )))
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
    /// `warning` explains an adjustment the caller should know about, such as
    /// the block being placed before an existing hook's final `exec`.
    Installed {
        path: PathBuf,
        changed: bool,
        warning: Option<String>,
    },
    Removed,
    Missing,
    Status {
        path: PathBuf,
        installed: bool,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcessOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
}

/// The single use-case seam consumed by CLI, TUI, and Tauri adapters.
#[allow(clippy::missing_errors_doc)]
pub trait ApplicationPort {
    fn local_mode(&mut self, _action: &str) -> AppResult<String> {
        Err(AppError::Message(
            "local mode is unavailable in this context".to_owned(),
        ))
    }

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
    /// Returns the project scratchpad; a project that never wrote one reads as
    /// the empty scratchpad at revision zero.
    fn scratchpad(&mut self) -> AppResult<Scratchpad> {
        Err(unavailable())
    }
    /// Replaces the project scratchpad behind its optimistic revision fence,
    /// stamping `revision` and `updated_at` itself.
    fn set_scratchpad(
        &mut self,
        _expected_revision: u64,
        _value: Scratchpad,
    ) -> AppResult<Scratchpad> {
        Err(unavailable())
    }
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
}

fn unavailable() -> AppError {
    AppError::Message("active runtime binding is unavailable".to_owned())
}

fn no_coordination_host() -> AppError {
    AppError::Message("no active agent coordination host for this project".to_owned())
}

pub struct LocalApplication {
    bindings: WorkspaceBindings,
    local_metadata: Option<crate::local_mode::LocalMetadata>,
}

impl LocalApplication {
    #[must_use]
    pub const fn new(bindings: WorkspaceBindings) -> Self {
        Self {
            bindings,
            local_metadata: None,
        }
    }

    pub(crate) fn project_only(
        bindings: WorkspaceBindings,
        metadata: crate::local_mode::LocalMetadata,
    ) -> Self {
        Self {
            bindings,
            local_metadata: Some(metadata),
        }
    }

    fn require_global(&self) -> AppResult<()> {
        if self.local_metadata.is_some() {
            return Err(crate::local_mode::global_refusal());
        }
        Ok(())
    }

    fn actor(&self) -> AppResult<Option<ActorIdentity>> {
        if let Some(metadata) = &self.local_metadata {
            return Ok(metadata.actor());
        }
        self.with_global(crate::identity::load_identity)
    }

    fn project(&self) -> AppResult<&ProjectEndpoint> {
        self.bindings.project.as_ref().ok_or(AppError::NoProject)
    }

    fn agent_client(&self) -> AppResult<AgentObservationClient> {
        self.require_global()?;
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
        let actor = self.actor()?;
        let pinned = self
            .local_metadata
            .as_ref()
            .map(|_| crate::local_mode::pin(&endpoint.root))
            .transpose()?;
        let store = if let Some(pinned) = &pinned {
            ProjectStore::open_existing_pinned(
                pinned,
                &endpoint.binding,
                &self.bindings.writer_version,
            )?
        } else {
            ProjectStore::open_existing(
                &endpoint.database,
                &endpoint.binding,
                &self.bindings.writer_version,
            )?
        }
        .with_actor(actor);
        let result = operation(&store);
        drop(store);
        if result.is_ok() {
            self.register_project_best_effort(endpoint);
        }
        result
    }

    fn with_global<R>(&self, operation: impl FnOnce(&GlobalStore) -> AppResult<R>) -> AppResult<R> {
        self.require_global()?;
        let store = GlobalStore::open_existing(
            &self.bindings.global_database,
            &self.bindings.global_binding,
        )?;
        let result = operation(&store);
        drop(store);
        result
    }

    fn register_project_best_effort(&self, endpoint: &ProjectEndpoint) {
        if self.local_metadata.is_some() {
            return;
        }
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
        let actor = self.actor()?;
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
                store.delete_plan_for_move(&subtree).map_err(|error| {
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

    pub(crate) fn guide_extra(&self) -> AppResult<String> {
        if let Some(metadata) = &self.local_metadata {
            return Ok(metadata.guide().to_owned());
        }
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

    fn scratchpad(&mut self) -> AppResult<Scratchpad> {
        self.with_project(|store| Ok(store.scratchpad()?))
    }

    fn set_scratchpad(
        &mut self,
        expected_revision: u64,
        value: Scratchpad,
    ) -> AppResult<Scratchpad> {
        // Checked here so an over-limit note is refused by its own field path
        // before a database file is opened for writing.
        value
            .validate()
            .map_err(|error| AppError::Message(error.to_string()))?;
        let now = SystemClock.now_local();
        self.with_project(|store| {
            store
                .set_scratchpad(expected_revision, value, now)
                .map_err(|error| match error {
                    ptrack_store::StoreError::ScratchpadConflict { stored } => {
                        AppError::ScratchpadConflict(stored)
                    }
                    other => AppError::from(other),
                })
        })
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
                    check_summary(&value).map_err(AppError::Message)?;
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
                Mutation::CompletePlan { id, force } => MutationResult::Notes(
                    store
                        .complete_plan(id, force)?
                        .override_note
                        .into_iter()
                        .collect(),
                ),
                Mutation::SetPlanStatusWithNotes { id, status, notes } => {
                    // Status and notes commit in one claim-gated store transaction,
                    // so a refused change leaves no orphan notes.
                    MutationResult::Notes(store.set_plan_status_with_notes(id, status, &notes)?)
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
                Mutation::SetTaskStatusWithNotes { id, status, notes } => {
                    // Status and notes commit in one claim-gated store transaction,
                    // so a refused change leaves no orphan notes.
                    MutationResult::Notes(store.set_task_status_with_notes(id, status, &notes)?)
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
                Mutation::UpdateIssue {
                    id,
                    expected_updated_at,
                    title,
                    body,
                    severity,
                    status,
                } => MutationResult::Issue(store.update_issue(
                    id,
                    expected_updated_at,
                    title,
                    body,
                    severity,
                    status,
                )?),
                Mutation::SetIssueTask {
                    id,
                    expected_task_id,
                    task_id,
                } => MutationResult::Issue(store.set_issue_task(id, expected_task_id, task_id)?),
                Mutation::MoveIssueTask {
                    id,
                    expected_task_id,
                    expected_plan_id,
                    plan_id,
                } => MutationResult::Issue(store.move_issue_task(
                    id,
                    expected_task_id,
                    expected_plan_id,
                    plan_id,
                )?),
                Mutation::ScheduleIssue {
                    id,
                    plan_id,
                    task_title,
                } => {
                    let (issue, task) = store.schedule_issue(id, plan_id, task_title)?;
                    MutationResult::ScheduledIssue { issue, task }
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
                } => {
                    check_commit_sha(&sha)?;
                    MutationResult::Commit(store.add_commit(sha, subject, plan_id, task_id)?)
                }
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
        self.actor()
    }

    fn set_identity(&mut self, name: &str) -> AppResult<ActorIdentity> {
        self.with_global(|store| crate::identity::set_identity_name(store, name))
    }

    fn backup(&mut self) -> AppResult<PathBuf> {
        self.require_global()?;
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
        self.require_global()?;
        let root = self.verified_root()?;
        let hooks = effective_hooks_directory(&root)?;
        let path = hooks.directory.join("post-commit");
        match action {
            HookAction::Install => {
                hooks.require_project_local()?;
                ensure_directory(&hooks.directory, "hook directory")?;
                let existing = read_regular(&path, "post-commit hook")?;
                let (updated, changed, warning) = upsert_hook(
                    existing.as_ref().map_or("", |file| &file.content),
                )
                .map_err(|message| AppError::Message(format!("{}: {message}", path.display())))?;
                if changed {
                    atomic_publish(&path, &updated, existing.as_ref(), 0o755, "hook")?;
                }
                Ok(HookResult::Installed {
                    path,
                    changed,
                    warning,
                })
            }
            HookAction::Uninstall => {
                hooks.require_project_local()?;
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
        self.require_global()?;
        if reference.is_empty() || reference.starts_with('-') {
            return Err(AppError::Message(format!(
                "invalid commit reference {reference:?}"
            )));
        }
        let root = self.verified_root()?;
        // The runner policy of `ptrack-git` (no fsmonitor, scrubbed `GIT_*`
        // environment), no external diff or textconv driver, and
        // `--end-of-options` so even a hostile stored value stays a revision:
        // git never reads it as `--output=<file>` or any other option.
        let mut args = vec![OsString::from("show")];
        args.extend(ptrack_git::NO_EXTERNAL_DIFF_ARGS.map(OsString::from));
        if stat {
            args.push(OsString::from("--stat"));
        }
        args.push(OsString::from("--end-of-options"));
        args.push(OsString::from(reference));
        let mut command = ptrack_git::hardened_git_command(&root, &args);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let output = command.output()?;
        Ok(ProcessOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.status.code(),
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

/// Where git actually runs this repository's hooks from.
struct HooksDirectory {
    directory: PathBuf,
    root: PathBuf,
    git_directory: PathBuf,
}

impl HooksDirectory {
    /// Refuses to write a hook outside the project and its git directory: a
    /// shared `core.hooksPath` (say `~/.githooks`) would run the block for
    /// every repository on the machine, not just this project.
    fn require_project_local(&self) -> AppResult<()> {
        let directory = canonical_or_parent(&self.directory);
        if directory.starts_with(&self.root) || directory.starts_with(&self.git_directory) {
            return Ok(());
        }
        Err(AppError::Message(format!(
            "git runs hooks from {} (core.hooksPath), outside this project; add this line to that post-commit hook yourself:\n{HOOK_BODY}",
            self.directory.display()
        )))
    }
}

/// Resolves the hooks directory with `git rev-parse --git-path hooks`, which
/// honors `core.hooksPath` and linked worktrees, instead of assuming
/// `.git/hooks`.
fn effective_hooks_directory(root: &Path) -> AppResult<HooksDirectory> {
    let args = ["rev-parse", "--git-common-dir", "--git-path", "hooks"].map(OsString::from);
    let output = ptrack_git::hardened_git_command(root, &args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| AppError::Message(format!("cannot run git: {error}")))?;
    let stdout = String::from_utf8(output.stdout).unwrap_or_default();
    let mut lines = stdout.lines().filter(|line| !line.is_empty());
    let (Some(git_directory), Some(hooks), true) =
        (lines.next(), lines.next(), output.status.success())
    else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Message(format!(
            "{} is not a git repository ({}) — run 'git init' or install the hook manually",
            root.display(),
            stderr.trim()
        )));
    };
    Ok(HooksDirectory {
        directory: root.join(hooks),
        root: canonical_or_parent(root),
        git_directory: canonical_or_parent(&root.join(git_directory)),
    })
}

/// Canonicalizes `path`, or its parent plus its own name while it does not
/// exist yet, so a containment check still sees through symlinks.
fn canonical_or_parent(path: &Path) -> PathBuf {
    if let Ok(path) = fs::canonicalize(path) {
        return path;
    }
    match (path.parent().map(fs::canonicalize), path.file_name()) {
        (Some(Ok(parent)), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}

/// Shells whose syntax the ptrack block is written in.
const HOOK_SHELLS: [&str; 5] = ["sh", "bash", "zsh", "dash", "ksh"];

/// The interpreter a hook's shebang names, when it is not a POSIX-style
/// shell. A hook with no shebang is run by git through `sh`.
fn foreign_interpreter(content: &str) -> Option<String> {
    let line = content.lines().next()?.strip_prefix("#!")?;
    let mut words = line.split_whitespace();
    let mut program = words.next()?.rsplit('/').next().unwrap_or_default();
    if program == "env" {
        program = words
            .find(|word| !word.starts_with('-') && !word.contains('='))
            .map_or("", |word| word.rsplit('/').next().unwrap_or_default());
    }
    (!HOOK_SHELLS.contains(&program)).then(|| program.to_owned())
}

/// The byte offset of the hook's last command line when that line ends the
/// script before an appended block could run: `exec`, `exit`, or a sourced
/// script (which may itself `exit`, as husky's runner does).
fn terminal_line(content: &str) -> Option<(usize, &str)> {
    let mut offset = 0;
    let mut last = None;
    for line in content.split_inclusive('\n') {
        let command = line.trim();
        if !command.is_empty() && !command.starts_with('#') {
            last = Some((offset, command));
        }
        offset += line.len();
    }
    let (offset, command) = last?;
    let word = command.split_whitespace().next().unwrap_or_default();
    matches!(word, "exec" | "exit" | "." | "source").then_some((offset, command))
}

/// Inserts or refreshes the managed block. Returns the new text, whether it
/// changed, and a warning when the block had to go before the hook's final
/// command; refuses a hook written for another interpreter.
fn upsert_hook(content: &str) -> Result<(String, bool, Option<String>), String> {
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
        return Ok((updated, changed, None));
    }
    if content.trim().is_empty() {
        return Ok((format!("#!/bin/sh\n{block}"), true, None));
    }
    if let Some(interpreter) = foreign_interpreter(content) {
        return Err(format!(
            "the existing post-commit hook runs {interpreter}, not a POSIX shell; \
             leaving it untouched — have it run this shell command after each commit:\n{HOOK_BODY}"
        ));
    }
    if let Some((offset, command)) = terminal_line(content) {
        let updated = format!("{}{block}{}", &content[..offset], &content[offset..]);
        let warning = format!(
            "the existing post-commit hook ends with `{command}`, so the ptrack block was placed before that line"
        );
        return Ok((updated, true, Some(warning)));
    }
    Ok((
        format!("{}\n\n{block}", content.trim_end_matches('\n')),
        true,
        None,
    ))
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
