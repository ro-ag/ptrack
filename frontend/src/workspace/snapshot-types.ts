// Wire shapes of the desktop replies the window controllers read field by
// field. They mirror the runtime's serde views (camelCase); optional fields
// are the ones the runtime skips when empty or that older runtimes omit.

import type { TaskStatus } from "./task-status";
import type { WorkspaceStatus } from "./controller";
import type {
  AgentActivitySectionPresentation,
  LinkedTaskRuntimeSummary,
  PaletteResult,
  RuntimeAssociationSummary,
  StackProfileProject,
} from "./presentation";
import type { Overview } from "./overview";
import type { ProjectTimeline as TimelinePayload } from "./project-timeline";
import type { DeletePreviewSummary, ProjectChoice } from "./plan-lifecycle";
import type { WorkspaceTab } from "./model";
import type { DiscoveredTerminalProfile } from "../terminal/linked-launch";
import type { TaskTransitionResult } from "./task-transition";

/** Replies to generation-fenced commands carry the generation they ran in. */
export interface GenerationReply {
  generation: number;
}

export interface WorkspaceProject {
  root: string;
  name?: string;
}

export interface WorkspaceStateResponse {
  status: WorkspaceStatus;
  generation: number;
  version?: string;
  project?: WorkspaceProject;
  error?: string;
}

export interface ActiveResources {
  terminals: number;
  agentRuns: number;
  pendingAdmissions?: number;
}

export interface WorkspaceCloseResult {
  state: WorkspaceStateResponse;
  requiresConfirmation?: boolean;
  confirmationToken: string;
  activeResources: ActiveResources;
  warning?: string;
}

// ------------------------------------------------------------------ board

export interface BoardPlan {
  id: number;
  title: string;
  status: string;
  isActive?: boolean;
  tasksTotal: number;
  tasksDone: number;
  holdReason?: string;
  claimedBy?: string;
  depsOpen?: number[];
}

export interface BoardTask {
  id: number;
  title: string;
  status: TaskStatus;
  updatedAt: string;
  planId?: number;
  noteCount?: number;
  commitCount?: number;
  issueCount?: number;
  latestNote?: string;
  holdReason?: string;
  linkedRuntime?: LinkedTaskRuntimeSummary;
  deps?: number[];
  depsOpen?: number[];
}

export interface BoardColumn {
  status: TaskStatus;
  title: string;
  tasks: BoardTask[];
}

export interface BoardStats {
  planTasks: number;
  planTasksDone: number;
  tasksOpen: number;
  tasksBlocked: number;
  notes: number;
  commits: number;
  openIssues: number;
  tasks: number;
  tasksDone: number;
  plans: number;
  plansDone: number;
  milestones: number;
  milestonesDone: number;
}

export interface BoardActivity {
  kind: string;
  title: string;
  detail: string;
  target: string;
  occurredAt: string;
}

export interface BoardIssue {
  id: number;
  title: string;
  severity: string;
  taskId: number;
}

export interface Board {
  projectName: string;
  goal: string;
  summary: string;
  /** When the rolling summary was last written (RFC 3339, UTC); null if never. */
  summaryUpdatedAt?: string | null;
  plans: BoardPlan[];
  planId: number;
  planTitle: string;
  columns: BoardColumn[];
  stats: BoardStats;
  activity: BoardActivity[];
  openIssues: BoardIssue[];
}

// --------------------------------------------------------------- snapshot

export interface SnapshotBound {
  shown: number;
  total: number;
  more?: boolean;
}

export interface ProjectStorage {
  exists: boolean;
  formatVersion?: number;
  sizeBytes?: number;
  lastWriteVersion?: string;
  error?: string;
}

export interface TrackingNote {
  kind?: string;
  target: string;
  targetId?: number;
  body: string;
  occurredAt: string;
}

export interface SnapshotTracking {
  state?: string;
  board: Board;
  blockers: BoardTask[];
  notes: TrackingNote[];
  bounds?: Record<string, SnapshotBound>;
}

export interface GitRemote {
  name: string;
  fetchUrls?: string[];
  pushUrls?: string[];
}

export interface GitBranch {
  name: string;
  current?: boolean;
  remote?: boolean;
  stale?: boolean;
  worktreePath?: string;
  lastCommitAt?: string;
}

export interface GitCommit {
  sha: string;
  subject: string;
  authorName: string;
  date: string;
  filesChanged: number;
  changedAreas?: { name: string; files: number }[];
  refs?: string[];
}

export interface GitStatus {
  detached?: boolean;
  oid?: string | null;
  branch?: string | null;
  staged?: number;
  unstaged?: number;
  untracked?: number;
  conflicted?: number;
  upstream?: string | null;
  ahead?: number;
  behind?: number;
}

export interface GitSnapshot {
  state?: string;
  status: GitStatus;
  linkedWorktree?: boolean;
  divergence?: { ahead?: number; behind?: number } | null;
  unpushedCommits?: unknown[];
  remotes?: GitRemote[];
  localBranches?: GitBranch[];
  remoteBranches?: GitBranch[];
  recentCommits?: GitCommit[];
}

export interface GitSection {
  state: string;
  error?: string;
  snapshot: GitSnapshot;
}

export interface WorkspaceSnapshot {
  generation: number;
  capturedAt: string;
  project: { name: string; root: string; storage: ProjectStorage };
  tracking: SnapshotTracking;
  git: GitSection;
  agentActivity: AgentActivitySectionPresentation & { items?: AgentActivityItem[] };
  drift: unknown;
}

/** An agent run as the snapshot's agent-activity section lists it. */
export interface AgentActivityItem {
  runId: string;
  live?: boolean;
  association?: RuntimeAssociationSummary & { revision?: number };
}

// ---------------------------------------------------------------- overview

export interface StackProfile {
  state: "ready" | "scanning" | "unavailable" | "failed";
  trackedFiles?: number;
  lines?: number;
  linesCounted?: boolean;
  incomplete?: boolean;
  scannedHead?: string;
  projects?: StackProfileProject[];
}

export interface ActivityHeatmapDay {
  date: string;
  count: number;
}

export type ProjectTimeline = TimelinePayload;

export interface GlobalOverviewRefreshResult {
  overview: Overview;
  refreshedProjects: number;
  skippedProjects: number;
}

// ------------------------------------------------------------------ issues

export interface IssueSummary {
  id: number;
  title: string;
  severity: string;
  status: string;
  updatedAt: string;
  taskId: number;
  planId: number;
  planTitle: string;
}

export interface IssuesResponse {
  generation: number;
  issues: IssueSummary[];
  offset?: number;
  bounds?: { shown?: number; total?: number };
}

export interface IssueRecord extends IssueSummary {
  body?: string;
  taskTitle?: string;
}

export interface IssueTargetPlan {
  id: number;
  title: string;
  holdReason?: string;
}

export interface IssueTargetTask {
  id: number;
  title: string;
  planId: number;
  status: string;
}

export interface IssueDetailResponse {
  generation: number;
  issue: IssueRecord;
  plans?: IssueTargetPlan[];
  tasks?: IssueTargetTask[];
}

export interface IssueMutationResult {
  generation: number;
  issue: { id: number };
}

// ------------------------------------------------------------------- tasks

export interface TaskNote {
  kind?: string;
  body: string;
  occurredAt: string;
}

export interface TaskCommit {
  sha: string;
  subject: string;
  occurredAt: string;
}

export interface TaskIssue {
  id: number;
  title: string;
  severity: string;
  status?: string;
}

export interface TerminalRuntimeRow {
  sessionId?: string;
  profileKind: string;
  state: string;
  live: boolean;
}

export interface AgentRuntimeRow {
  runId: string;
  live: boolean;
  state: string;
  processState: string;
  leaseState: string;
  terminalBacked: boolean;
  correspondingTerminal?: boolean;
  terminalPresent?: boolean;
  intelligence?: { state?: unknown; confidence?: unknown; eventCount?: unknown };
}

export interface LinkedRuntimeDetail {
  summary?: LinkedTaskRuntimeSummary;
  terminals?: TerminalRuntimeRow[];
  agents?: AgentRuntimeRow[];
  terminalRowsMore?: number;
  agentRowsMore?: number;
}

export interface AgentIntelligenceEntry {
  runId: string;
  association?: RuntimeAssociationSummary;
  intelligence: { state: string; confidence?: string };
  eventBounds?: { total?: number };
  suggestions?: { kind: string; label: string; reason: string }[];
}

export interface TaskDetailResponse {
  generation: number;
  task: BoardTask;
  notes: TaskNote[];
  commits: TaskCommit[];
  issues: TaskIssue[];
  linkedRuntime?: LinkedRuntimeDetail;
  agentIntelligence?: AgentIntelligenceEntry[];
}

export type MoveTaskResponse = TaskTransitionResult;

export interface AgentHandoffPreviewResult {
  generation: number;
  association?: RuntimeAssociationSummary;
  preview: { text: string };
}

// ------------------------------------------------------------------- plans

export interface PlanMutationResponse {
  generation: number;
  plan: { id: number; title?: string };
}

export interface PlanCompletionResponse {
  generation: number;
  checkpoint: { markdown: string; openPlans?: { id: number }[] };
}

export interface DeletePlanResponse {
  generation: number;
  previewRevision?: string;
  summary: DeletePreviewSummary;
}

export interface ListProjectsResponse {
  generation: number;
  projects?: ProjectChoice[];
}

// ------------------------------------------------------------------ search

export type SearchResult = PaletteResult;

// --------------------------------------------------------------- terminals

export type TerminalProfileSummary = DiscoveredTerminalProfile;

export interface TerminalSessionReply {
  generation: number;
  sessionId: string;
  profileId: string;
  cwd: string;
  state: string;
  streamUrl: string;
  associationRevision?: number;
  linkedLaunch?: boolean;
}

export interface TerminalStreamClaim {
  url: string;
  fromSequence: number;
  gap: boolean;
  state?: string;
}

export interface TerminalWindowShape extends WorkspaceTab {
  windowTabs?: WorkspaceTab[];
  activeWindowTabId?: string;
}

export interface TerminalWindowAssignment {
  sessions?: string[];
  shape: TerminalWindowShape;
}
