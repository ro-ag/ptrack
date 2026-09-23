// The desktop command surface `tauri-bridge.js` installs as
// `window.go.gui.App`, typed the way the window controllers call it. Replies
// the frontend normalizes itself (layout, preferences, recent projects, the
// first-run journey) stay `unknown` here and are parsed by their modules;
// replies read field by field are described in `workspace/snapshot-types.ts`.

import type { UpdateState } from "./updates/controller";
import type { TerminalAssociationMutationResult } from "./terminal/association-editor";
import type { Scratchpad } from "./terminal/scratchpad";
import type {
  TerminalWritebackKind,
  TerminalWritebackPreview,
  TerminalWritebackResult,
} from "./terminal/writeback";
import type { AssociationPointerV1, WorkspaceTab } from "./workspace/model";
import type { TerminalCWDValidation } from "./workspace/persistence";
import type { FirstPlanJourneyApi } from "./workspace/first-plan";
import type { FirstRunJourneyApi } from "./workspace/first-run-journey";
import type {
  ActivityHeatmapDay,
  AgentHandoffPreviewResult,
  DeletePlanResponse,
  GenerationReply,
  GlobalOverviewRefreshResult,
  IssueDetailResponse,
  IssueMutationResult,
  IssuesResponse,
  ListProjectsResponse,
  MoveTaskResponse,
  PlanCompletionResponse,
  PlanMutationResponse,
  ProjectTimeline,
  SearchResult,
  StackProfile,
  TaskDetailResponse,
  TerminalProfileSummary,
  TerminalSessionReply,
  TerminalStreamClaim,
  TerminalWindowAssignment,
  WorkspaceCloseResult,
  WorkspaceSnapshot,
  WorkspaceStateResponse,
} from "./workspace/snapshot-types";
import type { Overview } from "./workspace/overview";

export interface Backend extends FirstRunJourneyApi, FirstPlanJourneyApi {
  // Workspace lifecycle.
  GetWorkspaceState(): Promise<WorkspaceStateResponse>;
  GetPendingInitializationV1(): Promise<unknown>;
  PickProjectDirectory(purpose: string): Promise<unknown>;
  CloseProject(confirmationToken: string): Promise<WorkspaceCloseResult>;
  PreviewProjectGuideV1(request: { operationId: string; root: string }): Promise<unknown>;

  // Recent projects and the landing overview.
  GetRecentProjectsV1(): Promise<unknown>;
  ResolveRecentProjectV1(entryId: string, base: string, path: string): Promise<unknown>;
  OpenRecentProjectV1(
    entryId: string,
    base: string,
    canonicalRoot: string,
    resolutionToken: string,
    confirmationToken: string,
  ): Promise<unknown>;
  ForgetRecentProjectV1(entryId: string, base: string): Promise<unknown>;
  GetGlobalOverviewV1(): Promise<Overview>;
  RefreshGlobalOverviewV1(): Promise<GlobalOverviewRefreshResult>;

  // Snapshot and Overview.
  GetWorkspaceSnapshot(generation: number, planId: number | null): Promise<WorkspaceSnapshot>;
  GetStackProfileV1(rescan: boolean): Promise<StackProfile>;
  GetActivityHeatmapV2(weeks: number): Promise<ActivityHeatmapDay[]>;
  GetProjectTimelineV1(): Promise<ProjectTimeline>;
  SearchV2(query: string): Promise<SearchResult[]>;

  // Plans.
  AddPlanV1(generation: number, title: string): Promise<PlanMutationResponse>;
  RenamePlanV1(generation: number, planId: number, title: string): Promise<GenerationReply>;
  CompletePlanV1(generation: number, planId: number): Promise<PlanCompletionResponse>;
  HoldPlanV1(generation: number, planId: number, reason: string): Promise<GenerationReply>;
  ResumePlanV1(generation: number, planId: number): Promise<GenerationReply>;
  ReopenPlanV1(generation: number, planId: number): Promise<GenerationReply>;
  DeletePlanV1(
    generation: number,
    planId: number,
    confirm: boolean,
    previewRevision?: string,
  ): Promise<DeletePlanResponse>;
  MovePlanV1(generation: number, planId: number, targetPath: string, title: string): Promise<GenerationReply>;
  CopyPlanV1(generation: number, planId: number, targetPath: string, title: string): Promise<GenerationReply>;
  ListProjectsV1(generation: number): Promise<ListProjectsResponse>;
  /** Makes `planId` the desktop user's current plan; 0 clears it. */
  SetActivePlanV1(generation: number, planId: number): Promise<GenerationReply>;

  // Tasks.
  AddTaskV2(generation: number, planId: number, title: string): Promise<GenerationReply>;
  RenameTaskV2(generation: number, taskId: number, title: string): Promise<GenerationReply>;
  AddTaskNoteV2(generation: number, taskId: number, note: string): Promise<GenerationReply>;
  MoveTaskV3(
    generation: number,
    taskId: number,
    status: string,
    confirmationToken: string,
  ): Promise<MoveTaskResponse>;
  GetTaskDetailV2(generation: number, taskId: number): Promise<TaskDetailResponse>;

  // Issues.
  GetIssuesV1(generation: number, filter: string, offset: number): Promise<IssuesResponse>;
  GetIssueDetailV1(generation: number, issueId: number, query?: string): Promise<IssueDetailResponse>;
  AddIssueV1(generation: number, title: string, body: string, severity: string): Promise<IssueMutationResult>;
  UpdateIssueV1(
    generation: number,
    issueId: number,
    title: string,
    body: string,
    severity: string,
    status: string,
    expectedUpdatedAt: string,
  ): Promise<IssueMutationResult>;
  ScheduleIssueV1(generation: number, issueId: number, planId: number, title: string): Promise<IssueMutationResult>;
  SetIssueTaskV1(
    generation: number,
    issueId: number,
    expectedTaskId: number,
    taskId: number,
  ): Promise<IssueMutationResult>;
  MoveIssueTaskV1(
    generation: number,
    issueId: number,
    taskId: number,
    fromPlanId: number,
    toPlanId: number,
  ): Promise<IssueMutationResult>;

  // Agents.
  SetAgentTaskOwnershipV2(generation: number, runId: string, revision: number, owned: boolean): Promise<GenerationReply>;
  SetAgentWorktreeV2(
    generation: number,
    runId: string,
    revision: number,
    root: string,
    associate: boolean,
  ): Promise<GenerationReply>;
  SendAgentHandoffV2(
    generation: number,
    sourceRunId: string,
    targetRunId: string,
    sourceRevision: number,
    targetRevision: number,
  ): Promise<GenerationReply>;
  AcknowledgeAgentHandoffV2(generation: number, handoffId: string, targetRunId: string): Promise<GenerationReply>;
  PreviewAgentHandoffV2(generation: number, runId: string): Promise<AgentHandoffPreviewResult>;
  PrepareAgentWorkflowV2(
    generation: number,
    runId: string,
    revision: number,
    kind: string,
    target: string,
  ): Promise<GenerationReply>;
  ApproveAgentWorkflowV2(generation: number, proposalId: string): Promise<GenerationReply>;
  DismissAgentWorkflowV2(generation: number, proposalId: string): Promise<GenerationReply>;

  // Terminals.
  GetTerminalProfiles(): Promise<TerminalProfileSummary[]>;
  GetTerminalProfilesV2(generation: number): Promise<{ generation: number; profiles: TerminalProfileSummary[] }>;
  CreateTerminalV2(
    generation: number,
    profileId: string,
    cwd: string,
    rows: number,
    columns: number,
  ): Promise<TerminalSessionReply>;
  LaunchLinkedAgentV2(
    generation: number,
    profileId: string,
    cwd: string,
    rows: number,
    columns: number,
    association: AssociationPointerV1,
  ): Promise<TerminalSessionReply>;
  RollbackLinkedAgentLaunchV2(generation: number, sessionId: string): Promise<void>;
  OpenTerminalWindow(sessions: readonly string[], shape: WorkspaceTab): Promise<{ label: string }>;
  GetTerminalWindowTab(label: string): Promise<TerminalWindowAssignment | null>;
  SetTerminalWindowTab(label: string, sessions: string[], shape: unknown): Promise<void>;
  ClaimTerminalStream(sessionId: string, fromSequence: number): Promise<TerminalStreamClaim>;
  MutateTerminalAssociationV2(
    generation: number,
    sessionId: string,
    expectedRevision: number,
    detach: boolean,
    association: AssociationPointerV1 | { version: 1 },
  ): Promise<TerminalAssociationMutationResult & { generation: number }>;
  PreviewTerminalWritebackV2(
    generation: number,
    sessionId: string,
    expectedRevision: number,
    kind: TerminalWritebackKind,
    content: string,
  ): Promise<TerminalWritebackPreview & GenerationReply>;
  WriteTerminalMemoryV2(
    generation: number,
    sessionId: string,
    expectedRevision: number,
    requestId: string,
    kind: TerminalWritebackKind,
    content: string,
    confirmSummary: boolean,
  ): Promise<TerminalWritebackResult & GenerationReply>;
  ValidateTerminalCWDsV2(
    generation: number,
    cwds: string[],
  ): Promise<{ generation: number; results: TerminalCWDValidation[] }>;
  ResizeTerminalV2(generation: number, sessionId: string, rows: number, columns: number): Promise<void>;
  CloseTerminalV2(generation: number, sessionId: string, force: boolean): Promise<void>;
  GetScratchpadV1(generation: number): Promise<{ generation: number; scratchpad: Scratchpad }>;
  SetScratchpadV1(
    generation: number,
    revision: number,
    scratchpad: Scratchpad,
  ): Promise<{ generation: number; revision: number }>;

  // Application.
  GetPreferences(): Promise<unknown>;
  SetPreferences(patch: unknown): Promise<unknown>;
  ResetPreferences(): Promise<unknown>;
  GetLayoutState(): Promise<unknown>;
  SetLayoutState(patch: Record<string, unknown>): Promise<unknown>;
  ResetWindowLayout(): Promise<unknown>;
  ResetApplicationState(): Promise<unknown>;
  GetDiagnosticsReport(): Promise<unknown>;
  OpenHelpDestination(destination: string): Promise<void>;
  InstallShellCommand(): Promise<void>;

  // Updates.
  GetUpdateState(): Promise<UpdateState>;
  CheckForUpdates(): Promise<UpdateState>;
  DownloadUpdate(version: string): Promise<UpdateState>;
  ApplyUpdate(version: string): Promise<UpdateState>;
  CancelUpdateOperation(): Promise<UpdateState>;
  SetAutomaticUpdateChecks(enabled: boolean): Promise<UpdateState>;
}

/** The desktop runtime helpers the bridge installs beside the commands. */
export interface DesktopRuntime {
  EventsOnMultiple?(
    name: string,
    callback: (payload: unknown) => void,
    maxCallbacks: number,
  ): () => void;
  BrowserOpenURL?(url: string): Promise<void>;
  ClipboardGetText?(): Promise<string>;
  ClipboardSetText?(text: string): Promise<boolean>;
}

declare global {
  interface Window {
    go?: { gui?: { App?: Backend } };
    runtime?: DesktopRuntime;
  }
}
