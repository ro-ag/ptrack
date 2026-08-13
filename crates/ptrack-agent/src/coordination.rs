use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    ActivityState, Association, Event, EventNotificationKind, HandoffPreview,
    IntelligenceConfidence, IntelligenceState, LeaseState, ProcessState, RegistrationKind,
    Registry, RegistryError, Run, RunState, Timestamp, build_handoff_preview,
};

const PROJECTION_LIMIT: usize = 64;
pub(crate) const CANDIDATE_LIMIT: usize = 1_024;
const NOTIFICATION_EVENTS_PER_RUN: usize = 32;
const INTELLIGENCE_EVENTS_PER_RUN: usize = 128;
const SUGGESTION_LIMIT: usize = 16;
const FILE_SUGGESTION_LIMIT: usize = 8;
const DECISION_SUGGESTION_LIMIT: usize = 4;
const ISSUE_SUGGESTION_LIMIT: usize = 4;
pub(crate) const SUGGESTION_TEXT_BYTES: usize = 320;
const HANDOFF_TTL_SECONDS: i64 = 30 * 60;
const WORKFLOW_TTL_SECONDS: i64 = 5 * 60;
pub(crate) const INBOX_LIMIT: usize = 64;
pub const AGENT_WORKFLOW_NOTICE: &str = "Proposal and approval only; no command runs and no Git, hosting, task, or capability state changes.";
const RAW_URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;
type Random = Arc<dyn Fn(&mut [u8]) -> Result<(), String> + Send + Sync>;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundedSnapshot {
    pub shown: usize,
    pub total: usize,
    pub more: usize,
}

impl BoundedSnapshot {
    #[must_use]
    pub const fn new(shown: usize, total: usize) -> Self {
        Self {
            shown,
            total,
            more: total.saturating_sub(shown),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAssociation {
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub plan_id: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub task_id: u64,
    pub revision: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoordinationSession {
    pub id: String,
    pub profile_kind: String,
    pub state: String,
    pub association: Option<Association>,
}

pub trait CoordinationStore: Send + Sync {
    /// Revalidates an association against the current project catalog.
    fn current_association(
        &self,
        live_id: &str,
        association: &Association,
    ) -> Option<RuntimeAssociation>;

    fn linked_commit_shas(&self) -> Vec<String> {
        Vec::new()
    }

    /// Returns linked commit identities while honoring a caller-owned deadline.
    ///
    /// # Errors
    /// Returns a deadline or durable-store error.
    fn linked_commit_shas_until(
        &self,
        deadline: Instant,
    ) -> Result<Vec<String>, CoordinationError> {
        let values = self.linked_commit_shas();
        ensure_projection_deadline(Some(deadline))?;
        Ok(values)
    }

    fn tracking_started_at(&self) -> Timestamp {
        Timestamp::ZERO
    }

    /// Returns the bounded title of a current plan for suggestion projection.
    ///
    /// # Errors
    /// Returns a content-free durable-store lookup error.
    fn plan_title(&self, _plan_id: u64) -> Result<Option<String>, CoordinationError> {
        Ok(None)
    }

    /// Returns current task-to-plan metadata for suggestion projection.
    ///
    /// # Errors
    /// Returns a content-free durable-store lookup error.
    fn task_context(
        &self,
        _task_id: u64,
    ) -> Result<Option<CoordinationTaskContext>, CoordinationError> {
        Ok(None)
    }

    /// Returns newest durable decisions, already bounded by `limit`.
    ///
    /// # Errors
    /// Returns a content-free durable-store lookup error.
    fn recent_decisions(
        &self,
        _limit: usize,
    ) -> Result<Vec<CoordinationDecision>, CoordinationError> {
        Ok(Vec::new())
    }

    /// Returns durable open-issue metadata for suggestion projection.
    ///
    /// # Errors
    /// Returns a content-free durable-store lookup error.
    fn open_issues(&self) -> Result<Vec<CoordinationIssue>, CoordinationError> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoordinationTaskContext {
    pub id: u64,
    pub plan_id: u64,
    pub title: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CoordinationTarget {
    #[default]
    Project,
    Plan,
    Task,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoordinationDecision {
    pub id: u64,
    pub target: CoordinationTarget,
    pub target_id: u64,
    pub body: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoordinationIssue {
    pub id: u64,
    pub title: String,
    pub task_id: u64,
}

pub trait CoordinationSessions: Send + Sync {
    fn snapshot(&self, limit: usize) -> (Vec<CoordinationSession>, usize);
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeIdentity {
    pub root: String,
    #[serde(skip_serializing)]
    pub git_dir: String,
    #[serde(skip_serializing)]
    pub common_git_dir: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub branch: String,
    pub head: String,
    pub linked: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranch {
    pub name: String,
    pub head: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GitCommit {
    pub sha: String,
    pub committed_at: Timestamp,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDivergence {
    pub upstream: String,
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExistingWorktree {
    pub root: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub branch: String,
    pub head: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorkflowStatus {
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicted: usize,
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // Exact host Git snapshot contract.
pub struct CoordinationGitSnapshot {
    pub root: String,
    pub git_dir: String,
    pub common_git_dir: String,
    pub branch: String,
    pub head: String,
    pub bare: bool,
    pub upstream: String,
    pub detached: bool,
    pub initial: bool,
    pub status: AgentWorkflowStatus,
    pub changed_paths: Vec<String>,
    pub untracked_paths: Vec<String>,
    pub changed_more: usize,
    pub untracked_more: usize,
    pub branches: Vec<GitBranch>,
    pub worktrees: Vec<ExistingWorktree>,
    pub worktree_bounds: BoundedSnapshot,
    pub worktrees_incomplete: bool,
    pub recent_commits: Vec<GitCommit>,
    pub unpushed_commits: Vec<GitCommit>,
    pub recent_commits_incomplete: bool,
    pub unpushed_commits_incomplete: bool,
    pub divergence: Option<GitDivergence>,
}

pub trait CoordinationGit: Send + Sync {
    /// Inspects an already-existing worktree without mutating it.
    ///
    /// # Errors
    /// Returns a content-free validation or inspection error.
    fn inspect_worktree(
        &self,
        project_root: &Path,
        root: &Path,
    ) -> Result<WorktreeIdentity, CoordinationError>;
    /// Captures a bounded, read-only repository projection.
    ///
    /// # Errors
    /// Returns a content-free snapshot error.
    fn snapshot(&self, root: &Path) -> Result<CoordinationGitSnapshot, CoordinationError>;
}

#[derive(Clone)]
pub struct CoordinationConfig {
    pub generation: u64,
    pub project_root: PathBuf,
    pub registry: Arc<Registry>,
    pub store: Arc<dyn CoordinationStore>,
    pub git: Arc<dyn CoordinationGit>,
    pub sessions: Arc<dyn CoordinationSessions>,
    pub now: Option<Clock>,
    pub random: Option<Random>,
    /// Host-owned monotonic mutation counter. Incrementing it executes no host
    /// code and remains exact even when the refresh channel is full.
    pub mutation_revision: Option<Arc<AtomicU64>>,
    /// Bounded unit notification. It carries no content or authority.
    pub runtime_changed: Option<SyncSender<()>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CoordinationError {
    NoWorkspace,
    StaleGeneration { expected: u64, active: u64 },
    Closed,
    RunNotFound,
    RegistryUnavailable,
    OwnershipRequiresTask,
    OwnershipInactive,
    OwnershipRevision,
    WorktreeInactive,
    WorktreeRevision,
    WorktreeCwd,
    WorktreeRoot,
    WorktreeChanged,
    HandoffSameRun,
    HandoffInactive,
    HandoffStale,
    HandoffFull,
    WorkflowKind,
    WorkflowTarget,
    WorkflowInactive,
    WorkflowStale,
    WorkflowApproved,
    WorkflowFull,
    DeadlineExceeded,
    Message(String),
}

impl fmt::Display for CoordinationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoWorkspace => formatter.write_str("no project workspace is open"),
            Self::StaleGeneration { expected, active } => write!(
                formatter,
                "stale workspace generation: expected {expected}, active {active}"
            ),
            Self::Closed => formatter.write_str("workspace is closing"),
            Self::RunNotFound => formatter.write_str("AgentRun not found"),
            Self::RegistryUnavailable => formatter.write_str("AgentRun registry is unavailable"),
            Self::OwnershipRequiresTask => {
                formatter.write_str("agent ownership requires a current task association")
            }
            Self::OwnershipInactive => {
                formatter.write_str("agent ownership requires an active run")
            }
            Self::OwnershipRevision => {
                formatter.write_str("agent ownership association revision changed")
            }
            Self::WorktreeInactive => {
                formatter.write_str("worktree association requires an active run")
            }
            Self::WorktreeRevision => formatter.write_str("agent association revision changed"),
            Self::WorktreeCwd => {
                formatter.write_str("agent working directory is outside the selected worktree")
            }
            Self::WorktreeRoot => formatter.write_str("an existing worktree root is required"),
            Self::WorktreeChanged => {
                formatter.write_str("agent changed while worktree identity was inspected")
            }
            Self::HandoffSameRun => {
                formatter.write_str("agent handoff requires distinct source and target runs")
            }
            Self::HandoffInactive => {
                formatter.write_str("agent handoff requires live source and target runs")
            }
            Self::HandoffStale => formatter.write_str("agent handoff is stale or invalid"),
            Self::HandoffFull => formatter.write_str("agent handoff inbox is full"),
            Self::WorkflowKind => formatter.write_str("unsupported agent workflow kind"),
            Self::WorkflowTarget => formatter.write_str("workflow target branch is unavailable"),
            Self::WorkflowInactive => formatter.write_str("workflow requires a live run"),
            Self::WorkflowStale => formatter.write_str("agent workflow is stale or invalid"),
            Self::WorkflowApproved => formatter.write_str("agent workflow was already approved"),
            Self::WorkflowFull => formatter.write_str("agent workflow inbox is full"),
            Self::DeadlineExceeded => formatter.write_str("context deadline exceeded"),
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for CoordinationError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRuntimeSummary {
    pub session_id: String,
    pub profile_kind: String,
    pub state: String,
    pub live: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub association: Option<RuntimeAssociation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntelligenceSummary {
    pub state: IntelligenceState,
    pub confidence: IntelligenceConfidence,
    pub evidence_count: usize,
    pub event_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_at: Option<Timestamp>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum AgentSuggestionKind {
    #[serde(rename = "context")]
    Context,
    #[serde(rename = "file")]
    File,
    #[serde(rename = "decision")]
    Decision,
    #[serde(rename = "issue")]
    Issue,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSuggestion {
    pub kind: AgentSuggestionKind,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub target_id: u64,
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    pub reason: String,
    pub evidence_event_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntelligenceEvidence {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub event_id: String,
    #[serde(skip_serializing_if = "is_unset_event_kind")]
    pub kind: crate::EventKind,
    #[serde(skip_serializing_if = "is_unset_event_phase")]
    pub phase: crate::EventPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<Timestamp>,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntelligenceDetail {
    pub state: IntelligenceState,
    pub confidence: IntelligenceConfidence,
    pub evidence: Vec<AgentIntelligenceEvidence>,
    pub event_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_at: Option<Timestamp>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIntelligenceV2 {
    pub generation: u64,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub association: Option<RuntimeAssociation>,
    pub intelligence: AgentIntelligenceDetail,
    pub event_bounds: BoundedSnapshot,
    pub suggestions: Vec<AgentSuggestion>,
    pub bounds: BoundedSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // Exact established GUI wire contract.
pub struct AgentRuntimeSummary {
    pub run_id: String,
    pub registration_kind: RegistrationKind,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub terminal_id: String,
    pub terminal_backed: bool,
    pub terminal_present: bool,
    pub corresponding_terminal: bool,
    pub state: RunState,
    pub process_state: ProcessState,
    pub lease_state: LeaseState,
    pub live: bool,
    pub activity_state: ActivityState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub association: Option<RuntimeAssociation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intelligence: Option<AgentIntelligenceSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunsV2 {
    pub generation: u64,
    pub runs: Vec<AgentRuntimeSummary>,
    pub bounds: BoundedSnapshot,
}

/// Complete bounded candidate projection used by app-owned aggregate views.
/// Unlike [`AgentRunsV2`], `runs` retains every candidate up to the hard
/// lifecycle limit so task-scoped projections cannot miss an associated run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRuntimeCandidatesV2 {
    pub generation: u64,
    pub runs: Vec<AgentRuntimeSummary>,
    pub bounds: BoundedSnapshot,
    pub sources_truncated: bool,
    pub analysis_incomplete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentWorkspaceSnapshotV2 {
    pub runtime: AgentRuntimeCandidatesV2,
    pub activity: AgentActivitySnapshot,
    pub drift: DriftSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskOwnership {
    pub plan_id: u64,
    pub task_id: u64,
    pub association_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOwnershipMutationV2 {
    pub generation: u64,
    pub run_id: String,
    pub owned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ownership: Option<AgentTaskOwnership>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorktreeAssociation {
    pub identity: WorktreeIdentity,
    pub verified: bool,
    pub isolated: bool,
    pub cwd_matches: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorktreeMutationV2 {
    pub generation: u64,
    pub run_id: String,
    pub associated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<AgentWorktreeAssociation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentNotification {
    pub id: String,
    pub run_id: String,
    pub kind: EventNotificationKind,
    pub observed_at: Timestamp,
    pub terminal_backed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub association: Option<RuntimeAssociation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // Exact established GUI wire contract.
pub struct AgentActivity {
    pub run_id: String,
    pub state: ActivityState,
    pub registration_kind: RegistrationKind,
    pub terminal_backed: bool,
    pub terminal_present: bool,
    pub corresponding_terminal: bool,
    pub live: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub association: Option<RuntimeAssociation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<IntelligenceConfidence>,
    pub evidence_count: usize,
    pub event_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ownership: Option<AgentTaskOwnership>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<AgentWorktreeAssociation>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivityCounts {
    pub running: usize,
    pub waiting: usize,
    pub blocked: usize,
    pub completed: usize,
    pub failed: usize,
    pub stale: usize,
    pub unknown: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActivityConflict {
    pub plan_id: u64,
    pub task_id: u64,
    pub agent_count: usize,
    pub owner_count: usize,
    pub run_ids: Vec<String>,
    pub bounds: BoundedSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHandoffEnvelopeV2 {
    pub id: String,
    pub generation: u64,
    pub source_run_id: String,
    pub target_run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_association: Option<RuntimeAssociation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_association: Option<RuntimeAssociation>,
    pub preview: HandoffPreview,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHandoffInbox {
    pub items: Vec<AgentHandoffEnvelopeV2>,
    pub bounds: BoundedSnapshot,
    pub incomplete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHandoffAcknowledgementV2 {
    pub generation: u64,
    pub id: String,
    pub removed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHandoffV2 {
    pub generation: u64,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub association: Option<RuntimeAssociation>,
    pub preview: HandoffPreview,
    pub event_bounds: BoundedSnapshot,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum AgentWorkflowKind {
    #[default]
    #[serde(rename = "")]
    Unset,
    #[serde(rename = "validation")]
    Validation,
    #[serde(rename = "commit")]
    Commit,
    #[serde(rename = "pullRequest")]
    PullRequest,
    #[serde(rename = "merge")]
    Merge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum AgentWorkflowState {
    #[serde(rename = "proposed")]
    Proposed,
    #[serde(rename = "approved")]
    Approved,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorkflowProposalV2 {
    pub id: String,
    pub generation: u64,
    pub kind: AgentWorkflowKind,
    pub state: AgentWorkflowState,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub association: Option<RuntimeAssociation>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub worktree_root: String,
    pub isolated: bool,
    pub branch: String,
    pub head: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub target_branch: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub target_head: String,
    pub status: AgentWorkflowStatus,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<Timestamp>,
    pub notice: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorkflowInbox {
    pub items: Vec<AgentWorkflowProposalV2>,
    pub bounds: BoundedSnapshot,
    pub incomplete: bool,
    pub notice: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorkflowDismissalV2 {
    pub generation: u64,
    pub id: String,
    pub removed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriftFinding {
    pub kind: String,
    pub severity: String,
    pub scope: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sha: String,
    pub run_ids: Vec<String>,
    pub plan_ids: Vec<u64>,
    pub task_ids: Vec<u64>,
    pub evidence_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriftSnapshot {
    pub state: String,
    pub findings: Vec<DriftFinding>,
    pub bounds: BoundedSnapshot,
    pub incomplete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // Exact established GUI wire contract.
pub struct AgentActivitySnapshot {
    pub state: String,
    pub items: Vec<AgentActivity>,
    pub counts: AgentActivityCounts,
    pub bounds: BoundedSnapshot,
    pub conflicts: Vec<AgentActivityConflict>,
    pub conflict_bounds: BoundedSnapshot,
    pub analysis_incomplete: bool,
    pub notifications: Vec<AgentNotification>,
    pub notification_bounds: BoundedSnapshot,
    pub notifications_incomplete: bool,
    pub handoffs: AgentHandoffInbox,
    pub worktrees: Vec<ExistingWorktree>,
    pub worktree_bounds: BoundedSnapshot,
    pub worktrees_incomplete: bool,
    pub workflows: AgentWorkflowInbox,
    pub workflow_targets: Vec<String>,
    pub workflow_targets_incomplete: bool,
}

#[derive(Clone)]
pub(crate) struct OwnershipClaim {
    generation: u64,
    run_id: String,
    plan_id: u64,
    task_id: u64,
    association_revision: u64,
    lifecycle_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorktreeClaim {
    generation: u64,
    run_id: String,
    lifecycle_revision: u64,
    association: Option<RuntimeAssociation>,
    identity: WorktreeIdentity,
    isolated: bool,
}

#[derive(Clone)]
struct HandoffRecord {
    projected: AgentHandoffEnvelopeV2,
    source_lifecycle_revision: u64,
    target_lifecycle_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorkflowBinding {
    root: String,
    git_dir: String,
    common_git_dir: String,
    branch: String,
    head: String,
    target_branch: String,
    target_head: String,
    status: AgentWorkflowStatus,
    digest: String,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct WorkflowDigestInput<'a> {
    root: &'a str,
    git_dir: &'a str,
    common_git_dir: &'a str,
    branch: &'a str,
    head: &'a str,
    upstream: &'a str,
    target_branch: &'a str,
    target_head: &'a str,
    detached: bool,
    initial: bool,
    status: &'a AgentWorkflowStatus,
    changed: &'a [String],
    untracked: &'a [String],
    changed_more: usize,
    untracked_more: usize,
    divergence: &'a Option<GitDivergence>,
}

#[derive(Clone)]
struct WorkflowRecord {
    projected: AgentWorkflowProposalV2,
    lifecycle_revision: u64,
    worktree: Option<WorktreeIdentity>,
    worktree_revision: u64,
    binding: WorkflowBinding,
}

struct WorkflowCapture {
    run: Run,
    association: Option<RuntimeAssociation>,
    worktree: Option<WorktreeIdentity>,
    worktree_revision: u64,
    binding: WorkflowBinding,
}

#[derive(Clone)]
struct WorktreeEpoch {
    revision: u64,
    claim: Option<WorktreeClaim>,
}

struct CoordinationState {
    closed: bool,
    ownership: BTreeMap<String, OwnershipClaim>,
    worktrees: BTreeMap<String, WorktreeClaim>,
    worktree_revisions: BTreeMap<String, u64>,
    handoffs: BTreeMap<String, HandoffRecord>,
    workflows: BTreeMap<String, WorkflowRecord>,
}

pub struct Coordinator {
    generation: u64,
    project_root: PathBuf,
    registry: Arc<Registry>,
    store: Arc<dyn CoordinationStore>,
    git: Arc<dyn CoordinationGit>,
    sessions: Arc<dyn CoordinationSessions>,
    now: Clock,
    random: Random,
    mutation_revision: Option<Arc<AtomicU64>>,
    runtime_changed: Option<SyncSender<()>>,
    state: Mutex<CoordinationState>,
    #[cfg(test)]
    preview_barrier: Mutex<Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>>,
}

impl Coordinator {
    #[must_use]
    pub fn new(config: CoordinationConfig) -> Self {
        Self {
            generation: config.generation,
            project_root: config.project_root,
            registry: config.registry,
            store: config.store,
            git: config.git,
            sessions: config.sessions,
            now: config.now.unwrap_or_else(|| Arc::new(Timestamp::now_utc)),
            random: config.random.unwrap_or_else(|| {
                Arc::new(|bytes| getrandom::fill(bytes).map_err(|error| error.to_string()))
            }),
            mutation_revision: config.mutation_revision,
            runtime_changed: config.runtime_changed,
            state: Mutex::new(CoordinationState {
                closed: false,
                ownership: BTreeMap::new(),
                worktrees: BTreeMap::new(),
                worktree_revisions: BTreeMap::new(),
                handoffs: BTreeMap::new(),
                workflows: BTreeMap::new(),
            }),
            #[cfg(test)]
            preview_barrier: Mutex::new(None),
        }
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Clears all generation-scoped, memory-only coordination state.
    pub fn shutdown(&self) {
        let mut state = lock(&self.state);
        state.closed = true;
        state.ownership.clear();
        state.worktrees.clear();
        state.worktree_revisions.clear();
        state.handoffs.clear();
        state.workflows.clear();
    }

    /// Returns content-free run projection rows.
    ///
    /// # Errors
    /// Returns generation, shutdown, registry, or evidence errors.
    pub fn agent_runs(&self, generation: u64) -> Result<AgentRunsV2, CoordinationError> {
        self.check_generation(generation)?;
        let projection = self.projection()?;
        Ok(AgentRunsV2 {
            generation: self.generation,
            runs: projection.agents,
            bounds: projection.agent_bounds,
        })
    }

    /// Returns the complete, sanitized runtime candidate projection.
    ///
    /// # Errors
    /// Returns generation, shutdown, registry, session, or evidence errors.
    pub fn agent_runtime_candidates(
        &self,
        generation: u64,
    ) -> Result<AgentRuntimeCandidatesV2, CoordinationError> {
        self.check_generation(generation)?;
        let projection = self.projection()?;
        Ok(AgentRuntimeCandidatesV2 {
            generation: self.generation,
            runs: projection.agent_candidates,
            bounds: projection.agent_bounds,
            sources_truncated: projection.sources_truncated,
            analysis_incomplete: projection.incomplete,
        })
    }

    /// Builds all aggregate workspace sections from one runtime and Git epoch.
    ///
    /// # Errors
    /// Returns generation, shutdown, registry, or durable-store errors.
    pub fn workspace_snapshot(
        &self,
        generation: u64,
        git: &CoordinationGitSnapshot,
        deadline: Instant,
    ) -> Result<AgentWorkspaceSnapshotV2, CoordinationError> {
        self.check_generation(generation)?;
        ensure_projection_deadline(Some(deadline))?;
        let projection = self.projection_until(Some(deadline))?;
        let activity = self.activity_from_projection_git_until(&projection, git, Some(deadline))?;
        ensure_projection_deadline(Some(deadline))?;
        let linked_commits = self.store.linked_commit_shas_until(deadline)?;
        ensure_projection_deadline(Some(deadline))?;
        let drift = build_drift(
            &projection,
            &activity,
            git,
            &linked_commits,
            self.store.tracking_started_at(),
        );
        ensure_projection_deadline(Some(deadline))?;
        Ok(AgentWorkspaceSnapshotV2 {
            runtime: AgentRuntimeCandidatesV2 {
                generation: self.generation,
                runs: projection.agent_candidates.clone(),
                bounds: projection.agent_bounds,
                sources_truncated: projection.sources_truncated,
                analysis_incomplete: projection.incomplete,
            },
            activity,
            drift,
        })
    }

    /// Builds a standalone authority-free handoff preview from at most 128 events.
    ///
    /// # Errors
    /// Returns generation, shutdown, run lookup, registry, or privacy errors.
    pub fn preview_handoff(
        &self,
        generation: u64,
        run_id: &str,
    ) -> Result<AgentHandoffV2, CoordinationError> {
        self.check_generation(generation)?;
        let (mut run, events, total, _) = self
            .registry
            .intelligence_snapshot(run_id, INTELLIGENCE_EVENTS_PER_RUN)
            .map_err(registry_coordination_error)?;
        let association = current_association(self.store.as_ref(), &run);
        if association.is_none() {
            run.association = None;
        }
        let shown = events.len();
        Ok(AgentHandoffV2 {
            generation: self.generation,
            run_id: run.id.clone(),
            association,
            preview: build_handoff_preview(&run, &events),
            event_bounds: BoundedSnapshot::new(shown, total),
        })
    }

    /// Returns bounded, current-association intelligence and durable suggestions.
    ///
    /// # Errors
    /// Returns generation, shutdown, run, registry, or durable-store lookup errors.
    pub fn agent_intelligence(
        &self,
        generation: u64,
        run_id: &str,
    ) -> Result<AgentIntelligenceV2, CoordinationError> {
        self.check_generation(generation)?;
        let (mut run, events, total, _) = self
            .registry
            .intelligence_snapshot(run_id, INTELLIGENCE_EVENTS_PER_RUN)
            .map_err(registry_coordination_error)?;
        let association = current_association(self.store.as_ref(), &run);
        if association.is_none() {
            run.association = None;
        }
        let events = events
            .into_iter()
            .filter(|event| notification_current(self.generation, &run, association, event))
            .collect::<Vec<_>>();
        let intelligence = crate::derive_run_intelligence(&run, &events);
        let suggestions = build_suggestions(self.store.as_ref(), association, &events)?;
        let suggestion_total = suggestions.len();
        let suggestions = suggestions.into_iter().take(SUGGESTION_LIMIT).collect();
        Ok(AgentIntelligenceV2 {
            generation: self.generation,
            run_id: run.id,
            association,
            intelligence: AgentIntelligenceDetail {
                state: intelligence.state,
                confidence: intelligence.confidence,
                evidence: intelligence
                    .evidence
                    .into_iter()
                    .map(|value| AgentIntelligenceEvidence {
                        event_id: value.event_id,
                        kind: value.kind,
                        phase: value.phase,
                        observed_at: (!value.observed_at.is_zero()).then_some(value.observed_at),
                        reason: value.reason,
                    })
                    .collect(),
                event_count: intelligence.event_count,
                last_event_at: (!intelligence.last_event_at.is_zero())
                    .then_some(intelligence.last_event_at),
            },
            event_bounds: BoundedSnapshot::new(events.len(), total),
            suggestions,
            bounds: BoundedSnapshot::new(suggestion_total.min(SUGGESTION_LIMIT), suggestion_total),
        })
    }

    /// Builds the bounded activity/notification/inbox projection.
    ///
    /// # Errors
    /// Returns generation, shutdown, registry, store, or Git errors.
    pub fn activity(&self, generation: u64) -> Result<AgentActivitySnapshot, CoordinationError> {
        self.check_generation(generation)?;
        let projection = self.projection()?;
        self.activity_from_projection(&projection)
    }

    /// Builds bounded checkout, commit, agent, and cross-task drift findings.
    ///
    /// # Errors
    /// Returns generation, shutdown, registry, store, or Git errors.
    pub fn drift(&self, generation: u64) -> Result<DriftSnapshot, CoordinationError> {
        self.check_generation(generation)?;
        let projection = self.projection()?;
        let git = self.git.snapshot(&self.project_root)?;
        let activity = self.activity_from_projection_git(&projection, &git)?;
        Ok(build_drift(
            &projection,
            &activity,
            &git,
            &self.store.linked_commit_shas(),
            self.store.tracking_started_at(),
        ))
    }

    /// Records or releases descriptive task ownership.
    ///
    /// # Errors
    /// Returns exact generation, run, lifecycle, association, or registry errors.
    pub fn set_task_ownership(
        &self,
        generation: u64,
        run_id: &str,
        expected_association_revision: u64,
        owned: bool,
    ) -> Result<AgentOwnershipMutationV2, CoordinationError> {
        self.check_generation(generation)?;
        if run_id.is_empty() {
            return Err(CoordinationError::RunNotFound);
        }
        if expected_association_revision == 0 {
            return Err(CoordinationError::OwnershipRevision);
        }
        let (run, association) = self.exact_run(run_id)?;
        if !run_is_live(&run) {
            return Err(CoordinationError::OwnershipInactive);
        }
        let association = association.ok_or(CoordinationError::OwnershipRequiresTask)?;
        if association.task_id == 0 {
            return Err(CoordinationError::OwnershipRequiresTask);
        }
        if association.revision != expected_association_revision {
            return Err(CoordinationError::OwnershipRevision);
        }
        let claim = OwnershipClaim {
            generation: self.generation,
            run_id: run.id.clone(),
            plan_id: association.plan_id,
            task_id: association.task_id,
            association_revision: association.revision,
            lifecycle_revision: run.lifecycle_revision,
        };
        let changed = {
            let mut state = lock(&self.state);
            if owned {
                state.ownership.insert(run_id.to_owned(), claim);
                true
            } else {
                if let Some(existing) = state.ownership.get(run_id)
                    && !ownership_claim_equal(existing, &claim)
                {
                    return Err(CoordinationError::OwnershipRevision);
                }
                state.ownership.remove(run_id).is_some()
            }
        };
        if changed {
            self.notify();
        }
        Ok(AgentOwnershipMutationV2 {
            generation: self.generation,
            run_id: run_id.to_owned(),
            owned,
            ownership: owned.then_some(AgentTaskOwnership {
                plan_id: association.plan_id,
                task_id: association.task_id,
                association_revision: association.revision,
            }),
        })
    }

    /// Records or releases a verified existing-worktree claim.
    ///
    /// # Errors
    /// Returns exact generation, run, association, path, Git, or race errors.
    pub fn set_worktree(
        &self,
        generation: u64,
        run_id: &str,
        expected_association_revision: u64,
        root: &str,
        associated: bool,
    ) -> Result<AgentWorktreeMutationV2, CoordinationError> {
        self.check_generation(generation)?;
        let (before, association) =
            self.exact_worktree_run(run_id, expected_association_revision)?;
        if !associated {
            let changed = {
                let mut state = lock(&self.state);
                if let Some(existing) = state.worktrees.get(run_id)
                    && !worktree_claim_current(self.generation, existing, &before, association)
                {
                    return Err(CoordinationError::WorktreeRevision);
                }
                let changed = state.worktrees.remove(run_id).is_some();
                if changed {
                    bump_worktree_revision(&mut state.worktree_revisions, run_id);
                }
                changed
            };
            if changed {
                self.notify();
            }
            return Ok(AgentWorktreeMutationV2 {
                generation: self.generation,
                run_id: run_id.to_owned(),
                associated: false,
                worktree: None,
            });
        }
        if root.is_empty() || root.trim() != root || root.len() > 4_096 {
            return Err(CoordinationError::WorktreeRoot);
        }
        let identity = self
            .git
            .inspect_worktree(&self.project_root, Path::new(root))?;
        if !path_inside(Path::new(&identity.root), Path::new(&before.cwd)) {
            return Err(CoordinationError::WorktreeCwd);
        }
        let (after, after_association) =
            self.exact_worktree_run(run_id, expected_association_revision)?;
        if before.lifecycle_revision != after.lifecycle_revision
            || clean_path(&before.cwd) != clean_path(&after.cwd)
            || association != after_association
        {
            return Err(CoordinationError::WorktreeChanged);
        }
        let isolated = clean_path(&identity.root) != clean_path(&self.project_root);
        let projected = AgentWorktreeAssociation {
            identity: identity.clone(),
            verified: true,
            isolated,
            cwd_matches: true,
        };
        {
            let mut state = lock(&self.state);
            bump_worktree_revision(&mut state.worktree_revisions, run_id);
            state.worktrees.insert(
                run_id.to_owned(),
                WorktreeClaim {
                    generation: self.generation,
                    run_id: run_id.to_owned(),
                    lifecycle_revision: after.lifecycle_revision,
                    association,
                    identity,
                    isolated,
                },
            );
        }
        self.notify();
        Ok(AgentWorktreeMutationV2 {
            generation: self.generation,
            run_id: run_id.to_owned(),
            associated: true,
            worktree: Some(projected),
        })
    }

    /// Creates an immutable, bounded, authority-free handoff envelope.
    ///
    /// # Errors
    /// Returns exact generation, live-run, association, capacity, or race errors.
    pub fn send_handoff(
        &self,
        generation: u64,
        source_run_id: &str,
        target_run_id: &str,
        expected_source_revision: u64,
        expected_target_revision: u64,
    ) -> Result<AgentHandoffEnvelopeV2, CoordinationError> {
        self.check_generation(generation)?;
        if source_run_id.is_empty() || target_run_id.is_empty() || source_run_id == target_run_id {
            return Err(CoordinationError::HandoffSameRun);
        }
        let (source, target, source_association, target_association) =
            self.exact_pair(source_run_id, target_run_id)?;
        if association_revision(source_association) != expected_source_revision
            || association_revision(target_association) != expected_target_revision
        {
            return Err(CoordinationError::HandoffStale);
        }
        let (preview_run, events, _, _) = self
            .registry
            .intelligence_snapshot(source_run_id, INTELLIGENCE_EVENTS_PER_RUN)
            .map_err(registry_coordination_error)?;
        if !same_run_epoch(&source, &preview_run) {
            return Err(CoordinationError::HandoffStale);
        }
        let preview = build_handoff_preview(&preview_run, &events);
        #[cfg(test)]
        if let Some((started, release)) = lock(&self.preview_barrier).take() {
            started.wait();
            release.wait();
        }
        let revalidated = self.exact_pair(source_run_id, target_run_id);
        let Ok((
            current_source,
            current_target,
            current_source_association,
            current_target_association,
        )) = revalidated
        else {
            return Err(CoordinationError::HandoffStale);
        };
        if source.lifecycle_revision != current_source.lifecycle_revision
            || target.lifecycle_revision != current_target.lifecycle_revision
            || source_association != current_source_association
            || target_association != current_target_association
        {
            return Err(CoordinationError::HandoffStale);
        }
        let id = self.random_token()?;
        let now = (self.now)();
        let projected = AgentHandoffEnvelopeV2 {
            id: id.clone(),
            generation: self.generation,
            source_run_id: source_run_id.to_owned(),
            target_run_id: target_run_id.to_owned(),
            source_association,
            target_association,
            preview,
            created_at: now,
            expires_at: now.add_seconds(HANDOFF_TTL_SECONDS),
        };
        {
            let mut state = lock(&self.state);
            prune_handoffs(&mut state.handoffs, now);
            if state.handoffs.len() >= INBOX_LIMIT {
                return Err(CoordinationError::HandoffFull);
            }
            state.handoffs.insert(
                id,
                HandoffRecord {
                    projected: projected.clone(),
                    source_lifecycle_revision: source.lifecycle_revision,
                    target_lifecycle_revision: target.lifecycle_revision,
                },
            );
        }
        self.notify();
        Ok(projected)
    }

    /// One-time acknowledges a handoff for its exact target after revalidation.
    ///
    /// # Errors
    /// Returns exact generation, target, lifecycle, association, expiry, or race errors.
    pub fn acknowledge_handoff(
        &self,
        generation: u64,
        id: &str,
        target_run_id: &str,
    ) -> Result<AgentHandoffAcknowledgementV2, CoordinationError> {
        self.check_generation(generation)?;
        let now = (self.now)();
        let record = {
            let mut state = lock(&self.state);
            prune_handoffs(&mut state.handoffs, now);
            state.handoffs.get(id).cloned()
        }
        .filter(|record| {
            record.projected.generation == self.generation
                && record.projected.target_run_id == target_run_id
        })
        .ok_or(CoordinationError::HandoffStale)?;
        let revalidated = self.exact_pair(
            &record.projected.source_run_id,
            &record.projected.target_run_id,
        );
        let Ok((source, target, source_association, target_association)) = revalidated else {
            lock(&self.state).handoffs.remove(id);
            return Err(CoordinationError::HandoffStale);
        };
        if source.lifecycle_revision != record.source_lifecycle_revision
            || target.lifecycle_revision != record.target_lifecycle_revision
            || source_association != record.projected.source_association
            || target_association != record.projected.target_association
        {
            lock(&self.state).handoffs.remove(id);
            return Err(CoordinationError::HandoffStale);
        }
        if lock(&self.state).handoffs.remove(id).is_none() {
            return Err(CoordinationError::HandoffStale);
        }
        self.notify();
        Ok(AgentHandoffAcknowledgementV2 {
            generation: self.generation,
            id: id.to_owned(),
            removed: true,
        })
    }

    /// Captures an exact proposal for a possible future host workflow.
    ///
    /// # Errors
    /// Returns generation, kind, run, association, worktree, Git, target, capacity, or race errors.
    pub fn prepare_workflow(
        &self,
        generation: u64,
        run_id: &str,
        expected_association_revision: u64,
        kind: AgentWorkflowKind,
        target_branch: &str,
    ) -> Result<AgentWorkflowProposalV2, CoordinationError> {
        self.check_generation(generation)?;
        if kind == AgentWorkflowKind::Unset {
            return Err(CoordinationError::WorkflowKind);
        }
        let capture =
            self.capture_workflow(run_id, expected_association_revision, kind, target_branch)?;
        let id = self.random_token()?;
        let now = (self.now)();
        let projected = project_workflow(
            &id,
            self.generation,
            kind,
            AgentWorkflowState::Proposed,
            run_id,
            capture.association,
            capture.worktree.as_ref(),
            &self.project_root,
            &capture.binding,
            now,
            now.add_seconds(WORKFLOW_TTL_SECONDS),
            None,
        );
        self.with_exact_snapshot(|runs| {
            self.validate_workflow_capture(&capture, runs)?;
            let mut state = lock(&self.state);
            if state.closed
                || !worktree_epoch_matches(
                    &state,
                    &capture.run.id,
                    capture.worktree_revision,
                    capture.worktree.as_ref(),
                )
            {
                return Err(CoordinationError::WorkflowStale);
            }
            prune_workflows(&mut state.workflows, now);
            if state.workflows.len() >= INBOX_LIMIT {
                return Err(CoordinationError::WorkflowFull);
            }
            state.workflows.insert(
                id,
                WorkflowRecord {
                    projected: projected.clone(),
                    lifecycle_revision: capture.run.lifecycle_revision,
                    worktree: capture.worktree.clone(),
                    worktree_revision: capture.worktree_revision,
                    binding: capture.binding.clone(),
                },
            );
            Ok(())
        })?;
        self.notify();
        Ok(projected)
    }

    /// Marks one proposal approved after complete revalidation. It executes nothing.
    ///
    /// # Errors
    /// Returns generation, expiry, prior approval, lifecycle, worktree, Git, or race errors.
    pub fn approve_workflow(
        &self,
        generation: u64,
        id: &str,
    ) -> Result<AgentWorkflowProposalV2, CoordinationError> {
        self.check_generation(generation)?;
        let now = (self.now)();
        let record = {
            let mut state = lock(&self.state);
            prune_workflows(&mut state.workflows, now);
            state.workflows.get(id).cloned()
        }
        .filter(|record| record.projected.generation == self.generation)
        .ok_or(CoordinationError::WorkflowStale)?;
        if record.projected.state == AgentWorkflowState::Approved {
            return Err(CoordinationError::WorkflowApproved);
        }
        let Ok(capture) = self.capture_workflow(
            &record.projected.run_id,
            association_revision(record.projected.association),
            record.projected.kind,
            &record.projected.target_branch,
        ) else {
            return Err(self.consume_failed_workflow_approval(id, &record));
        };
        if capture.run.lifecycle_revision != record.lifecycle_revision
            || capture.association != record.projected.association
            || capture.worktree != record.worktree
            || capture.worktree_revision != record.worktree_revision
            || capture.binding != record.binding
        {
            return Err(self.consume_failed_workflow_approval(id, &record));
        }
        let approved = self.with_exact_snapshot(|runs| {
            self.validate_workflow_capture(&capture, runs)?;
            let mut state = lock(&self.state);
            if state.closed
                || !worktree_epoch_matches(
                    &state,
                    &capture.run.id,
                    capture.worktree_revision,
                    capture.worktree.as_ref(),
                )
            {
                state.workflows.remove(id);
                return Err(CoordinationError::WorkflowStale);
            }
            prune_workflows(&mut state.workflows, (self.now)());
            let current = state
                .workflows
                .get_mut(id)
                .ok_or(CoordinationError::WorkflowStale)?;
            if current.lifecycle_revision != record.lifecycle_revision
                || current.worktree_revision != record.worktree_revision
                || current.binding != record.binding
            {
                state.workflows.remove(id);
                return Err(CoordinationError::WorkflowStale);
            }
            if current.projected.state == AgentWorkflowState::Approved {
                return Err(CoordinationError::WorkflowApproved);
            }
            current.projected.state = AgentWorkflowState::Approved;
            current.projected.approved_at = Some(now);
            Ok(current.projected.clone())
        });
        let approved = match approved {
            Ok(approved) => approved,
            Err(CoordinationError::WorkflowApproved) => {
                return Err(CoordinationError::WorkflowApproved);
            }
            Err(_) => return Err(self.consume_failed_workflow_approval(id, &record)),
        };
        self.notify();
        Ok(approved)
    }

    /// Removes one workflow proposal without executing it.
    ///
    /// # Errors
    /// Returns generation, shutdown, expiry, or unknown-id errors.
    pub fn dismiss_workflow(
        &self,
        generation: u64,
        id: &str,
    ) -> Result<AgentWorkflowDismissalV2, CoordinationError> {
        self.check_generation(generation)?;
        let now = (self.now)();
        let removed = {
            let mut state = lock(&self.state);
            prune_workflows(&mut state.workflows, now);
            state.workflows.remove(id).is_some()
        };
        if !removed {
            return Err(CoordinationError::WorkflowStale);
        }
        self.notify();
        Ok(AgentWorkflowDismissalV2 {
            generation: self.generation,
            id: id.to_owned(),
            removed: true,
        })
    }

    fn check_generation(&self, generation: u64) -> Result<(), CoordinationError> {
        if lock(&self.state).closed {
            return Err(CoordinationError::Closed);
        }
        if generation != 0 && generation != self.generation {
            return Err(CoordinationError::StaleGeneration {
                expected: generation,
                active: self.generation,
            });
        }
        Ok(())
    }

    fn consume_failed_workflow_approval(
        &self,
        id: &str,
        expected: &WorkflowRecord,
    ) -> CoordinationError {
        let mut state = lock(&self.state);
        let Some(current) = state.workflows.get(id) else {
            return CoordinationError::WorkflowStale;
        };
        if current.projected.state == AgentWorkflowState::Approved {
            return CoordinationError::WorkflowApproved;
        }
        if current.lifecycle_revision == expected.lifecycle_revision
            && current.worktree_revision == expected.worktree_revision
            && current.binding == expected.binding
            && current.projected == expected.projected
        {
            state.workflows.remove(id);
        }
        CoordinationError::WorkflowStale
    }

    #[cfg(test)]
    pub(crate) fn install_preview_barrier(
        &self,
        started: Arc<std::sync::Barrier>,
        release: Arc<std::sync::Barrier>,
    ) {
        *lock(&self.preview_barrier) = Some((started, release));
    }

    fn exact_run(&self, id: &str) -> Result<(Run, Option<RuntimeAssociation>), CoordinationError> {
        let mut selected = None;
        self.registry
            .with_exact_runtime_snapshot(CANDIDATE_LIMIT, |runs| {
                selected = runs.iter().find(|run| run.id == id).cloned();
                Ok(())
            })
            .map_err(registry_coordination_error)?;
        let run = selected.ok_or(CoordinationError::RunNotFound)?;
        let association = current_association(self.store.as_ref(), &run);
        Ok((run, association))
    }

    fn exact_pair(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> Result<
        (
            Run,
            Run,
            Option<RuntimeAssociation>,
            Option<RuntimeAssociation>,
        ),
        CoordinationError,
    > {
        let mut source = None;
        let mut target = None;
        self.registry
            .with_exact_runtime_snapshot(CANDIDATE_LIMIT, |runs| {
                source = runs.iter().find(|run| run.id == source_id).cloned();
                target = runs.iter().find(|run| run.id == target_id).cloned();
                Ok(())
            })
            .map_err(registry_coordination_error)?;
        let (source, target) = source
            .zip(target)
            .filter(|(source, target)| run_is_live(source) && run_is_live(target))
            .ok_or(CoordinationError::HandoffInactive)?;
        let source_association = current_association(self.store.as_ref(), &source);
        let target_association = current_association(self.store.as_ref(), &target);
        if (source.association.is_some() && source_association.is_none())
            || (target.association.is_some() && target_association.is_none())
        {
            return Err(CoordinationError::HandoffStale);
        }
        Ok((source, target, source_association, target_association))
    }

    fn exact_worktree_run(
        &self,
        run_id: &str,
        expected_revision: u64,
    ) -> Result<(Run, Option<RuntimeAssociation>), CoordinationError> {
        let (run, association) = self.exact_run(run_id)?;
        if !run_is_live(&run) {
            return Err(CoordinationError::WorktreeInactive);
        }
        if association_revision(association) != expected_revision {
            return Err(CoordinationError::WorktreeRevision);
        }
        Ok((run, association))
    }

    fn exact_workflow_run(
        &self,
        run_id: &str,
        expected_revision: u64,
    ) -> Result<(Run, Option<RuntimeAssociation>), CoordinationError> {
        self.exact_worktree_run(run_id, expected_revision)
            .map_err(|error| match error {
                CoordinationError::WorktreeInactive => CoordinationError::WorkflowInactive,
                other => other,
            })
    }

    fn capture_workflow(
        &self,
        run_id: &str,
        expected_revision: u64,
        kind: AgentWorkflowKind,
        target_branch: &str,
    ) -> Result<WorkflowCapture, CoordinationError> {
        let (before, association) = self.exact_workflow_run(run_id, expected_revision)?;
        let epoch = self.worktree_epoch(&before, association)?;
        let worktree = if let Some(claim) = &epoch.claim {
            let observed = self
                .git
                .inspect_worktree(&self.project_root, Path::new(&claim.identity.root))
                .map_err(|_| CoordinationError::WorkflowStale)?;
            if !same_worktree_repository(&observed, &claim.identity)
                || !path_inside(Path::new(&observed.root), Path::new(&before.cwd))
            {
                return Err(CoordinationError::WorkflowStale);
            }
            Some(observed)
        } else {
            if !path_inside(&self.project_root, Path::new(&before.cwd)) {
                return Err(CoordinationError::WorkflowStale);
            }
            None
        };
        let root = worktree
            .as_ref()
            .map_or(self.project_root.as_path(), |identity| {
                Path::new(&identity.root)
            });
        let binding = workflow_binding(self.git.snapshot(root)?, kind, target_branch)?;
        if let Some(identity) = &worktree
            && !binding_matches_worktree(&binding, identity)
        {
            return Err(CoordinationError::WorkflowStale);
        }
        let (after, after_association) = self.exact_workflow_run(run_id, expected_revision)?;
        if !same_run_epoch(&before, &after)
            || before.cwd != after.cwd
            || association != after_association
            || !worktree_epoch_matches(
                &lock(&self.state),
                run_id,
                epoch.revision,
                worktree.as_ref(),
            )
        {
            return Err(CoordinationError::WorkflowStale);
        }
        Ok(WorkflowCapture {
            run: after,
            association: after_association,
            worktree,
            worktree_revision: epoch.revision,
            binding,
        })
    }

    fn worktree_epoch(
        &self,
        run: &Run,
        association: Option<RuntimeAssociation>,
    ) -> Result<WorktreeEpoch, CoordinationError> {
        let mut state = lock(&self.state);
        let revision = state.worktree_revisions.get(&run.id).copied().unwrap_or(0);
        let claim = state.worktrees.get(&run.id).cloned();
        if claim
            .as_ref()
            .is_some_and(|claim| !worktree_claim_current(self.generation, claim, run, association))
        {
            state.worktrees.remove(&run.id);
            bump_worktree_revision(&mut state.worktree_revisions, &run.id);
            return Err(CoordinationError::WorkflowStale);
        }
        Ok(WorktreeEpoch { revision, claim })
    }

    fn validate_workflow_capture(
        &self,
        capture: &WorkflowCapture,
        runs: &[Run],
    ) -> Result<(), CoordinationError> {
        let run = runs
            .iter()
            .find(|run| run.id == capture.run.id)
            .ok_or(CoordinationError::WorkflowStale)?;
        let association = current_association(self.store.as_ref(), run);
        if !run_is_live(run)
            || !same_run_epoch(&capture.run, run)
            || capture.run.cwd != run.cwd
            || capture.association != association
        {
            return Err(CoordinationError::WorkflowStale);
        }
        Ok(())
    }

    fn with_exact_snapshot<T>(
        &self,
        use_snapshot: impl FnOnce(&[Run]) -> Result<T, CoordinationError>,
    ) -> Result<T, CoordinationError> {
        let mut result = None;
        self.registry
            .with_exact_runtime_snapshot(CANDIDATE_LIMIT, |runs| {
                result = Some(use_snapshot(runs));
                Ok(())
            })
            .map_err(registry_coordination_error)?;
        result.ok_or(CoordinationError::RegistryUnavailable)?
    }

    #[allow(clippy::too_many_lines)] // One bounded snapshot transaction is easier to audit whole.
    #[allow(clippy::unnecessary_wraps)] // Stable coordinator seam remains fallible for adapters.
    fn projection(&self) -> Result<Projection, CoordinationError> {
        self.projection_until(None)
    }

    #[allow(clippy::too_many_lines)] // One bounded snapshot transaction is easier to audit whole.
    #[allow(clippy::unnecessary_wraps)] // Stable coordinator seam remains fallible for adapters.
    fn projection_until(&self, deadline: Option<Instant>) -> Result<Projection, CoordinationError> {
        ensure_projection_deadline(deadline)?;
        let (sessions, terminal_total) = self.sessions.snapshot(CANDIDATE_LIMIT);
        ensure_projection_deadline(deadline)?;
        let (runs, agent_total) = self.registry.runtime_snapshot_bounded(CANDIDATE_LIMIT);
        ensure_projection_deadline(deadline)?;
        let sources_truncated = terminal_total > sessions.len() || agent_total > runs.len();
        let mut terminals = sessions
            .into_iter()
            .map(|session| {
                ensure_projection_deadline(deadline)?;
                Ok(TerminalRuntimeSummary {
                    session_id: session.id.clone(),
                    profile_kind: session.profile_kind,
                    live: session_state_live(&session.state),
                    state: session.state,
                    association: session.association.as_ref().and_then(|association| {
                        self.store.current_association(&session.id, association)
                    }),
                })
            })
            .collect::<Result<Vec<_>, CoordinationError>>()?;
        terminals.sort_by(|left, right| {
            association_sort_key(left.association, &left.session_id)
                .cmp(&association_sort_key(right.association, &right.session_id))
        });
        let terminals_by_id: BTreeMap<String, TerminalRuntimeSummary> = terminals
            .iter()
            .cloned()
            .map(|terminal| (terminal.session_id.clone(), terminal))
            .collect();
        let mut exact_runs = BTreeMap::new();
        let mut events_by_run = BTreeMap::new();
        let mut event_totals_by_run = BTreeMap::new();
        let mut incomplete = sources_truncated || agent_total > PROJECTION_LIMIT;
        let mut agents = Vec::new();
        for run in runs {
            ensure_projection_deadline(deadline)?;
            let association = current_association(self.store.as_ref(), &run);
            let terminal_backed =
                run.registration_kind == RegistrationKind::Launched && !run.terminal_id.is_empty();
            let terminal = terminals_by_id.get(&run.terminal_id);
            let terminal_present = terminal.is_some();
            let corresponding_terminal = association.is_some()
                && terminal.is_some_and(|terminal| terminal.association == association);
            let intelligence_snapshot = self
                .registry
                .intelligence_snapshot(&run.id, INTELLIGENCE_EVENTS_PER_RUN);
            let (intelligence, events, event_total) = match intelligence_snapshot {
                Ok((observed, events, total, _)) if same_run_epoch(&run, &observed) => {
                    ensure_projection_deadline(deadline)?;
                    if total > events.len() {
                        incomplete = true;
                    }
                    let mut current_events = Vec::with_capacity(events.len());
                    for event in events {
                        ensure_projection_deadline(deadline)?;
                        if notification_current(self.generation, &run, association, &event) {
                            current_events.push(event);
                        }
                    }
                    let events = current_events;
                    let intelligence = crate::derive_run_intelligence(&run, &events);
                    let summary = AgentIntelligenceSummary {
                        state: intelligence.state,
                        confidence: intelligence.confidence,
                        evidence_count: intelligence.evidence.len(),
                        event_count: intelligence.event_count,
                        last_event_at: (!intelligence.last_event_at.is_zero())
                            .then_some(intelligence.last_event_at),
                    };
                    (Some(summary), events, total)
                }
                _ => {
                    incomplete = true;
                    (None, Vec::new(), 0)
                }
            };
            let activity_state = intelligence.as_ref().map_or_else(
                || {
                    crate::derive_activity_state(
                        &run,
                        &crate::RunIntelligence {
                            run_id: run.id.clone(),
                            state: IntelligenceState::Unknown,
                            confidence: IntelligenceConfidence::Unset,
                            evidence: Vec::new(),
                            event_count: 0,
                            last_event_at: Timestamp::ZERO,
                        },
                    )
                },
                |summary| {
                    crate::derive_activity_state(
                        &run,
                        &crate::RunIntelligence {
                            run_id: run.id.clone(),
                            state: summary.state,
                            confidence: summary.confidence,
                            evidence: Vec::new(),
                            event_count: summary.event_count,
                            last_event_at: summary.last_event_at.unwrap_or(Timestamp::ZERO),
                        },
                    )
                },
            );
            events_by_run.insert(run.id.clone(), events);
            event_totals_by_run.insert(run.id.clone(), event_total);
            exact_runs.insert(run.id.clone(), run.clone());
            let live = run_is_live(&run);
            agents.push(AgentRuntimeSummary {
                run_id: run.id,
                registration_kind: run.registration_kind,
                terminal_id: run.terminal_id,
                terminal_backed,
                terminal_present,
                corresponding_terminal,
                state: run.state,
                process_state: run.process_state,
                lease_state: run.lease_state,
                live,
                activity_state,
                association,
                intelligence,
            });
        }
        agents.sort_by(|left, right| {
            association_sort_key(left.association, &left.run_id)
                .cmp(&association_sort_key(right.association, &right.run_id))
        });
        let agent_bounds = BoundedSnapshot::new(agents.len().min(PROJECTION_LIMIT), agent_total);
        let terminal_bounds =
            BoundedSnapshot::new(terminals.len().min(PROJECTION_LIMIT), terminal_total);
        let agent_candidates = agents.clone();
        agents.truncate(PROJECTION_LIMIT);
        terminals.truncate(PROJECTION_LIMIT);
        ensure_projection_deadline(deadline)?;
        Ok(Projection {
            generation: self.generation,
            terminals,
            terminal_bounds,
            agents,
            agent_candidates,
            agent_bounds,
            exact_runs,
            events_by_run,
            event_totals_by_run,
            sources_truncated,
            incomplete,
        })
    }

    fn activity_from_projection(
        &self,
        projection: &Projection,
    ) -> Result<AgentActivitySnapshot, CoordinationError> {
        let git = self.git.snapshot(&self.project_root)?;
        self.activity_from_projection_git(projection, &git)
    }

    #[allow(clippy::unnecessary_wraps)] // Stable aggregate seam preserves coordination errors.
    fn activity_from_projection_git(
        &self,
        projection: &Projection,
        git: &CoordinationGitSnapshot,
    ) -> Result<AgentActivitySnapshot, CoordinationError> {
        self.activity_from_projection_git_until(projection, git, None)
    }

    fn activity_from_projection_git_until(
        &self,
        projection: &Projection,
        git: &CoordinationGitSnapshot,
        deadline: Option<Instant>,
    ) -> Result<AgentActivitySnapshot, CoordinationError> {
        ensure_projection_deadline(deadline)?;
        let mut items = projection
            .agents
            .iter()
            .map(|run| {
                ensure_projection_deadline(deadline)?;
                Ok(AgentActivity {
                    run_id: run.run_id.clone(),
                    state: run.activity_state,
                    registration_kind: run.registration_kind,
                    terminal_backed: run.terminal_backed,
                    terminal_present: run.terminal_present,
                    corresponding_terminal: run.corresponding_terminal,
                    live: run.live,
                    association: run.association,
                    confidence: run.intelligence.as_ref().map(|value| value.confidence),
                    evidence_count: run
                        .intelligence
                        .as_ref()
                        .map_or(0, |value| value.evidence_count),
                    event_count: run
                        .intelligence
                        .as_ref()
                        .map_or(0, |value| value.event_count),
                    last_event_at: run
                        .intelligence
                        .as_ref()
                        .and_then(|value| value.last_event_at),
                    ownership: None,
                    worktree: None,
                })
            })
            .collect::<Result<Vec<_>, CoordinationError>>()?;
        let mut state = lock(&self.state);
        let mut valid_ownership = BTreeMap::new();
        for item in &mut items {
            ensure_projection_deadline(deadline)?;
            let Some(run) = projection.exact_runs.get(&item.run_id) else {
                continue;
            };
            if let Some(claim) = state.ownership.get(&item.run_id).cloned()
                && ownership_claim_current(self.generation, &claim, run, item.association)
            {
                item.ownership = Some(project_ownership(&claim));
                valid_ownership.insert(item.run_id.clone(), claim);
            }
            if let Some(claim) = state.worktrees.get(&item.run_id).cloned() {
                if worktree_claim_current(self.generation, &claim, run, item.association) {
                    item.worktree = Some(project_worktree(&claim));
                } else {
                    state.worktrees.remove(&item.run_id);
                }
            }
        }
        let now = (self.now)();
        prune_handoffs(&mut state.handoffs, now);
        prune_workflows(&mut state.workflows, now);
        let handoffs = project_handoffs(&mut state.handoffs, projection, self.generation);
        let workflows =
            project_workflows(&mut state, projection, self.generation, &self.project_root);
        drop(state);
        let counts = activity_counts(&items);
        let (conflicts, conflict_bounds) =
            activity_conflicts(&projection.agent_candidates, &valid_ownership);
        let (notifications, notification_bounds, notifications_incomplete) =
            notifications(self.generation, projection);
        ensure_projection_deadline(deadline)?;
        let mut workflow_targets = git
            .branches
            .iter()
            .filter(|branch| !branch.name.is_empty() && valid_workflow_head(&branch.head))
            .map(|branch| branch.name.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let workflow_targets_incomplete = git.branches.len() >= 100;
        workflow_targets.sort();
        ensure_projection_deadline(deadline)?;
        Ok(AgentActivitySnapshot {
            state: "ready".to_owned(),
            items,
            counts,
            bounds: projection.agent_bounds,
            conflicts,
            conflict_bounds,
            analysis_incomplete: projection.incomplete || conflict_bounds.more > 0,
            notifications,
            notification_bounds,
            notifications_incomplete,
            handoffs,
            worktrees: git.worktrees.clone(),
            worktree_bounds: git.worktree_bounds,
            worktrees_incomplete: git.worktrees_incomplete,
            workflows,
            workflow_targets,
            workflow_targets_incomplete,
        })
    }

    fn random_token(&self) -> Result<String, CoordinationError> {
        let mut bytes = [0_u8; 32];
        (self.random)(&mut bytes).map_err(CoordinationError::Message)?;
        Ok(raw_url_base64(&bytes))
    }

    fn notify(&self) {
        if let Some(revision) = &self.mutation_revision {
            increment_saturating(revision);
        }
        if let Some(sender) = &self.runtime_changed {
            let _ = sender.try_send(());
        }
    }
}

fn increment_saturating(value: &AtomicU64) {
    let _ = value.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        Some(current.saturating_add(1))
    });
}

#[derive(Clone)]
struct Projection {
    generation: u64,
    #[allow(dead_code)]
    terminals: Vec<TerminalRuntimeSummary>,
    #[allow(dead_code)]
    terminal_bounds: BoundedSnapshot,
    agents: Vec<AgentRuntimeSummary>,
    agent_candidates: Vec<AgentRuntimeSummary>,
    agent_bounds: BoundedSnapshot,
    exact_runs: BTreeMap<String, Run>,
    events_by_run: BTreeMap<String, Vec<Event>>,
    event_totals_by_run: BTreeMap<String, usize>,
    sources_truncated: bool,
    incomplete: bool,
}

fn current_association(store: &dyn CoordinationStore, run: &Run) -> Option<RuntimeAssociation> {
    run.association
        .as_ref()
        .and_then(|association| store.current_association(&run.id, association))
}

fn run_is_live(run: &Run) -> bool {
    if run.state != RunState::Running || run.process_state == ProcessState::Exited {
        return false;
    }
    match run.registration_kind {
        RegistrationKind::External => run.lease_state == LeaseState::Active,
        RegistrationKind::Launched => run.process_state == ProcessState::Running,
        RegistrationKind::Unset => false,
    }
}

fn session_state_live(state: &str) -> bool {
    matches!(state, "starting" | "running" | "closing")
}

fn association_sort_key(
    association: Option<RuntimeAssociation>,
    id: &str,
) -> (bool, u64, u64, u64, &str) {
    association.map_or((true, 0, 0, 0, id), |association| {
        (
            false,
            association.plan_id,
            association.task_id,
            association.revision,
            id,
        )
    })
}

fn same_run_epoch(left: &Run, right: &Run) -> bool {
    left.id == right.id
        && left.lifecycle_revision != 0
        && left.lifecycle_revision == right.lifecycle_revision
        && left.project_root == right.project_root
        && left.terminal_id == right.terminal_id
        && left.registration_kind == right.registration_kind
        && left.association == right.association
}

fn ownership_claim_equal(left: &OwnershipClaim, right: &OwnershipClaim) -> bool {
    left.generation == right.generation
        && left.run_id == right.run_id
        && left.plan_id == right.plan_id
        && left.task_id == right.task_id
        && left.association_revision == right.association_revision
        && left.lifecycle_revision == right.lifecycle_revision
}

fn ownership_claim_current(
    generation: u64,
    claim: &OwnershipClaim,
    run: &Run,
    association: Option<RuntimeAssociation>,
) -> bool {
    claim.generation == generation
        && claim.run_id == run.id
        && claim.lifecycle_revision != 0
        && claim.lifecycle_revision == run.lifecycle_revision
        && run_is_live(run)
        && association.is_some_and(|association| {
            association.task_id != 0
                && claim.plan_id == association.plan_id
                && claim.task_id == association.task_id
                && claim.association_revision == association.revision
        })
}

fn project_ownership(claim: &OwnershipClaim) -> AgentTaskOwnership {
    AgentTaskOwnership {
        plan_id: claim.plan_id,
        task_id: claim.task_id,
        association_revision: claim.association_revision,
    }
}

fn worktree_claim_current(
    generation: u64,
    claim: &WorktreeClaim,
    run: &Run,
    association: Option<RuntimeAssociation>,
) -> bool {
    claim.generation == generation
        && claim.run_id == run.id
        && claim.lifecycle_revision != 0
        && claim.lifecycle_revision == run.lifecycle_revision
        && run_is_live(run)
        && claim.association == association
}

fn bump_worktree_revision(revisions: &mut BTreeMap<String, u64>, run_id: &str) {
    let revision = revisions.entry(run_id.to_owned()).or_insert(0);
    *revision = revision.saturating_add(1).max(1);
}

fn worktree_epoch_matches(
    state: &CoordinationState,
    run_id: &str,
    revision: u64,
    expected: Option<&WorktreeIdentity>,
) -> bool {
    if state.worktree_revisions.get(run_id).copied().unwrap_or(0) != revision {
        return false;
    }
    match (state.worktrees.get(run_id), expected) {
        (None, None) => true,
        (Some(claim), Some(expected)) => same_worktree_repository(&claim.identity, expected),
        _ => false,
    }
}

fn project_worktree(claim: &WorktreeClaim) -> AgentWorktreeAssociation {
    AgentWorktreeAssociation {
        identity: claim.identity.clone(),
        verified: true,
        isolated: claim.isolated,
        cwd_matches: true,
    }
}

fn association_revision(association: Option<RuntimeAssociation>) -> u64 {
    association.map_or(0, |association| association.revision)
}

fn prune_handoffs(records: &mut BTreeMap<String, HandoffRecord>, now: Timestamp) {
    records.retain(|_, record| now < record.projected.expires_at);
}

fn prune_workflows(records: &mut BTreeMap<String, WorkflowRecord>, now: Timestamp) {
    records.retain(|_, record| now < record.projected.expires_at);
}

fn project_handoffs(
    records: &mut BTreeMap<String, HandoffRecord>,
    projection: &Projection,
    generation: u64,
) -> AgentHandoffInbox {
    records.retain(|_, record| {
        let source = projection.exact_runs.get(&record.projected.source_run_id);
        let target = projection.exact_runs.get(&record.projected.target_run_id);
        source.zip(target).is_some_and(|(source, target)| {
            record.projected.generation == generation
                && record.source_lifecycle_revision == source.lifecycle_revision
                && record.target_lifecycle_revision == target.lifecycle_revision
                && record.projected.source_association
                    == projection
                        .agent_candidates
                        .iter()
                        .find(|run| run.run_id == source.id)
                        .and_then(|run| run.association)
                && record.projected.target_association
                    == projection
                        .agent_candidates
                        .iter()
                        .find(|run| run.run_id == target.id)
                        .and_then(|run| run.association)
        })
    });
    let mut items = records
        .values()
        .map(|record| record.projected.clone())
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    AgentHandoffInbox {
        bounds: BoundedSnapshot::new(items.len(), items.len()),
        items,
        incomplete: projection.incomplete,
    }
}

fn project_workflows(
    state: &mut CoordinationState,
    projection: &Projection,
    generation: u64,
    project_root: &Path,
) -> AgentWorkflowInbox {
    let worktrees = state.worktrees.clone();
    let worktree_revisions = state.worktree_revisions.clone();
    state.workflows.retain(|_, record| {
        let Some(run) = projection.exact_runs.get(&record.projected.run_id) else {
            return false;
        };
        let association = projection
            .agent_candidates
            .iter()
            .find(|candidate| candidate.run_id == run.id)
            .and_then(|candidate| candidate.association);
        if record.projected.generation != generation
            || !run_is_live(run)
            || record.lifecycle_revision != run.lifecycle_revision
            || record.projected.association != association
            || worktree_revisions.get(&run.id).copied().unwrap_or(0) != record.worktree_revision
        {
            return false;
        }
        match (&record.worktree, worktrees.get(&run.id)) {
            (None, None) => path_inside(project_root, Path::new(&run.cwd)),
            (Some(expected), Some(claim)) => {
                worktree_claim_current(generation, claim, run, association)
                    && same_worktree_repository(expected, &claim.identity)
            }
            _ => false,
        }
    });
    let mut items = state
        .workflows
        .values()
        .map(|record| record.projected.clone())
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    AgentWorkflowInbox {
        bounds: BoundedSnapshot::new(items.len(), items.len()),
        items,
        incomplete: projection.incomplete,
        notice: AGENT_WORKFLOW_NOTICE.to_owned(),
    }
}

fn activity_counts(items: &[AgentActivity]) -> AgentActivityCounts {
    let mut counts = AgentActivityCounts::default();
    for item in items {
        match item.state {
            ActivityState::Running => counts.running += 1,
            ActivityState::Waiting => counts.waiting += 1,
            ActivityState::Blocked => counts.blocked += 1,
            ActivityState::Completed => counts.completed += 1,
            ActivityState::Failed => counts.failed += 1,
            ActivityState::Stale => counts.stale += 1,
            ActivityState::Unknown => counts.unknown += 1,
        }
    }
    counts
}

pub(crate) fn activity_conflicts(
    runs: &[AgentRuntimeSummary],
    ownership: &BTreeMap<String, OwnershipClaim>,
) -> (Vec<AgentActivityConflict>, BoundedSnapshot) {
    let mut grouped: BTreeMap<(u64, u64), BTreeSet<String>> = BTreeMap::new();
    for run in runs {
        if run.live
            && let Some(association) = run.association
            && association.task_id != 0
        {
            grouped
                .entry((association.plan_id, association.task_id))
                .or_default()
                .insert(run.run_id.clone());
        }
    }
    let total = grouped.values().filter(|runs| runs.len() >= 2).count();
    let mut conflicts = Vec::new();
    for ((plan_id, task_id), runs) in grouped.into_iter().filter(|(_, runs)| runs.len() >= 2) {
        if conflicts.len() == PROJECTION_LIMIT {
            break;
        }
        let run_ids = runs.into_iter().collect::<Vec<_>>();
        let owner_count = run_ids
            .iter()
            .filter(|run_id| ownership.contains_key(*run_id))
            .count();
        let shown = run_ids.len().min(16);
        conflicts.push(AgentActivityConflict {
            plan_id,
            task_id,
            agent_count: run_ids.len(),
            owner_count,
            run_ids: run_ids[..shown].to_vec(),
            bounds: BoundedSnapshot::new(shown, run_ids.len()),
        });
    }
    let bounds = BoundedSnapshot::new(conflicts.len(), total);
    (conflicts, bounds)
}

fn notifications(
    generation: u64,
    projection: &Projection,
) -> (Vec<AgentNotification>, BoundedSnapshot, bool) {
    let mut latest: BTreeMap<(String, EventNotificationKind, u64, u64, u64), AgentNotification> =
        BTreeMap::new();
    let mut incomplete = projection.incomplete;
    for run in &projection.agents {
        if projection
            .event_totals_by_run
            .get(&run.run_id)
            .is_some_and(|total| *total > NOTIFICATION_EVENTS_PER_RUN)
        {
            incomplete = true;
        }
        let Some(exact) = projection.exact_runs.get(&run.run_id) else {
            incomplete = true;
            continue;
        };
        for event in projection
            .events_by_run
            .get(&run.run_id)
            .into_iter()
            .flatten()
            .rev()
            .take(NOTIFICATION_EVENTS_PER_RUN)
        {
            if !event.notification.is_valid()
                || !notification_current(generation, exact, run.association, event)
            {
                continue;
            }
            let association = run.association.unwrap_or_default();
            let key = (
                run.run_id.clone(),
                event.notification,
                association.plan_id,
                association.task_id,
                association.revision,
            );
            let candidate = AgentNotification {
                id: event.id.clone(),
                run_id: run.run_id.clone(),
                kind: event.notification,
                observed_at: event.observed_at,
                terminal_backed: run.terminal_backed,
                association: run.association,
            };
            let replace = latest.get(&key).is_none_or(|current| {
                candidate.observed_at > current.observed_at
                    || (candidate.observed_at == current.observed_at && candidate.id > current.id)
            });
            if replace {
                latest.insert(key, candidate);
            }
        }
    }
    let mut items = latest.into_values().collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .observed_at
            .cmp(&left.observed_at)
            .then_with(|| left.run_id.cmp(&right.run_id))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    let total = items.len();
    if items.len() > PROJECTION_LIMIT {
        items.truncate(PROJECTION_LIMIT);
        incomplete = true;
    }
    let bounds = BoundedSnapshot::new(items.len(), total);
    (items, bounds, incomplete)
}

fn notification_current(
    generation: u64,
    run: &Run,
    association: Option<RuntimeAssociation>,
    event: &Event,
) -> bool {
    let correlation = &event.correlation;
    if event.lifecycle_revision != run.lifecycle_revision
        || correlation.project_root != run.project_root
        || correlation.terminal_id != run.terminal_id
    {
        return false;
    }
    association.map_or_else(
        || {
            correlation.generation == 0
                && correlation.association_revision == 0
                && correlation.plan_id == 0
                && correlation.task_id == 0
        },
        |association| {
            correlation.generation == generation
                && correlation.plan_id == association.plan_id
                && correlation.task_id == association.task_id
                && correlation.association_revision == association.revision
        },
    )
}

#[allow(clippy::too_many_lines)] // Exact ordered suggestion pipeline stays auditable together.
fn build_suggestions(
    store: &dyn CoordinationStore,
    association: Option<RuntimeAssociation>,
    events: &[Event],
) -> Result<Vec<AgentSuggestion>, CoordinationError> {
    let mut candidates = Vec::new();
    if let Some(association) = association {
        if association.task_id != 0
            && let Some(task) = store.task_context(association.task_id)?
        {
            candidates.push(AgentSuggestion {
                kind: AgentSuggestionKind::Context,
                target_id: task.id,
                label: bounded_suggestion_text(&format!("Task #{} · {}", task.id, task.title)),
                path: String::new(),
                reason: "current host-validated task association".to_owned(),
                evidence_event_ids: Vec::new(),
            });
        }
        if association.plan_id != 0
            && let Some(title) = store.plan_title(association.plan_id)?
        {
            candidates.push(AgentSuggestion {
                kind: AgentSuggestionKind::Context,
                target_id: association.plan_id,
                label: bounded_suggestion_text(&format!("Plan #{} · {title}", association.plan_id)),
                path: String::new(),
                reason: "current host-validated plan association".to_owned(),
                evidence_event_ids: Vec::new(),
            });
        }
    }
    let mut seen_paths = BTreeSet::new();
    'events: for event in events.iter().rev() {
        for path in &event.paths {
            if !seen_paths.insert(path.clone()) {
                continue;
            }
            candidates.push(AgentSuggestion {
                kind: AgentSuggestionKind::File,
                target_id: 0,
                label: bounded_suggestion_text(path),
                path: path.clone(),
                reason: "observed structured file evidence".to_owned(),
                evidence_event_ids: vec![event.id.clone()],
            });
            if seen_paths.len() == FILE_SUGGESTION_LIMIT {
                break 'events;
            }
        }
    }
    let mut decisions = 0;
    for decision in store.recent_decisions(50)? {
        if decisions == DECISION_SUGGESTION_LIMIT {
            break;
        }
        let relevant = match decision.target {
            CoordinationTarget::Project => decision.target_id == 0,
            CoordinationTarget::Plan => {
                association.is_some_and(|value| value.plan_id == decision.target_id)
            }
            CoordinationTarget::Task => {
                association.is_some_and(|value| value.task_id == decision.target_id)
            }
        };
        if !relevant {
            continue;
        }
        candidates.push(AgentSuggestion {
            kind: AgentSuggestionKind::Decision,
            target_id: decision.id,
            label: bounded_suggestion_text(&decision.body),
            path: String::new(),
            reason: "relevant durable decision".to_owned(),
            evidence_event_ids: Vec::new(),
        });
        decisions += 1;
    }
    let mut issues = 0;
    for issue in store.open_issues()? {
        if issues == ISSUE_SUGGESTION_LIMIT {
            break;
        }
        let relevant = match association {
            None => issue.task_id == 0,
            Some(value) if value.task_id != 0 => issue.task_id == value.task_id,
            Some(value) if value.plan_id != 0 && issue.task_id != 0 => store
                .task_context(issue.task_id)?
                .is_some_and(|task| task.plan_id == value.plan_id),
            Some(_) => false,
        };
        if !relevant {
            continue;
        }
        candidates.push(AgentSuggestion {
            kind: AgentSuggestionKind::Issue,
            target_id: issue.id,
            label: bounded_suggestion_text(&issue.title),
            path: String::new(),
            reason: "relevant open issue".to_owned(),
            evidence_event_ids: Vec::new(),
        });
        issues += 1;
    }
    let mut seen = BTreeSet::new();
    candidates.retain(|candidate| {
        seen.insert((
            candidate.kind,
            candidate.target_id,
            candidate.path.clone(),
            candidate.label.clone(),
        ))
    });
    Ok(candidates)
}

pub(crate) fn bounded_suggestion_text(value: &str) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.len() <= SUGGESTION_TEXT_BYTES {
        return collapsed;
    }
    let mut end = SUGGESTION_TEXT_BYTES;
    while !collapsed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", collapsed[..end].trim())
}

#[allow(clippy::too_many_lines)] // Exact ordered matrix intentionally stays together.
fn build_drift(
    projection: &Projection,
    activity: &AgentActivitySnapshot,
    git: &CoordinationGitSnapshot,
    linked_commits: &[String],
    tracking_started_at: Timestamp,
) -> DriftSnapshot {
    let mut findings = Vec::new();
    let mut incomplete = projection.incomplete
        || git.changed_more > 0
        || git.untracked_more > 0
        || git.recent_commits_incomplete
        || git.unpushed_commits_incomplete;
    for path in &git.changed_paths {
        findings.push(drift_path("checkoutChangedPath", "info", path));
    }
    for path in &git.untracked_paths {
        findings.push(drift_path("untrackedFile", "warning", path));
    }
    let observed = git
        .unpushed_commits
        .iter()
        .chain(&git.recent_commits)
        .collect::<Vec<_>>();
    let linked = linked_observed_commits(linked_commits, &observed);
    let mut commits = BTreeSet::new();
    for commit in observed {
        let sha = commit.sha.trim().to_ascii_lowercase();
        if !tracking_started_at.is_zero() {
            let Some(committed) = commit.committed_at.unix_nanoseconds() else {
                incomplete = true;
                continue;
            };
            let Some(started) = tracking_started_at.unix_nanoseconds() else {
                continue;
            };
            let started_seconds = started.div_euclid(1_000_000_000) * 1_000_000_000;
            if committed < started_seconds {
                continue;
            }
        }
        if !sha.is_empty() && !linked.contains(&sha) && commits.insert(sha.clone()) {
            findings.push(DriftFinding {
                kind: "unlinkedCommit".to_owned(),
                severity: "info".to_owned(),
                scope: "projectUnattributed".to_owned(),
                path: String::new(),
                sha,
                run_ids: Vec::new(),
                plan_ids: Vec::new(),
                task_ids: Vec::new(),
                evidence_count: 1,
            });
        }
    }
    for run in &projection.agents {
        if run.live
            && run
                .intelligence
                .as_ref()
                .is_some_and(|value| value.state == IntelligenceState::PotentiallyDrifting)
            && let Some(association) = run.association
        {
            findings.push(DriftFinding {
                kind: "taskDriftSignal".to_owned(),
                severity: "warning".to_owned(),
                scope: "agent".to_owned(),
                path: String::new(),
                sha: String::new(),
                run_ids: vec![run.run_id.clone()],
                plan_ids: vec![association.plan_id],
                task_ids: vec![association.task_id],
                evidence_count: run
                    .intelligence
                    .as_ref()
                    .map_or(0, |value| value.evidence_count),
            });
        }
    }
    let owned: BTreeSet<&str> = activity
        .items
        .iter()
        .filter(|item| item.ownership.is_some())
        .map(|item| item.run_id.as_str())
        .collect();
    let mut by_path: BTreeMap<String, Vec<(&str, RuntimeAssociation)>> = BTreeMap::new();
    for run in &projection.agents {
        if !run.live || !owned.contains(run.run_id.as_str()) {
            continue;
        }
        let Some(association) = run.association.filter(|value| value.task_id != 0) else {
            continue;
        };
        let mut seen = BTreeSet::new();
        for path in projection
            .events_by_run
            .get(&run.run_id)
            .into_iter()
            .flatten()
            .filter(|event| {
                projection.exact_runs.get(&run.run_id).is_some_and(|exact| {
                    notification_current(projection.generation, exact, run.association, event)
                })
            })
            .flat_map(|event| &event.paths)
        {
            if !path.is_empty() && seen.insert(path.clone()) {
                by_path
                    .entry(path.clone())
                    .or_default()
                    .push((&run.run_id, association));
            }
        }
    }
    for (path, mut evidence) in by_path {
        let targets: BTreeSet<(u64, u64)> = evidence
            .iter()
            .map(|(_, association)| (association.plan_id, association.task_id))
            .collect();
        if targets.len() < 2 {
            continue;
        }
        evidence.sort_by_key(|(run_id, _)| *run_id);
        findings.push(DriftFinding {
            kind: "crossTaskPathOverlap".to_owned(),
            severity: "warning".to_owned(),
            scope: "taskComparison".to_owned(),
            path,
            sha: String::new(),
            run_ids: evidence
                .iter()
                .map(|(run_id, _)| (*run_id).to_owned())
                .collect(),
            plan_ids: evidence
                .iter()
                .map(|(_, association)| association.plan_id)
                .collect(),
            task_ids: evidence
                .iter()
                .map(|(_, association)| association.task_id)
                .collect(),
            evidence_count: evidence.len(),
        });
    }
    findings.sort_by(|left, right| {
        (
            left.severity != "warning",
            &left.kind,
            &left.path,
            &left.sha,
            &left.run_ids,
        )
            .cmp(&(
                right.severity != "warning",
                &right.kind,
                &right.path,
                &right.sha,
                &right.run_ids,
            ))
    });
    let total = findings.len();
    if findings.len() > PROJECTION_LIMIT {
        findings.truncate(PROJECTION_LIMIT);
        incomplete = true;
    }
    DriftSnapshot {
        state: "ready".to_owned(),
        bounds: BoundedSnapshot::new(findings.len(), total),
        findings,
        incomplete,
    }
}

fn drift_path(kind: &str, severity: &str, path: &str) -> DriftFinding {
    DriftFinding {
        kind: kind.to_owned(),
        severity: severity.to_owned(),
        scope: "projectUnattributed".to_owned(),
        path: path.to_owned(),
        sha: String::new(),
        run_ids: Vec::new(),
        plan_ids: Vec::new(),
        task_ids: Vec::new(),
        evidence_count: 1,
    }
}

fn linked_observed_commits(linked_commits: &[String], observed: &[&GitCommit]) -> BTreeSet<String> {
    let observed_shas = observed
        .iter()
        .map(|commit| commit.sha.trim().to_ascii_lowercase())
        .filter(|sha| !sha.is_empty())
        .collect::<BTreeSet<_>>();
    let mut linked = BTreeSet::new();
    for candidate in linked_commits {
        let candidate = candidate.trim().to_ascii_lowercase();
        if observed_shas.contains(&candidate) {
            linked.insert(candidate);
            continue;
        }
        if !(7..=64).contains(&candidate.len())
            || !candidate.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            continue;
        }
        let mut matches = observed_shas
            .iter()
            .filter(|sha| sha.starts_with(&candidate));
        if let Some(found) = matches.next()
            && matches.next().is_none()
        {
            linked.insert(found.clone());
        }
    }
    linked
}

fn workflow_binding(
    mut snapshot: CoordinationGitSnapshot,
    kind: AgentWorkflowKind,
    target_branch: &str,
) -> Result<WorkflowBinding, CoordinationError> {
    if snapshot.bare
        || snapshot.root.is_empty()
        || snapshot.git_dir.is_empty()
        || snapshot.common_git_dir.is_empty()
        || snapshot.branch.is_empty()
        || !valid_workflow_head(&snapshot.head)
    {
        return Err(CoordinationError::WorkflowStale);
    }
    let target_head = if matches!(
        kind,
        AgentWorkflowKind::PullRequest | AgentWorkflowKind::Merge
    ) {
        snapshot
            .branches
            .iter()
            .find(|branch| {
                branch.name == target_branch
                    && branch.name != snapshot.branch
                    && valid_workflow_head(&branch.head)
            })
            .map(|branch| branch.head.clone())
            .ok_or(CoordinationError::WorkflowTarget)?
    } else {
        if !target_branch.is_empty() {
            return Err(CoordinationError::WorkflowTarget);
        }
        String::new()
    };
    snapshot.changed_paths.sort();
    snapshot.untracked_paths.sort();
    let digest_input = WorkflowDigestInput {
        root: &snapshot.root,
        git_dir: &snapshot.git_dir,
        common_git_dir: &snapshot.common_git_dir,
        branch: &snapshot.branch,
        head: &snapshot.head,
        upstream: &snapshot.upstream,
        target_branch,
        target_head: &target_head,
        detached: snapshot.detached,
        initial: snapshot.initial,
        status: &snapshot.status,
        changed: &snapshot.changed_paths,
        untracked: &snapshot.untracked_paths,
        changed_more: snapshot.changed_more,
        untracked_more: snapshot.untracked_more,
        divergence: &snapshot.divergence,
    };
    let encoded = serde_json::to_vec(&digest_input)
        .map_err(|error| CoordinationError::Message(error.to_string()))?;
    let digest = Sha256::digest(encoded);
    Ok(WorkflowBinding {
        root: snapshot.root,
        git_dir: snapshot.git_dir,
        common_git_dir: snapshot.common_git_dir,
        branch: snapshot.branch,
        head: snapshot.head,
        target_branch: target_branch.to_owned(),
        target_head,
        status: snapshot.status,
        digest: format!("{digest:x}"),
    })
}

fn valid_workflow_head(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn binding_matches_worktree(binding: &WorkflowBinding, worktree: &WorktreeIdentity) -> bool {
    binding.root == worktree.root
        && binding.git_dir == worktree.git_dir
        && binding.common_git_dir == worktree.common_git_dir
        && binding.head == worktree.head
        && binding.branch == worktree.branch
}

#[allow(clippy::too_many_arguments)]
fn project_workflow(
    id: &str,
    generation: u64,
    kind: AgentWorkflowKind,
    state: AgentWorkflowState,
    run_id: &str,
    association: Option<RuntimeAssociation>,
    worktree: Option<&WorktreeIdentity>,
    project_root: &Path,
    binding: &WorkflowBinding,
    created_at: Timestamp,
    expires_at: Timestamp,
    approved_at: Option<Timestamp>,
) -> AgentWorkflowProposalV2 {
    AgentWorkflowProposalV2 {
        id: id.to_owned(),
        generation,
        kind,
        state,
        run_id: run_id.to_owned(),
        association,
        worktree_root: worktree.map_or_else(String::new, |value| value.root.clone()),
        isolated: worktree.is_some_and(|value| clean_path(&value.root) != clean_path(project_root)),
        branch: binding.branch.clone(),
        head: binding.head.clone(),
        target_branch: binding.target_branch.clone(),
        target_head: binding.target_head.clone(),
        status: binding.status.clone(),
        created_at,
        expires_at,
        approved_at,
        notice: AGENT_WORKFLOW_NOTICE.to_owned(),
    }
}

fn same_worktree_repository(left: &WorktreeIdentity, right: &WorktreeIdentity) -> bool {
    left.root == right.root
        && left.git_dir == right.git_dir
        && left.common_git_dir == right.common_git_dir
        && left.linked == right.linked
}

fn path_inside(root: &Path, candidate: &Path) -> bool {
    let Ok(relative) = candidate.strip_prefix(root) else {
        return false;
    };
    !relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
}

fn clean_path(path: impl AsRef<Path>) -> PathBuf {
    let mut cleaned = PathBuf::new();
    for component in path.as_ref().components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                cleaned.pop();
            }
            _ => cleaned.push(component),
        }
    }
    cleaned
}

fn registry_coordination_error(error: RegistryError) -> CoordinationError {
    match error {
        RegistryError::RunNotFound => CoordinationError::RunNotFound,
        other => CoordinationError::Message(other.to_string()),
    }
}

fn ensure_projection_deadline(deadline: Option<Instant>) -> Result<(), CoordinationError> {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        Err(CoordinationError::DeadlineExceeded)
    } else {
        Ok(())
    }
}

fn raw_url_base64(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        result.push(char::from(RAW_URL_ALPHABET[usize::from(first >> 2)]));
        result.push(char::from(
            RAW_URL_ALPHABET[usize::from(((first & 3) << 4) | (second >> 4))],
        ));
        if chunk.len() > 1 {
            result.push(char::from(
                RAW_URL_ALPHABET[usize::from(((second & 15) << 2) | (third >> 6))],
            ));
        }
        if chunk.len() > 2 {
            result.push(char::from(RAW_URL_ALPHABET[usize::from(third & 63)]));
        }
    }
    result
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

const fn is_unset_event_kind(value: &crate::EventKind) -> bool {
    matches!(value, crate::EventKind::Unset)
}

const fn is_unset_event_phase(value: &crate::EventPhase) -> bool {
    matches!(value, crate::EventPhase::Unset)
}

const fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}
