import "./tauri-bridge";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Terminal } from "@xterm/xterm";
import { mountTerminalDock } from "./terminal/pane";
import { TerminalStreamClient } from "./terminal/client";
import {
  binaryStringToBytes,
  commitClipboardPaste,
  prepareClipboardPaste,
  splitTerminalInput,
  terminalShortcutAction,
  terminalTextToBytes,
} from "./terminal/paste";
import { terminalSearchResultLabel } from "./terminal/search";
import {
  normalizeTerminalProfileSettings,
  terminalRendererOptions,
} from "./terminal/profile-settings";
import {
  reclaimStream,
  reclaimingStreamNotice,
  streamReclaimFailedNotice,
  terminalGapNotice,
  terminalWindowLabel,
  terminalWindowStatusLabel,
} from "./terminal/pop-out";
import {
  linkedAssociationPointer,
  selectedInstalledAgentProfile,
} from "./terminal/linked-launch";
import { findTerminalPane, paneIds } from "./workspace/model";
import {
  WorkspaceTabController,
  createCryptoIdFactory,
} from "./workspace/tab-controller";
import { WorkspaceSplitView } from "./workspace/split-view";
import {
  stableTerminalWritebackRequestID,
  terminalWritebackContentPolicy,
} from "./terminal/writeback";
import { THEME_STORAGE_KEY, initTheme, resolveTheme } from "./theme";
import {
  applyPreferenceMirrors,
  defaultPreferences,
  preferenceSaveMessage,
  preferencesFromMirrors,
  preferencesResponse,
  readTerminalPreferenceOverrides,
  storageStatusNotice,
} from "./settings/preferences";
import {
  clampTerminalFontSize,
  readTerminalProfileFontSize,
  writeTerminalProfileFontSize,
} from "./terminal/preferences";
import {
  diagnosticsRows,
  nextSettingsSectionIndex,
  resetApplicationStateConfirmation,
  resetApplicationStateMessage,
  resetWindowLayoutConfirmation,
  settingsPanelId,
  settingsSectionIndex,
  settingsSections,
  settingsTabId,
} from "./settings/sections";
import {
  formatUpdateBytes,
  updateModalOpenTransition,
  updatePresentation,
  updateProgress,
  updateStateIsNewer,
} from "./updates/presentation";
import {
  clampSidebarWidth,
  defaultLayoutState,
  defaultSidebarWidth,
  layoutProjectState,
  layoutStatePatch,
  normalizeLayoutState,
  sidebarHiddenStorageKey,
  sidebarMaximumWidth,
  sidebarWidthFromKey,
  sidebarWidthStorageKey,
  storedSidebarWidth,
} from "./workspace/layout";
import {
  terminalWorkspaceStoragePrefix,
  WorkspacePersistenceScheduler,
} from "./workspace/persistence";
import {
  applicationOverlayKeyboardPolicy,
  ApplicationOverlayCoordinator,
} from "./workspace/application-overlay";
import {
  nativeMenuCommandAllowed,
  nativeMenuViewTarget,
  registerNativeMenuActions,
} from "./workspace/native-menu";
import {
  clampMenuPosition,
  deleteConfirmationText,
  planMenuItems,
  transferSubmitDisabled,
} from "./workspace/plan-lifecycle";
import {
  RefreshGate,
  RefreshLoop,
  RuntimeRefreshCoalescer,
  WorkspaceController,
} from "./workspace/controller";
import {
  canOpenPreservedFirstRunProject,
  completedInitializationWorkspaceMatches,
  firstRunFocusTarget,
  initializationFailureMessage,
  initializationStatusMatchesOperation,
  initialFirstRunState,
  isProjectGuidePartiallyApplied,
  isProjectGuidePreviewStale,
  pendingInitializationEvent,
  parseProjectGuidePreview,
  PROJECT_GUIDANCE_UNAVAILABLE,
  projectGuideCommitFields,
  reduceFirstRun,
  resolveFirstRunStartupState,
  validateNorthStarGoal,
} from "./workspace/first-run";
import {
  commitInitialization,
  initializeProjectRequest,
  openExactProject as runExactProjectOpen,
  readInitializationStatus,
  resumeInitialization,
  validateInitializationTarget,
} from "./workspace/first-run-journey";
import {
  createFirstPlan,
  createFirstTask,
  firstPlanExitFocusTarget,
  firstPlanFocusTarget,
  initialFirstPlanState,
  reduceFirstPlan,
  startFirstTask as runStartFirstTask,
  validateOnboardingTitle,
} from "./workspace/first-plan";
import {
  focusAfterForgottenProject,
  initialRecentProjectsState,
  parseForgetRecentProjectResult,
  parseRecentProjectOpenResult,
  parseRecentProjectResolution,
  parseRecentProjects,
  preselectedRecentProject,
  RECENT_RELOCATION_UNCONFIRMED,
  recentProjectFocusKey,
  recentProjectPrimaryAction,
  reduceRecentProjects,
} from "./workspace/recent-projects";
import {
  taskTransitionCanStart,
  taskTransitionConfirmationCopy,
  taskTransitionFocusIntent,
  taskTransitionResponseIsCurrent,
} from "./workspace/task-transition";
import {
  agentActivityAnnouncement,
  agentIntelligenceLabel,
  agentActivityPresentation,
  driftPresentation,
  appVersionLabel,
  collapsedLaneStatuses,
  commandShortcut,
  confirmationCopy,
  durableProjectGuideReviewCopy,
  firstRunRecoveryActions,
  focusCycleIndex,
  groupSearchResults,
  handoffPreviewResponseIsCurrent,
  heatmapWeeks,
  linkedTaskRuntimePresentation,
  mutationFocusFallback,
  paletteTarget,
  preserveSectionOnError,
  postProjectOnboardingActions,
  projectGuideRecoveryCopy,
  projectGuideReviewCopy,
  runtimeAssociationLabel,
  runtimeEventIsCurrent,
  shortcutIntent,
  workflowMutationFocusKey,
  worktreeSelectionForRerender,
  workspaceStateCopy,
} from "./workspace/presentation";

const statuses = ["todo", "doing", "blocked", "done"];
const laneColors = {
  todo: "var(--todo)",
  doing: "var(--doing)",
  blocked: "var(--blocked)",
  done: "var(--done)",
};
const severityColors = {
  low: "var(--text-soft)",
  medium: "var(--info)",
  high: "var(--doing)",
  critical: "var(--blocked)",
};

const elements = {
  app: document.querySelector("#app"),
  sidebar: document.querySelector("#sidebar"),
  sidebarResize: document.querySelector("#sidebar-resize"),
  sidebarToggle: document.querySelector("#sidebar-toggle"),
  panelControls: document.querySelector(".panel-controls"),
  boardPanelToggle: document.querySelector("#board-panel-toggle"),
  terminalPanelToggle: document.querySelector("#terminal-panel-toggle"),
  workspace: document.querySelector("#workspace"),
  overviewPage: document.querySelector("#overview-page"),
  overviewHeading: document.querySelector("#overview-heading"),
  navBoard: document.querySelector("#nav-board"),
  navOverview: document.querySelector("#nav-overview"),
  stateScreen: document.querySelector("#workspace-state-screen"),
  stateCard: document.querySelector("#project-state-card"),
  welcomePanel: document.querySelector("#welcome-panel"),
  stateEyebrow: document.querySelector("#workspace-state-eyebrow"),
  stateHeading: document.querySelector("#workspace-state-heading"),
  stateDetail: document.querySelector("#workspace-state-detail"),
  stateInitialize: document.querySelector("#state-initialize-project-button"),
  stateOpen: document.querySelector("#state-open-project-button"),
  recents: document.querySelector("#recent-project-list"),
  recentHeading: document.querySelector("#recent-project-heading"),
  recentStatus: document.querySelector("#recent-project-status"),
  recentError: document.querySelector("#recent-project-error"),
  setupPanel: document.querySelector("#setup-panel"),
  setupProgress: document.querySelector("#setup-progress"),
  setupEyebrow: document.querySelector("#setup-eyebrow"),
  setupHeading: document.querySelector("#setup-heading"),
  setupDetail: document.querySelector("#setup-detail"),
  setupOperation: document.querySelector("#setup-operation"),
  setupTargetSummary: document.querySelector("#setup-target-summary"),
  setupTarget: document.querySelector("#setup-target"),
  setupGoalForm: document.querySelector("#setup-goal-form"),
  setupGoal: document.querySelector("#setup-goal"),
  setupGoalError: document.querySelector("#setup-goal-error"),
  setupGoalBack: document.querySelector("#setup-goal-back"),
  setupGoalCancel: document.querySelector("#setup-goal-cancel"),
  setupGuide: document.querySelector("#setup-guide"),
  setupGuideDefaultChoice: document.querySelector("#setup-guide-default-choice"),
  setupGuidePreview: document.querySelector("#setup-guide-preview"),
  setupGuideFiles: document.querySelector("#setup-guide-files"),
  setupGuideDefaultActions: document.querySelector("#setup-guide-default-actions"),
  setupGuideSkip: document.querySelector("#setup-guide-skip"),
  setupGuidePreviewButton: document.querySelector("#setup-guide-preview-button"),
  setupGuideBack: document.querySelector("#setup-guide-back"),
  setupGuideCancel: document.querySelector("#setup-guide-cancel"),
  setupGuideInstallActions: document.querySelector("#setup-guide-install-actions"),
  setupGuideInstall: document.querySelector("#setup-guide-install"),
  setupGuidePreviewSkip: document.querySelector("#setup-guide-preview-skip"),
  setupGuidePreviewBack: document.querySelector("#setup-guide-preview-back"),
  setupGuidePreviewCancel: document.querySelector("#setup-guide-preview-cancel"),
  setupGuideStaleActions: document.querySelector("#setup-guide-stale-actions"),
  setupGuideReviewAgain: document.querySelector("#setup-guide-review-again"),
  setupGuideStaleSkip: document.querySelector("#setup-guide-stale-skip"),
  setupGuideStaleBack: document.querySelector("#setup-guide-stale-back"),
  setupGuideStaleCancel: document.querySelector("#setup-guide-stale-cancel"),
  setupReview: document.querySelector("#setup-review"),
  setupStorageSummary: document.querySelector("#setup-storage-summary"),
  setupUntouchedRoot: document.querySelector("#setup-untouched-root"),
  setupReviewGoal: document.querySelector("#setup-review-goal"),
  setupReviewGuideChoice: document.querySelector("#setup-review-guide-choice"),
  setupReviewGuideDetail: document.querySelector("#setup-review-guide-detail"),
  setupReviewGuideChanges: document.querySelector("#setup-review-guide-changes"),
  setupCompleteChanges: document.querySelector("#setup-complete-changes"),
  setupReviewBack: document.querySelector("#setup-review-back"),
  setupReviewCancel: document.querySelector("#setup-review-cancel"),
  setupCommit: document.querySelector("#setup-commit"),
  setupExistingActions: document.querySelector("#setup-existing-actions"),
  setupOpenExisting: document.querySelector("#setup-open-existing"),
  setupExistingChoose: document.querySelector("#setup-existing-choose"),
  setupExistingCancel: document.querySelector("#setup-existing-cancel"),
  setupNewTargetActions: document.querySelector("#setup-new-target-actions"),
  setupNewTargetContinue: document.querySelector("#setup-new-target-continue"),
  setupNewTargetChoose: document.querySelector("#setup-new-target-choose"),
  setupNewTargetCancel: document.querySelector("#setup-new-target-cancel"),
  setupRecoveryActions: document.querySelector("#setup-recovery-actions"),
  setupRetry: document.querySelector("#setup-retry"),
  setupResume: document.querySelector("#setup-resume"),
  setupOpenRecovery: document.querySelector("#setup-open-recovery"),
  setupRecoveryHelp: document.querySelector("#setup-recovery-help"),
  setupRecoveryChoose: document.querySelector("#setup-recovery-choose"),
  setupReturnWelcome: document.querySelector("#setup-return-welcome"),
  setupUncertainActions: document.querySelector("#setup-uncertain-actions"),
  setupCheckStatus: document.querySelector("#setup-check-status"),
  setupStatus: document.querySelector("#setup-status"),
  setupError: document.querySelector("#setup-error"),
  onboarding: document.querySelector("#post-project-onboarding"),
  onboardingProgress: document.querySelector("#onboarding-progress"),
  onboardingHeading: document.querySelector("#onboarding-heading"),
  onboardingDetail: document.querySelector("#onboarding-detail"),
  onboardingOperation: document.querySelector("#onboarding-operation"),
  onboardingPlanForm: document.querySelector("#onboarding-plan-form"),
  onboardingPlanTitle: document.querySelector("#onboarding-plan-title"),
  onboardingPlanError: document.querySelector("#onboarding-plan-error"),
  onboardingCreatePlan: document.querySelector("#onboarding-create-plan"),
  onboardingSkipPlan: document.querySelector("#onboarding-skip-plan"),
  onboardingTaskForm: document.querySelector("#onboarding-task-form"),
  onboardingActivePlan: document.querySelector("#onboarding-active-plan"),
  onboardingTaskTitle: document.querySelector("#onboarding-task-title"),
  onboardingTaskError: document.querySelector("#onboarding-task-error"),
  onboardingStartNow: document.querySelector("#onboarding-start-now"),
  onboardingCreateTask: document.querySelector("#onboarding-create-task"),
  onboardingFinishWithPlan: document.querySelector("#onboarding-finish-with-plan"),
  onboardingStartFailedActions: document.querySelector("#onboarding-start-failed-actions"),
  onboardingRetryStart: document.querySelector("#onboarding-retry-start"),
  onboardingFinishSetup: document.querySelector("#onboarding-finish-setup"),
  onboardingStatus: document.querySelector("#onboarding-status"),
  onboardingError: document.querySelector("#onboarding-error"),
  board: document.querySelector("#board"),
  appVersion: document.querySelector("#app-version"),
  settingsOpen: document.querySelector("#settings-open"),
  settingsModal: document.querySelector("#settings-modal"),
  settingsClose: document.querySelector("#settings-close"),
  settingsBody: document.querySelector("#settings-body"),
  settingsSectionList: document.querySelector("#settings-section-list"),
  settingsStorageNotice: document.querySelector("#settings-storage-notice"),
  settingsSaveStatus: document.querySelector("#settings-save-status"),
  settingsReset: document.querySelector("#settings-reset"),
  settingsStartupRestore: document.querySelector("#settings-startup-restore"),
  settingsResetWindowLayout: document.querySelector("#settings-reset-window-layout"),
  settingsResetApplicationState: document.querySelector(
    "#settings-reset-application-state",
  ),
  settingsTheme: document.querySelector("#settings-theme"),
  settingsDensity: document.querySelector("#settings-density"),
  settingsReducedMotion: document.querySelector("#settings-reduced-motion"),
  settingsTerminalProfile: document.querySelector("#settings-terminal-profile"),
  settingsTerminalFontFamily: document.querySelector("#settings-terminal-font-family"),
  settingsTerminalFontSize: document.querySelector("#settings-terminal-font-size"),
  settingsTerminalUnicode: document.querySelector("#settings-terminal-unicode"),
  settingsTerminalScrollback: document.querySelector("#settings-terminal-scrollback"),
  settingsTerminalRenderer: document.querySelector("#settings-terminal-renderer"),
  settingsUpdatesAutomatic: document.querySelector("#settings-updates-automatic"),
  settingsOpenUpdates: document.querySelector("#settings-open-updates"),
  settingsDiagnostics: document.querySelector("#settings-diagnostics"),
  aboutVersion: document.querySelector("#about-version"),
  aboutBuild: document.querySelector("#about-build"),
  aboutProject: document.querySelector("#about-project"),
  aboutLicenseLink: document.querySelector("#about-license-link"),
  aboutHelp: document.querySelector("#about-help"),
  aboutReport: document.querySelector("#about-report"),
  updatesModal: document.querySelector("#updates-modal"),
  updatesClose: document.querySelector("#updates-close"),
  updatesCurrentVersion: document.querySelector("#updates-current-version"),
  updatesAutomatic: document.querySelector("#updates-automatic"),
  updatesStatus: document.querySelector("#updates-status"),
  updatesStatusTitle: document.querySelector("#updates-status-title"),
  updatesStatusDetail: document.querySelector("#updates-status-detail"),
  updatesProgressWrap: document.querySelector("#updates-progress-wrap"),
  updatesProgress: document.querySelector("#updates-progress"),
  updatesProgressLabel: document.querySelector("#updates-progress-label"),
  updatesRelease: document.querySelector("#updates-release"),
  updatesReleaseVersion: document.querySelector("#updates-release-version"),
  updatesReleaseMeta: document.querySelector("#updates-release-meta"),
  updatesReleaseNotes: document.querySelector("#updates-release-notes"),
  updatesReleasePage: document.querySelector("#updates-release-page"),
  updatesVerified: document.querySelector("#updates-verified"),
  updatesCancel: document.querySelector("#updates-cancel"),
  updatesPrimary: document.querySelector("#updates-primary"),
  projectName: document.querySelector("#project-name"),
  planTitle: document.querySelector("#plan-title"),
  planTotal: document.querySelector("#plan-total"),
  planList: document.querySelector("#sidebar-plan-list"),
  planProgress: document.querySelector("#plan-progress"),
  planProgressLabel: document.querySelector("#plan-progress-label"),
  planLaunchAgent: document.querySelector("#plan-launch-agent"),
  planTitleMenu: document.querySelector("#plan-title-menu"),
  goal: document.querySelector("#goal"),
  summary: document.querySelector("#summary"),
  stats: document.querySelector("#project-stats"),
  snapshotBounds: document.querySelector("#snapshot-bounds"),
  issues: document.querySelector("#issue-list"),
  issueTotal: document.querySelector("#issue-total"),
  activity: document.querySelector("#activity-list"),
  activityMore: document.querySelector("#activity-more"),
  memoryModal: document.querySelector("#memory-modal"),
  memoryDialogList: document.querySelector("#memory-dialog-list"),
  memoryDialogClose: document.querySelector("#memory-dialog-close"),
  status: document.querySelector("#status"),
  themeToggle: document.querySelector("#theme-toggle"),
  openProject: document.querySelector("#open-project-button"),
  switchProject: document.querySelector("#switch-project-button"),
  closeProject: document.querySelector("#close-project-button"),
  addForm: document.querySelector("#add-form"),
  taskTitle: document.querySelector("#task-title"),
  modal: document.querySelector("#modal"),
  dialogForm: document.querySelector("#dialog-form"),
  dialogEyebrow: document.querySelector("#dialog-eyebrow"),
  dialogHeading: document.querySelector("#dialog-heading"),
  dialogLabel: document.querySelector("#dialog-label"),
  dialogInput: document.querySelector("#dialog-input"),
  dialogNote: document.querySelector("#dialog-note"),
  dialogHelp: document.querySelector("#dialog-help"),
  dialogSubmit: document.querySelector("#dialog-submit"),
  planDialog: document.querySelector("#plan-dialog"),
  planDialogForm: document.querySelector("#plan-dialog-form"),
  planDialogEyebrow: document.querySelector("#plan-dialog-eyebrow"),
  planDialogHeading: document.querySelector("#plan-dialog-heading"),
  planDialogBody: document.querySelector("#plan-dialog-body"),
  planDialogProjectLabel: document.querySelector("#plan-dialog-project-label"),
  planDialogProject: document.querySelector("#plan-dialog-project"),
  planDialogTitleLabel: document.querySelector("#plan-dialog-title-label"),
  planDialogTitle: document.querySelector("#plan-dialog-title"),
  planDialogError: document.querySelector("#plan-dialog-error"),
  planDialogCancel: document.querySelector("#plan-dialog-cancel"),
  planDialogSubmit: document.querySelector("#plan-dialog-submit"),
  confirmModal: document.querySelector("#workspace-confirm-modal"),
  confirmEyebrow: document.querySelector("#workspace-confirm-eyebrow"),
  confirmHeading: document.querySelector("#workspace-confirm-heading"),
  confirmDetail: document.querySelector("#workspace-confirm-detail"),
  confirmCancel: document.querySelector("#workspace-confirm-cancel"),
  confirmSubmit: document.querySelector("#workspace-confirm-submit"),
  projectRoot: document.querySelector("#project-root"),
  storageStatus: document.querySelector("#storage-status"),
  gitState: document.querySelector("#git-state"),
  gitSummary: document.querySelector("#git-summary"),
  gitRemotes: document.querySelector("#git-remotes"),
  gitBranches: document.querySelector("#git-branches"),
  gitCommits: document.querySelector("#git-commits"),
  agentActivityTotal: document.querySelector("#agent-activity-total"),
  agentActivitySummary: document.querySelector("#agent-activity-summary"),
  agentActivity: document.querySelector("#agent-activity"),
  agentActivityLive: document.querySelector("#agent-activity-live"),
  agentHandoffForm: document.querySelector("#agent-handoff-form"),
  agentHandoffSource: document.querySelector("#agent-handoff-source"),
  agentHandoffTarget: document.querySelector("#agent-handoff-target"),
  agentHandoffSend: document.querySelector("#agent-handoff-send"),
  agentHandoffInbox: document.querySelector("#agent-handoff-inbox"),
	agentWorkflowForm: document.querySelector("#agent-workflow-form"),
	agentWorkflowRun: document.querySelector("#agent-workflow-run"),
	agentWorkflowKind: document.querySelector("#agent-workflow-kind"),
	agentWorkflowTarget: document.querySelector("#agent-workflow-target"),
	agentWorkflowPrepare: document.querySelector("#agent-workflow-prepare"),
	agentWorkflowInbox: document.querySelector("#agent-workflow-inbox"),
  agentDrift: document.querySelector("#agent-drift"),
  blockers: document.querySelector("#overview-blockers"),
  notes: document.querySelector("#overview-notes"),
  drawer: document.querySelector("#task-drawer"),
  drawerEyebrow: document.querySelector("#drawer-eyebrow"),
  drawerTitle: document.querySelector("#drawer-title"),
  drawerStatus: document.querySelector("#drawer-status"),
  drawerUpdated: document.querySelector("#drawer-updated"),
  drawerClose: document.querySelector("#drawer-close"),
  drawerStatusSelect: document.querySelector("#drawer-status-select"),
  drawerRename: document.querySelector("#drawer-rename"),
  drawerMemory: document.querySelector("#drawer-memory"),
  drawerLaunchAgent: document.querySelector("#drawer-launch-agent"),
  drawerRuntime: document.querySelector("#drawer-runtime"),
  drawerRuntimeCount: document.querySelector("#drawer-runtime-count"),
  drawerNotes: document.querySelector("#drawer-notes"),
  drawerNotesCount: document.querySelector("#drawer-notes-count"),
  drawerCommits: document.querySelector("#drawer-commits"),
  drawerCommitsCount: document.querySelector("#drawer-commits-count"),
  drawerIssues: document.querySelector("#drawer-issues"),
  drawerIssuesCount: document.querySelector("#drawer-issues-count"),
  agentLaunchModal: document.querySelector("#agent-launch-modal"),
  agentLaunchForm: document.querySelector("#agent-launch-form"),
  agentLaunchHeading: document.querySelector("#agent-launch-heading"),
  agentLaunchDetail: document.querySelector("#agent-launch-detail"),
  agentLaunchSelect: document.querySelector("#agent-launch-profile"),
  agentLaunchMessage: document.querySelector("#agent-launch-message"),
  agentLaunchCancel: document.querySelector("#agent-launch-cancel"),
  agentLaunchSubmit: document.querySelector("#agent-launch-submit"),
  terminalHelp: document.querySelector("#terminal-help"),
  terminalLinkContext: document.querySelector("#terminal-link-context"),
  terminalWriteback: document.querySelector("#terminal-writeback"),
  terminalAssociationModal: document.querySelector("#terminal-association-modal"),
  terminalAssociationForm: document.querySelector("#terminal-association-form"),
  terminalAssociationHeading: document.querySelector("#terminal-association-heading"),
  terminalAssociationDetail: document.querySelector("#terminal-association-detail"),
  terminalAssociationTarget: document.querySelector("#terminal-association-target"),
  terminalAssociationMessage: document.querySelector("#terminal-association-message"),
  terminalAssociationCancel: document.querySelector("#terminal-association-cancel"),
  terminalAssociationDetach: document.querySelector("#terminal-association-detach"),
  terminalAssociationSubmit: document.querySelector("#terminal-association-submit"),
  terminalWritebackModal: document.querySelector("#terminal-writeback-modal"),
  terminalWritebackForm: document.querySelector("#terminal-writeback-form"),
  terminalWritebackTarget: document.querySelector("#terminal-writeback-target"),
  terminalWritebackKind: document.querySelector("#terminal-writeback-kind"),
  terminalWritebackContent: document.querySelector("#terminal-writeback-content"),
  terminalWritebackMessage: document.querySelector("#terminal-writeback-message"),
  terminalWritebackPreview: document.querySelector("#terminal-writeback-preview"),
  terminalWritebackPreviewTarget: document.querySelector("#terminal-writeback-preview-target"),
  terminalWritebackPreviewContent: document.querySelector("#terminal-writeback-preview-content"),
  terminalWritebackSummaryWarning: document.querySelector("#terminal-writeback-summary-warning"),
  terminalWritebackSummaryConfirm: document.querySelector("#terminal-writeback-summary-confirm"),
  terminalWritebackCancel: document.querySelector("#terminal-writeback-cancel"),
  terminalWritebackPreviewButton: document.querySelector("#terminal-writeback-preview-button"),
  terminalWritebackSave: document.querySelector("#terminal-writeback-save"),
  taskTransitionModal: document.querySelector("#task-transition-modal"),
  taskTransitionForm: document.querySelector("#task-transition-form"),
  taskTransitionHeading: document.querySelector("#task-transition-heading"),
  taskTransitionDetail: document.querySelector("#task-transition-detail"),
  taskTransitionMessage: document.querySelector("#task-transition-message"),
  taskTransitionCancel: document.querySelector("#task-transition-cancel"),
  taskTransitionSubmit: document.querySelector("#task-transition-submit"),
  palette: document.querySelector("#palette"),
  paletteInput: document.querySelector("#palette-input"),
  paletteResults: document.querySelector("#palette-results"),
  planRing: document.querySelector("#plan-ring"),
  heatmap: document.querySelector("#activity-heatmap"),
  toast: document.querySelector("#toast"),
};

const workspaceController = new WorkspaceController();
const refreshGate = new RefreshGate();
const nativeEventDisposers = [];
const refreshLoop = new RefreshLoop(() => {
  void loadSnapshot(board?.planId || 0, true);
}, 15_000);
const runtimeRefreshes = new RuntimeRefreshCoalescer((generation) => {
  if (!runtimeEventIsCurrent(
    generation,
    workspaceController.state.generation,
    workspaceController.state.status === "open",
  )) return;
  void loadSnapshot(board?.planId || 0, true);
});

let workspaceState = { status: "welcome", generation: 0 };
let firstRunState = { ...initialFirstRunState };
let firstPlanState = { ...initialFirstPlanState };
let recentProjectsState = { ...initialRecentProjectsState };
let view = "board";
let snapshot = null;
let board = null;
let draggedTask = null;
let editingTask = null;
let dialogMode = "rename";
let dialogReturnFocus = null;
let planContextMenu = null;
let planContextMenuDispose = null;
let planContextMenuReturnFocus = null;
let planRenameActive = false;
let planDialogMode = null; // "delete" | "move" | "copy"
let planDialogPlan = null;
let planDialogTransferState = null;
let planDialogReturnFocus = null;
let toastTimer = null;
let memoryModalReturnFocus = null;
let settingsModalReturnFocus = null;
let settingsSection = settingsSections[0].id;
let settingsSaveSequence = 0;
let settingsDiagnosticsRequest = 0;
let settingsStatusTimer = 0;
// Long enough to read the longest status the dialog writes, short enough that
// it never becomes part of the furniture.
const settingsStatusClearDelay = 6000;
let preferences = defaultPreferences;
let updatesModalReturnFocus = null;
let updateState = { revision: 0, phase: "idle", currentVersion: "dev" };
let updateActionBusy = false;
let updateCancelRequested = false;
let confirmReturnFocus = null;
let confirmResolve = null;
let recentListRequest = 0;
let recentOperationSequence = 0;
let terminalHandle = null;
let terminalGeneration = 0;
let terminalProjectRoot = "";
let snapshotSequence = 0;
let activeSnapshotRequest = null;
let queuedSnapshotPlanId = 0;
let agentActivityAnnouncementKey = "";
let detailTask = null;
let detailRequest = 0;
let drawerReturnFocus = null;
let drawerOpenTimer = null;
let agentLaunchRequest = null;
let agentLaunchProfiles = [];
let agentLaunchReturnFocus = null;
let agentLaunchSequence = 0;
let agentLaunchBusy = false;
let terminalAssociationRequest = null;
let terminalAssociationReturnFocus = null;
let terminalAssociationSequence = 0;
let terminalAssociationBusy = false;
let terminalWritebackRequest = null;
let terminalWritebackReturnFocus = null;
let terminalWritebackSequence = 0;
let terminalWritebackBusy = false;
let taskTransitionRequest = null;
let taskTransitionSequence = 0;
let taskTransitionBusy = false;
let dragJustEndedAt = 0;
let sidebarWidth = defaultSidebarWidth;
let sidebarHidden = false;
let sidebarDragCleanup = null;
let layoutState = defaultLayoutState();
let panelLayoutRestored = false;
let paletteItems = [];
let paletteActive = -1;
let paletteTimer = null;
let paletteSequence = 0;
let paletteReturnFocus = null;
let pendingDetailTaskId = 0;
let heatmapRequested = false;
let repoStatsRequested = false;
let repoStats = null;
const expandedLanes = new Set();
const foldedLanes = new Set();

function readLayoutPreference(key) {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeLayoutPreference(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // The layout remains usable when WebView storage is unavailable.
  }
}

// The stored layout record is the authority. The localStorage keys stay
// mirrors of it, written so the sidebar does not reflow before GetLayoutState
// answers — the same reason the theme keeps its pre-paint key.
const layoutStateScheduler = new WorkspacePersistenceScheduler(
  {
    setTimeout: (callback, delay) => window.setTimeout(callback, delay),
    clearTimeout: (handle) => window.clearTimeout(handle),
  },
  () => writeLayoutState(),
);

function writeLayoutState() {
  try {
    void api()
      .SetLayoutState(layoutStatePatch(layoutState, workspaceState.project?.root || ""))
      .catch(() => {});
  } catch {
    // Layout persistence is optional; the live layout is unchanged.
  }
}

function recordSidebarLayout() {
  layoutState.sidebar = { width: sidebarWidth, hidden: sidebarHidden };
  layoutStateScheduler.markDirty();
}

// Only what the user did counts, and a click on a toggle is the only signal
// that means it: the dock also forces the panels open when the last session
// exits and when it disposes, and that teardown must not erase the record.
// The listener sits on the group so it runs after the dock's own handler and
// reads what the dock settled on.
function recordPanelLayout(event) {
  if (
    !panelLayoutRestored ||
    !event.target.closest("#board-panel-toggle, #terminal-panel-toggle")
  ) return;
  const next = {
    boardHidden: elements.boardPanelToggle.getAttribute("aria-pressed") === "true",
    terminalHidden: elements.terminalPanelToggle.getAttribute("aria-pressed") === "true",
  };
  if (
    next.boardHidden === layoutState.panels.boardHidden &&
    next.terminalHidden === layoutState.panels.terminalHidden
  ) return;
  layoutState.panels = next;
  layoutStateScheduler.markDirty();
}

// The dock owns the panel toggles, so the stored record is applied through
// them. The dock refuses a board change while no session is live, and a
// refusal leaves the record alone rather than overwriting the preference this
// restore exists to honor.
function restorePanelLayout() {
  panelLayoutRestored = false;
  for (const [toggle, hidden] of [
    [elements.boardPanelToggle, layoutState.panels.boardHidden],
    [elements.terminalPanelToggle, layoutState.panels.terminalHidden],
  ]) {
    if (toggle.disabled) continue;
    if ((toggle.getAttribute("aria-pressed") === "true") !== hidden) toggle.click();
  }
  // The clicks above dispatch synchronously, so the guard has done its whole
  // job by the time the loop ends: every click after this one is the user's,
  // and a dock that refused the restore must not silence them for the rest of
  // its life. The refusal itself is still not adopted — it happened under the
  // guard — so the stored record stands until a real gesture moves it.
  panelLayoutRestored = true;
}

function restoreProjectLayout(projectRoot) {
  const stored = layoutProjectState(layoutState, projectRoot);
  view = stored.view;
  expandedLanes.clear();
  foldedLanes.clear();
  for (const lane of stored.foldedLanes) foldedLanes.add(lane);
}

// Nothing is recorded before the board loads: the restored plan is still only
// a hint at that point, and writing a zero over it would lose it.
function recordProjectLayout() {
  const projectRoot = workspaceState.project?.root;
  if (!projectRoot || !board) return;
  const next = {
    view,
    planId: Number(board.planId || 0),
    foldedLanes: [...foldedLanes].sort(),
  };
  const current = layoutState.projects[projectRoot];
  if (current && JSON.stringify(current) === JSON.stringify(next)) return;
  layoutState.projects[projectRoot] = next;
  layoutStateScheduler.markDirty();
}

// The stored plan is a hint. The backend still resolves it, and a plan that no
// longer resolves silently falls back to the active plan.
function restoredPlanId(projectRoot) {
  return layoutProjectState(layoutState, projectRoot || "").planId;
}

function applyLayoutState(next) {
  layoutState = next;
  setSidebarWidth(layoutState.sidebar.width, false);
  setSidebarHidden(layoutState.sidebar.hidden, false);
  writeLayoutPreference(sidebarWidthStorageKey, String(sidebarWidth));
  writeLayoutPreference(sidebarHiddenStorageKey, String(sidebarHidden));
  restorePanelLayout();
}

async function loadLayoutState() {
  let stored;
  try {
    stored = normalizeLayoutState(await api().GetLayoutState());
  } catch {
    return;
  }
  if (stored.storage !== "ok") {
    // No readable record yet, so the mirror is what this window is already
    // using; adopting a default width here would move the sidebar for nothing.
    layoutState = { ...stored, sidebar: { width: sidebarWidth, hidden: sidebarHidden } };
    return;
  }
  applyLayoutState(stored);
}

function setSidebarWidth(width, persist = true) {
  sidebarWidth = clampSidebarWidth(width, window.innerWidth);
  const maximum = sidebarMaximumWidth(window.innerWidth);
  elements.app.style.setProperty("--sidebar-width", `${sidebarWidth}px`);
  elements.sidebarResize.setAttribute("aria-valuemax", String(maximum));
  elements.sidebarResize.setAttribute("aria-valuenow", String(sidebarWidth));
  if (persist) {
    writeLayoutPreference(sidebarWidthStorageKey, String(sidebarWidth));
    recordSidebarLayout();
  }
}

function setSidebarHidden(hidden, persist = true) {
  sidebarHidden = Boolean(hidden);
  elements.sidebar.hidden = sidebarHidden;
  elements.sidebarResize.hidden = sidebarHidden;
  elements.app.dataset.sidebarHidden = String(sidebarHidden);
  elements.sidebarToggle.setAttribute("aria-expanded", String(!sidebarHidden));
  const label = sidebarHidden
    ? "Show project sidebar"
    : "Hide project sidebar";
  elements.sidebarToggle.setAttribute("aria-label", label);
  elements.sidebarToggle.title = label;
  if (persist) {
    writeLayoutPreference(sidebarHiddenStorageKey, String(sidebarHidden));
    recordSidebarLayout();
  }
}

function beginSidebarResize(event) {
  if (firstPlanState.phase !== "idle" || sidebarHidden || event.button !== 0) return;
  event.preventDefault();
  sidebarDragCleanup?.();
  const startX = event.clientX;
  const startWidth = sidebarWidth;
  const pointerID = event.pointerId;
  const move = (moveEvent) => {
    if (moveEvent.pointerId !== pointerID) return;
    setSidebarWidth(startWidth + moveEvent.clientX - startX, false);
  };
  const cleanup = () => {
    elements.sidebarResize.removeEventListener("pointermove", move);
    elements.sidebarResize.removeEventListener("pointerup", finish);
    elements.sidebarResize.removeEventListener("pointercancel", finish);
    elements.sidebarResize.removeEventListener("lostpointercapture", finish);
    if (elements.sidebarResize.hasPointerCapture(pointerID)) {
      elements.sidebarResize.releasePointerCapture(pointerID);
    }
    if (sidebarDragCleanup === cleanup) sidebarDragCleanup = null;
  };
  const finish = (finishEvent) => {
    if (
      finishEvent.type !== "lostpointercapture" &&
      finishEvent.pointerId !== pointerID
    ) return;
    cleanup();
    writeLayoutPreference(sidebarWidthStorageKey, String(sidebarWidth));
    recordSidebarLayout();
  };
  sidebarDragCleanup = cleanup;
  elements.sidebarResize.setPointerCapture(pointerID);
  elements.sidebarResize.addEventListener("pointermove", move);
  elements.sidebarResize.addEventListener("pointerup", finish);
  elements.sidebarResize.addEventListener("pointercancel", finish);
  elements.sidebarResize.addEventListener("lostpointercapture", finish);
}

function resizeSidebarFromKeyboard(event) {
  if (firstPlanState.phase !== "idle" || sidebarHidden) return;
  const nextWidth = sidebarWidthFromKey(sidebarWidth, event.key, window.innerWidth);
  if (nextWidth === null) return;
  event.preventDefault();
  setSidebarWidth(nextWidth);
}

function initializeSidebarLayout() {
  sidebarWidth = storedSidebarWidth(
    readLayoutPreference(sidebarWidthStorageKey),
    window.innerWidth,
  );
  sidebarHidden = readLayoutPreference(sidebarHiddenStorageKey) === "true";
  setSidebarWidth(sidebarWidth, false);
  setSidebarHidden(sidebarHidden, false);
  elements.panelControls.addEventListener("click", recordPanelLayout);
}

const statusTitles = {
  todo: "Todo",
  doing: "Doing",
  blocked: "Blocked",
  done: "Done",
};

function api() {
  const backend = window.go?.gui?.App;
  if (!backend) throw new Error("The Wails backend is not ready");
  return backend;
}

function openHelpDestination(destination) {
  void api().OpenHelpDestination(destination).catch(() => {
    showError(new Error("Could not open the Help Center."));
  });
}

function messageFrom(error) {
  if (typeof error === "string") return error;
  return error?.message || "Something went wrong";
}

function showError(error) {
  window.clearTimeout(toastTimer);
  elements.toast.textContent = messageFrom(error);
  elements.toast.hidden = false;
  toastTimer = window.setTimeout(() => {
    elements.toast.hidden = true;
  }, 5000);
}

function setStatus(message) {
  elements.status.textContent = message;
}

function relativeTime(value) {
  const date = new Date(value);
  const elapsed = Date.now() - date.getTime();
  if (!Number.isFinite(elapsed)) return "";
  const minutes = Math.max(0, Math.round(elapsed / 60000));
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

function compactBytes(value) {
  if (!Number.isFinite(value) || value < 1024) return `${value || 0} B`;
  if (value < 1024 * 1024) return `${Math.round(value / 1024)} KiB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
}

function statElement(value, label) {
  const stat = document.createElement("div");
  stat.className = "stat";
  const caption = document.createElement("span");
  caption.className = "stat-label";
  caption.textContent = label;
  const number = document.createElement("span");
  number.className = "stat-value";
  number.textContent = value;
  stat.append(caption, number);
  return stat;
}

function emptyMemory(message) {
  const empty = document.createElement("div");
  empty.className = "memory-empty";
  empty.textContent = message;
  return empty;
}

function intelligenceItem(titleText, detailText, state = "") {
  const item = document.createElement("article");
  item.className = "intelligence-item";
  if (state) item.dataset.state = state;
  const title = document.createElement("p");
  title.className = "intelligence-title";
  title.textContent = titleText;
  const detail = document.createElement("p");
  detail.className = "intelligence-detail";
  detail.textContent = detailText;
  item.append(title, detail);
  return item;
}

function pill(label, value, tone = "") {
  const item = document.createElement("span");
  item.className = "intelligence-pill";
  if (tone) item.dataset.tone = tone;
  item.textContent = `${label} ${value}`;
  return item;
}

function activityElement(activity, expanded = false) {
  const item = document.createElement("article");
  item.className = expanded ? "activity activity-expanded" : "activity";
  item.style.setProperty(
    "--activity-color",
    activity.kind === "commit" ? "var(--todo)" : "var(--accent)",
  );
  const title = document.createElement("p");
  title.className = "activity-title";
  title.textContent = activity.title;
  const detail = document.createElement("p");
  detail.className = "activity-detail";
  detail.textContent = activity.detail;
  const meta = document.createElement("span");
  meta.className = "activity-meta";
  meta.textContent = `${activity.kind} · ${activity.target} · ${relativeTime(activity.occurredAt)}`;
  item.append(title, detail, meta);
  return item;
}

function fitRecentMemory() {
  if (!board || elements.activity.children.length === 0) return;
  const items = Array.from(elements.activity.children);
  items.forEach((item) => {
    item.hidden = false;
  });
  elements.activityMore.hidden = true;
  if (elements.activity.scrollHeight <= elements.activity.clientHeight + 1) return;

  elements.activityMore.hidden = false;
  const available = elements.activity.clientHeight;
  let visible = 0;
  items.forEach((item, index) => {
    const fits = item.offsetTop + item.offsetHeight <= available;
    item.hidden = !fits && index > 0;
    if (!item.hidden) visible += 1;
  });
  const hidden = Math.max(0, items.length - visible);
  elements.activityMore.hidden = hidden === 0;
  elements.activityMore.setAttribute(
    "aria-label",
    hidden === 1 ? "Show 1 more memory item" : `Show ${hidden} more memory items`,
  );
}

function renderMemory() {
  elements.goal.textContent = board.goal || "No north star set for this project.";
  elements.summary.textContent =
    board.summary || "No rolling summary yet. Agents can update it with ptrack summary set.";
  // The Overview is project-wide: totals never change with the selected
  // plan (the per-plan numbers stay on the board header).
  const tiles = [
    statElement(`${board.stats.tasksDone}/${board.stats.tasks}`, "Tasks done"),
    statElement(`${board.stats.plansDone}/${board.stats.plans}`, "Plans done"),
    statElement(board.stats.tasksOpen, "Open tasks"),
    statElement(board.stats.tasksBlocked, "Blocked"),
    statElement(board.stats.notes, "Notes"),
    statElement(board.stats.commits, "Commits"),
    statElement(board.stats.openIssues, "Open issues"),
  ];
  if (board.stats.milestones) {
    tiles.push(
      statElement(`${board.stats.milestonesDone}/${board.stats.milestones}`, "Milestones"),
    );
  }
  if (repoStats?.available) {
    tiles.push(
      statElement(repoStats.files.toLocaleString(), "Tracked files"),
      statElement(repoStats.lines.toLocaleString(), "Lines of code"),
    );
  }
  elements.stats.replaceChildren(...tiles);
  renderPlanRing(board.stats.tasksDone, board.stats.tasks);

  elements.issueTotal.textContent = board.stats.openIssues;
  elements.issues.replaceChildren();
  if (board.openIssues.length === 0) {
    elements.issues.append(emptyMemory("No open issues. The path is clear."));
  } else {
    board.openIssues.forEach((issue) => {
      const item = document.createElement("article");
      item.className = "issue";
      item.style.setProperty("--issue-color", severityColors[issue.severity] || "var(--muted)");
      const marker = document.createElement("span");
      marker.className = "issue-marker";
      marker.setAttribute("aria-hidden", "true");
      const content = document.createElement("div");
      const title = document.createElement("p");
      title.className = "issue-title";
      title.textContent = issue.title;
      const meta = document.createElement("span");
      meta.className = "issue-meta";
      meta.textContent = `${issue.severity} · #${issue.id}${issue.taskId ? ` · task #${issue.taskId}` : ""}`;
      content.append(title, meta);
      item.append(marker, content);
      elements.issues.append(item);
    });
  }

  elements.activity.replaceChildren();
  elements.memoryDialogList.replaceChildren();
  if (board.activity.length === 0) {
    const message = "Decisions and linked commits will appear here as the project evolves.";
    elements.activity.append(emptyMemory(message));
    elements.memoryDialogList.append(emptyMemory(message));
    elements.activityMore.hidden = true;
  } else {
    board.activity.forEach((activity) => {
      elements.activity.append(activityElement(activity));
      elements.memoryDialogList.append(activityElement(activity, true));
    });
    requestAnimationFrame(fitRecentMemory);
  }
}

const SVG_NS = "http://www.w3.org/2000/svg";

function svgElement(name, attributes = {}) {
  const node = document.createElementNS(SVG_NS, name);
  for (const [key, value] of Object.entries(attributes)) {
    node.setAttribute(key, value);
  }
  return node;
}

function renderPlanRing(done, total) {
  elements.planRing.replaceChildren();
  if (!total) {
    elements.planRing.hidden = true;
    return;
  }
  elements.planRing.hidden = false;
  const radius = 34;
  const circumference = 2 * Math.PI * radius;
  const fraction = Math.min(1, done / total);
  const svg = svgElement("svg", {
    viewBox: "0 0 84 84",
    class: "plan-ring-svg",
    "aria-hidden": "true",
  });
  svg.append(
    svgElement("circle", { class: "plan-ring-track", cx: 42, cy: 42, r: radius }),
    svgElement("circle", {
      class: "plan-ring-value",
      cx: 42,
      cy: 42,
      r: radius,
      "stroke-dasharray": `${circumference}`,
      "stroke-dashoffset": `${circumference * (1 - fraction)}`,
      transform: "rotate(-90 42 42)",
    }),
  );
  const number = svgElement("text", {
    class: "plan-ring-number",
    x: 42,
    y: 40,
    "text-anchor": "middle",
  });
  number.textContent = `${done}/${total}`;
  const caption = svgElement("text", {
    class: "plan-ring-caption",
    x: 42,
    y: 54,
    "text-anchor": "middle",
  });
  caption.textContent = "done";
  svg.append(number, caption);
  elements.planRing.setAttribute(
    "aria-label",
    `Project progress: ${done} of ${total} tasks done`,
  );
  elements.planRing.append(svg);
}

function renderHeatmap(days) {
  elements.heatmap.replaceChildren();
  if (!days.length) {
    elements.heatmap.append(emptyMemory("No activity recorded yet."));
    return;
  }
  const columns = heatmapWeeks(days);
  const cell = 10;
  const pitch = cell + 2;
  const width = columns.length * pitch - 2;
  const height = 7 * pitch - 2;
  const svg = svgElement("svg", {
    viewBox: `0 0 ${width} ${height}`,
    width: width,
    height: height,
    class: "heatmap-svg",
    role: "img",
    "aria-label": "Daily note and commit activity for the last 16 weeks",
  });
  columns.forEach((column, x) => {
    column.forEach((day, y) => {
      if (!day.date) return;
      const rect = svgElement("rect", {
        class: `heatmap-cell heatmap-level-${day.level}`,
        x: x * pitch,
        y: y * pitch,
        width: cell,
        height: cell,
        rx: 2,
      });
      const tip = svgElement("title");
      tip.textContent = `${day.count} ${day.count === 1 ? "item" : "items"} · ${day.date}`;
      rect.append(tip);
      svg.append(rect);
    });
  });
  elements.heatmap.append(svg);
}

// Repository code statistics follow the heatmap pattern: fetched lazily
// once the Overview is shown, re-fetched after a snapshot reload.
async function loadRepoStats(force = false) {
  if (workspaceController.state.status !== "open") return;
  if (repoStatsRequested && !force) return;
  repoStatsRequested = true;
  try {
    repoStats = await api().GetRepoStatsV1();
    if (board) renderMemory();
  } catch {
    repoStatsRequested = false;
  }
}

// The heatmap is fetched lazily: only once the Overview is shown, and
// again (forced) after a snapshot reload while it is visible.
async function loadHeatmap(force = false) {
  if (workspaceController.state.status !== "open") return;
  if (heatmapRequested && !force) return;
  heatmapRequested = true;
  try {
    renderHeatmap(await api().GetActivityHeatmapV2(16));
  } catch (error) {
    heatmapRequested = false;
    if (workspaceController.state.status === "open") showError(error);
  }
}

function contextChip(count, singular, extraClass = "") {
  const chip = document.createElement("span");
  chip.className = `context-chip ${extraClass}`.trim();
  chip.textContent = `${count} ${count === 1 ? singular : `${singular}s`}`;
  return chip;
}

function actionButton(label, title, handler) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "card-action";
  button.textContent = label;
  button.title = title;
  button.setAttribute("aria-label", title);
  button.addEventListener("click", handler);
  return button;
}

function cardElement(task) {
  const card = document.createElement("article");
  card.className = "card";
  card.dataset.taskId = task.id;
  card.dataset.status = task.status;

  const dragZone = document.createElement("div");
  dragZone.className = "card-drag-zone";
  dragZone.draggable = true;
  dragZone.tabIndex = 0;
  dragZone.setAttribute(
    "aria-label",
    `Task #${task.id}: ${task.title}. Drag to change status, press Enter for details.`,
  );
  const meta = document.createElement("div");
  meta.className = "card-meta";
  const identity = document.createElement("span");
  identity.textContent = `#${task.id} · ${relativeTime(task.updatedAt)}`;
  const dragLabel = document.createElement("span");
  dragLabel.className = "drag-label";
  dragLabel.textContent = "Drag";
  meta.append(identity, dragLabel);
  const title = document.createElement("p");
  title.className = "card-title";
  title.textContent = task.title;
  dragZone.append(meta, title);

  const linkedRuntime = linkedTaskRuntimePresentation(task.linkedRuntime);
  if (linkedRuntime) {
    const linked = document.createElement("span");
    linked.className = "card-linked-runtime";
    linked.dataset.state = linkedRuntime.state;
    linked.textContent = linkedRuntime.compact;
    linked.title = linkedRuntime.detail;
    linked.setAttribute("aria-label", `Linked runtime: ${linkedRuntime.detail}`);
    dragZone.append(linked);
    dragZone.setAttribute(
      "aria-label",
      `Task #${task.id}: ${task.title}. ${linkedRuntime.detail}. Drag to change status, press Enter for details.`,
    );
  }

  // Hold is orthogonal to status: the card keeps its lane and gains a badge.
  // Drag and drop stay enabled — a held task can still change status.
  if (task.holdReason) {
    const hold = document.createElement("span");
    hold.className = "card-hold";
    hold.textContent = "⏸ On hold";
    hold.title = task.holdReason;
    dragZone.append(hold);
    dragZone.setAttribute(
      "aria-label",
      `${dragZone.getAttribute("aria-label")} On hold: ${task.holdReason}.`,
    );
  }

  // Open deps are orthogonal to status too: the card keeps its lane and gains
  // a badge; the blocking IDs are the badge's tooltip.
  if (task.depsOpen?.length) {
    const deps = document.createElement("span");
    deps.className = "card-deps";
    deps.textContent = "⛓ Waiting";
    deps.title = `Waiting on ${task.depsOpen.map((id) => `#${id}`).join(", ")}`;
    dragZone.append(deps);
    dragZone.setAttribute(
      "aria-label",
      `${dragZone.getAttribute("aria-label")} ${deps.title}.`,
    );
  }

  if (task.latestNote) {
    const note = document.createElement("p");
    note.className = "latest-note";
    note.textContent = task.latestNote;
    dragZone.append(note);
  }
  if (task.noteCount || task.commitCount || task.issueCount) {
    const context = document.createElement("div");
    context.className = "card-context";
    if (task.noteCount) context.append(contextChip(task.noteCount, "note"));
    if (task.commitCount) context.append(contextChip(task.commitCount, "commit"));
    if (task.issueCount) context.append(contextChip(task.issueCount, "issue", "issue-chip"));
    dragZone.append(context);
  }
  dragZone.addEventListener("click", () => {
    // A click that ends a drag, or the first click of a double-click rename,
    // must not open the drawer.
    if (Date.now() - dragJustEndedAt < 300) return;
    window.clearTimeout(drawerOpenTimer);
    drawerOpenTimer = window.setTimeout(() => {
      drawerOpenTimer = null;
      openTaskDetail(task);
    }, 240);
  });
  dragZone.addEventListener("keydown", (event) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    window.clearTimeout(drawerOpenTimer);
    drawerOpenTimer = null;
    openTaskDetail(task);
  });
  dragZone.addEventListener("dblclick", () => {
    window.clearTimeout(drawerOpenTimer);
    drawerOpenTimer = null;
    openRename(task);
  });
  dragZone.addEventListener("dragstart", (event) => {
    draggedTask = task;
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", String(task.id));
    requestAnimationFrame(() => card.classList.add("dragging"));
  });
  dragZone.addEventListener("dragend", () => {
    draggedTask = null;
    dragJustEndedAt = Date.now();
    card.classList.remove("dragging");
    document.querySelectorAll(".drag-over").forEach((node) => node.classList.remove("drag-over"));
  });

  const actions = document.createElement("div");
  actions.className = "card-actions";
  const statusSelect = document.createElement("select");
  statusSelect.setAttribute("aria-label", `Move task #${task.id}`);
  board.columns.forEach((column) => {
    const option = document.createElement("option");
    option.value = column.status;
    option.textContent = column.title;
    option.selected = column.status === task.status;
    statusSelect.append(option);
  });
  statusSelect.addEventListener("change", (event) =>
    void moveTask(task.id, statusSelect.value, event.currentTarget)
  );
  actions.append(
    statusSelect,
    actionButton(
      "Agent",
      `Launch an installed agent for task #${task.id}`,
      (event) => void openAgentLaunchPicker(
        { planId: Number(board.planId), task },
        event.currentTarget,
      ),
    ),
    actionButton("Edit", "Rename task", () => openRename(task)),
    actionButton("Memory", "Record a memory note", () => openMemory(task)),
  );
  card.append(dragZone, actions);
  return card;
}

function columnElement(column, collapsed = false) {
  const lane = document.createElement("section");
  lane.className = collapsed ? "column column-collapsed" : "column";
  lane.dataset.status = column.status;
  lane.style.setProperty("--lane-color", laneColors[column.status]);
  if (collapsed) {
    // Slim rail for an empty lane: rotated title + count, click to expand.
    lane.setAttribute("role", "button");
    lane.tabIndex = 0;
    lane.setAttribute("aria-expanded", "false");
    lane.setAttribute("aria-label", `${column.title} lane is collapsed. Activate to expand.`);
    lane.title = `${column.title} · ${column.tasks.length} — click to expand`;
    const rail = document.createElement("div");
    rail.className = "column-rail";
    const heading = document.createElement("h3");
    heading.className = "column-title";
    const dot = document.createElement("span");
    dot.className = "column-dot";
    dot.setAttribute("aria-hidden", "true");
    heading.append(dot, document.createTextNode(column.title));
    const count = document.createElement("span");
    count.className = "column-count";
    count.textContent = column.tasks.length;
    rail.append(heading, count);
    lane.append(rail);
    const expand = () => {
      foldedLanes.delete(column.status);
      expandedLanes.add(column.status);
      renderBoard();
      recordProjectLayout();
    };
    lane.addEventListener("click", (event) => {
      if (Date.now() - dragJustEndedAt < 300) return;
      event.preventDefault();
      expand();
    });
    lane.addEventListener("keydown", (event) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      expand();
    });
  } else {
    const header = document.createElement("header");
    header.className = "column-header";
    const heading = document.createElement("h3");
    heading.className = "column-title";
    const dot = document.createElement("span");
    dot.className = "column-dot";
    dot.setAttribute("aria-hidden", "true");
    heading.append(dot, document.createTextNode(column.title));
    const count = document.createElement("span");
    count.className = "column-count";
    count.textContent = column.tasks.length;
    const fold = document.createElement("button");
    fold.type = "button";
    fold.className = "column-fold";
    fold.textContent = "⌄";
    fold.title = `Collapse ${column.title} lane`;
    fold.setAttribute("aria-label", `Collapse ${column.title} lane`);
    fold.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      expandedLanes.delete(column.status);
      foldedLanes.add(column.status);
      renderBoard();
      recordProjectLayout();
    });
    header.append(heading, count, fold);
    const cards = document.createElement("div");
    cards.className = "cards";
    if (column.tasks.length === 0) {
      const empty = document.createElement("div");
      empty.className = "empty";
      empty.textContent = board.planId ? "Drop a task here" : "No active plan";
      cards.append(empty);
    } else {
      column.tasks.forEach((task) => cards.append(cardElement(task)));
    }
    lane.append(header, cards);
  }
  lane.addEventListener("dragover", (event) => {
    if (!draggedTask || draggedTask.status === column.status) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    lane.classList.add("drag-over");
  });
  lane.addEventListener("dragleave", (event) => {
    if (!lane.contains(event.relatedTarget)) lane.classList.remove("drag-over");
  });
  lane.addEventListener("drop", (event) => {
    event.preventDefault();
    lane.classList.remove("drag-over");
    if (draggedTask && draggedTask.status !== column.status) {
      const taskId = draggedTask.id;
      const invoker = document.querySelector(
        `.card[data-task-id="${taskId}"] .card-drag-zone`,
      );
      void moveTask(taskId, column.status, invoker);
    }
  });
  return lane;
}

function selectPlan(planId) {
  if (firstPlanState.phase !== "idle") return;
  // Same selection path the topbar picker used: a snapshot for that plan.
  void loadSnapshot(planId);
}

function renderPlanList() {
  elements.planList.replaceChildren();
  elements.planTotal.textContent = board.plans.length;
  board.plans.forEach((plan) => {
    // Not a native <button>: it hosts the nested "⋯" plan-actions button
    // below, and interactive content can't nest inside a real button.
    const item = document.createElement("div");
    item.setAttribute("role", "button");
    item.tabIndex = 0;
    item.className = "sidebar-plan";
    if (String(plan.id) === String(board.planId)) {
      item.classList.add("active");
      item.setAttribute("aria-current", "true");
    }
    item.title = plan.isActive ? `${plan.title} · active plan` : plan.title;
    const title = document.createElement("span");
    title.className = "sidebar-plan-title";
    title.textContent = `#${plan.id} ${plan.title}`;
    item.append(title);
    if (plan.isActive) {
      const dot = document.createElement("span");
      dot.className = "sidebar-plan-dot";
      dot.setAttribute("aria-hidden", "true");
      item.append(dot);
    }
    if (plan.holdReason) {
      const hold = document.createElement("span");
      hold.className = "sidebar-plan-hold";
      hold.textContent = "⏸";
      hold.setAttribute("aria-hidden", "true");
      item.append(hold);
      item.title = `${item.title} · on hold: ${plan.holdReason}`;
    }
    if (plan.claimedBy) {
      const claim = document.createElement("span");
      claim.className = "sidebar-plan-claim";
      claim.textContent = "🔒";
      claim.setAttribute("aria-hidden", "true");
      item.append(claim);
      item.title = `${item.title} · claimed by ${plan.claimedBy}`;
    }
    if (plan.depsOpen?.length) {
      const deps = document.createElement("span");
      deps.className = "sidebar-plan-deps";
      deps.textContent = "⛓";
      deps.setAttribute("aria-hidden", "true");
      item.append(deps);
      item.title = `${item.title} · waiting on ${plan.depsOpen.map((id) => `#${id}`).join(", ")}`;
    }
    item.addEventListener("click", () => selectPlan(plan.id));
    item.addEventListener("keydown", (event) => {
      // Ignore keys bubbling up from the nested gear button (Enter/Space
      // there should trigger it, not also select the row).
      if (event.target !== item) return;
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      selectPlan(plan.id);
    });
    item.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      openPlanContextMenu(plan, title, item, { x: event.clientX, y: event.clientY });
    });
    if (plan.tasksTotal > 0) {
      // 2px session progress track; absolutely positioned so the 30px row
      // height never changes.
      const track = document.createElement("span");
      track.className = "sidebar-plan-track";
      track.setAttribute("aria-hidden", "true");
      const fill = document.createElement("span");
      fill.className = "sidebar-plan-fill";
      fill.style.width = `${Math.round((plan.tasksDone / plan.tasksTotal) * 100)}%`;
      track.append(fill);
      item.append(track);
      item.title = `${item.title} · ${plan.tasksDone}/${plan.tasksTotal} done`;
    }
    const menuButton = document.createElement("button");
    menuButton.type = "button";
    menuButton.className = "sidebar-plan-menu";
    // Same gear as the board header's plan-actions trigger, cloned so the
    // icon path lives in index.html exactly once.
    menuButton.append(elements.planTitleMenu.querySelector("svg").cloneNode(true));
    menuButton.setAttribute("aria-label", `Plan #${plan.id} actions`);
    menuButton.setAttribute("aria-haspopup", "menu");
    menuButton.addEventListener("click", (event) => {
      event.stopPropagation();
      const rect = menuButton.getBoundingClientRect();
      openPlanContextMenu(plan, title, menuButton, { x: rect.left, y: rect.bottom + 4 });
    });
    item.append(menuButton);
    elements.planList.append(item);
  });
}

function renderBoard() {
  elements.projectName.textContent = board.projectName;
  elements.planTitle.textContent = board.planTitle || "No active plan";
  renderPlanList();
  const total = board.stats.planTasks;
  const done = board.stats.planTasksDone;
  const percentage = total ? Math.round((done / total) * 100) : 0;
  elements.planProgress.style.width = `${percentage}%`;
  elements.planProgressLabel.textContent = `${done}/${total} done`;
  elements.taskTitle.disabled = board.planId === 0;
  elements.addForm.querySelector("button").disabled = board.planId === 0;
  elements.planLaunchAgent.disabled = board.planId === 0;
  const collapsed = new Set(
    collapsedLaneStatuses(
      board.columns.map((column) => ({
        status: column.status,
        taskCount: column.tasks.length,
      })),
      expandedLanes,
      foldedLanes,
    ),
  );
  elements.board.style.gridTemplateColumns = board.columns
    .map((column) => (collapsed.has(column.status) ? "48px" : "minmax(214px, 1fr)"))
    .join(" ");
  elements.board.replaceChildren();
  board.columns.forEach((column) =>
    elements.board.append(columnElement(column, collapsed.has(column.status))),
  );
  renderMemory();
}

function renderIntelligence() {
  const project = snapshot.project;
  const tracking = snapshot.tracking;
  elements.projectRoot.textContent = project.root;
  const storage = project.storage;
  elements.storageStatus.textContent = storage.exists
    ? `p-track format v${storage.formatVersion} · ${compactBytes(storage.sizeBytes)} · writer ${storage.lastWriteVersion || "unknown"}`
    : storage.error || "p-track storage unavailable";
  elements.snapshotBounds.replaceChildren();
  for (const [label, bound] of Object.entries(tracking.bounds || {})) {
    elements.snapshotBounds.append(
      pill(label, bound.more ? `${bound.shown}/${bound.total}` : bound.total),
    );
  }

  elements.blockers.replaceChildren();
  if (tracking.blockers.length === 0) {
    elements.blockers.append(emptyMemory("No blocked tasks."));
  } else {
    tracking.blockers.slice(0, 10).forEach((task) => {
      elements.blockers.append(intelligenceItem(`Blocked · #${task.id}`, task.title, "error"));
    });
  }
  elements.notes.replaceChildren();
  tracking.notes.slice(0, 10).forEach((note) => {
    elements.notes.append(
      intelligenceItem(
        `${note.kind || "Note"} · ${note.target}${note.targetId ? ` #${note.targetId}` : ""}`,
        `${relativeTime(note.occurredAt)} · ${note.body}`,
      ),
    );
  });

  renderGitIntelligence(snapshot.git);
  renderAgentActivity(snapshot.agentActivity);
  renderDrift(snapshot.drift);
}

function renderDrift(section) {
  elements.agentDrift.replaceChildren();
  const drift = driftPresentation(section);
  if (drift.incomplete) {
    elements.agentDrift.append(
      intelligenceItem(
        "Work comparison incomplete",
        "Bounded Git or agent evidence was omitted. No missing warning should be treated as proof of alignment.",
        "stale",
      ),
    );
  }
  const copy = {
    checkoutChangedPath: ["Shared checkout change", "Project-level and unattributed"],
    untrackedFile: ["Untracked file", "Project-level and unattributed"],
    unlinkedCommit: ["Unlinked commit", "Exact SHA has no p-track commit link"],
    crossTaskPathOverlap: ["Possible cross-task path overlap", "Explicit owners on different tasks reported the same current path"],
    taskDriftSignal: ["Possible task drift", "Provider-neutral structured evidence indicates a current scope mismatch"],
  };
  drift.findings.forEach((finding) => {
    const [title, meaning] = copy[finding.kind];
    const evidence = finding.path || finding.sha ||
      finding.runIds.map((runId) => runId.slice(0, 8)).join(", ") || "structured evidence";
    elements.agentDrift.append(
      intelligenceItem(
        title,
        `${meaning} · ${evidence} · ${finding.evidenceCount} evidence signal${finding.evidenceCount === 1 ? "" : "s"}. This is advisory, not proof of drift.`,
        finding.severity === "warning" ? "waiting" : "",
      ),
    );
  });
}

function renderGitIntelligence(section) {
  elements.gitSummary.replaceChildren();
  elements.gitRemotes.replaceChildren();
  elements.gitBranches.replaceChildren();
  elements.gitCommits.replaceChildren();
  elements.gitState.textContent = section.state;
  if (section.state !== "ready" && section.state !== "stale") {
    elements.gitState.textContent = "Error";
    elements.gitSummary.append(pill("Git", section.error || "unavailable", "error"));
    return;
  }
  if (section.state === "stale") {
    elements.gitSummary.append(
      pill("Git", `stale · ${section.error || "refresh unavailable"}`, "error"),
    );
  }
  const git = section.snapshot;
  if (git.state === "notRepository") {
    elements.gitState.textContent = "No repository";
    elements.gitSummary.append(pill("Git", "not found"));
    return;
  }
  const status = git.status;
  elements.gitState.textContent = status.detached
    ? "Detached"
    : git.linkedWorktree
      ? "Worktree"
      : "Ready";
  elements.gitSummary.append(
    pill("branch", status.detached ? status.oid?.slice(0, 8) || "detached" : status.branch || "initial"),
    pill("staged", status.staged),
    pill("unstaged", status.unstaged),
    pill("untracked", status.untracked),
    pill("conflicts", status.conflicted, status.conflicted ? "error" : ""),
    pill("ignored", status.ignored),
  );
  if (status.upstream) {
    elements.gitSummary.append(
      pill("upstream", status.upstream),
      pill("ahead", git.divergence?.ahead ?? status.ahead, status.ahead ? "warning" : ""),
      pill("behind", git.divergence?.behind ?? status.behind, status.behind ? "warning" : ""),
      pill("unpushed", git.unpushedCommits?.length || 0, git.unpushedCommits?.length ? "warning" : ""),
    );
  } else {
    elements.gitSummary.append(pill("upstream", "none", "warning"));
  }

  if (!git.remotes?.length) {
    elements.gitRemotes.append(emptyMemory("No remotes configured."));
  } else {
    git.remotes.forEach((remote) => {
      const fetch = remote.fetchUrls?.join(", ") || "none";
      const push = remote.pushUrls?.join(", ") || fetch;
      elements.gitRemotes.append(intelligenceItem(`Remote · ${remote.name}`, `fetch ${fetch} · push ${push}`));
    });
  }
  const branches = [...(git.localBranches || []), ...(git.remoteBranches || [])];
  branches.slice(0, 24).forEach((branch) => {
    const flags = [
      branch.current ? "current" : "",
      branch.remote ? "remote" : "local",
      branch.stale ? "stale signal" : "",
      branch.worktreePath ? `worktree ${branch.worktreePath}` : "",
    ].filter(Boolean);
    elements.gitBranches.append(
      intelligenceItem(branch.name, `${flags.join(" · ")} · ${relativeTime(branch.lastCommitAt)}`, branch.stale ? "stale" : ""),
    );
  });
  if (branches.length === 0) {
    elements.gitBranches.append(emptyMemory("No branch refs found."));
  }
  (git.recentCommits || []).slice(0, 12).forEach((commit) => {
    const areas = commit.changedAreas?.map((area) => `${area.name} ${area.files}`).join(", ");
    const refs = commit.refs?.length ? ` · ${commit.refs.join(", ")}` : "";
    elements.gitCommits.append(
      intelligenceItem(
        `${commit.sha.slice(0, 8)} · ${commit.subject}`,
        `${commit.authorName} · ${relativeTime(commit.date)} · ${commit.filesChanged} files${areas ? ` · ${areas}` : ""}${refs}`,
      ),
    );
  });
}

function renderAgentActivity(section) {
  const focusKey = document.activeElement?.dataset?.mutationFocusKey || "";
  const focusedWorktreeSelection = captureFocusedWorktreeSelection();
  elements.agentActivity.replaceChildren();
  elements.agentActivitySummary.replaceChildren();
  const activity = agentActivityPresentation(section);
  const announcement = agentActivityAnnouncement(activity, agentActivityAnnouncementKey);
  if (announcement) {
    agentActivityAnnouncementKey = announcement.key;
    elements.agentActivityLive.textContent = announcement.text;
  }
  renderAgentHandoffs(activity.items, activity.handoffs);
	renderAgentWorkflows(
		activity.items,
		activity.workflows,
		activity.workflowTargets,
		activity.workflowTargetsIncomplete,
	);
  elements.agentActivityTotal.textContent = activity.compact;
  elements.agentActivityTotal.title = activity.detail;
  activity.counts.forEach(({ state, count }) => {
    const tone = ["failed", "blocked"].includes(state)
      ? "error"
      : state === "waiting"
        ? "warning"
        : "";
    elements.agentActivitySummary.append(pill(state, count, tone));
  });
  if (activity.analysisIncomplete) {
    elements.agentActivity.append(
      intelligenceItem(
        "Overlap analysis incomplete",
        "The bounded runtime snapshot omitted agents or conflict groups. Absence of another warning does not prove exclusive task ownership.",
        "stale",
      ),
    );
  }
  if (activity.worktreesIncomplete) {
    elements.agentActivity.append(
      intelligenceItem(
        "Worktree discovery incomplete",
        "Only the bounded set of existing host-observed worktrees shown here may be selected.",
        "stale",
      ),
    );
  }
  activity.conflicts.forEach((conflict) => {
    const ownership = conflict.ownerCount
      ? ` · ${conflict.ownerCount} explicit owner${conflict.ownerCount === 1 ? "" : "s"}`
      : "";
    elements.agentActivity.append(
      intelligenceItem(
        `Overlap warning · plan #${conflict.planId} · task #${conflict.taskId}`,
        `${conflict.agentCount} active agents share this task${ownership}. Advisory only; no agent, association, or task was changed.`,
        "blocked",
      ),
    );
  });
  if (activity.notificationsIncomplete) {
    elements.agentActivity.append(
      intelligenceItem(
        "Notifications incomplete",
        "Older structured events or agent rows were omitted by the workspace bounds.",
        "stale",
      ),
    );
  }
  const notificationLabels = {
    approvalRequested: ["Approval requested", "Attention required; no permission has been granted."],
    question: ["Agent question", "The agent is waiting for user attention."],
    failure: ["Agent failure", "The provider reported an explicit failure."],
    completion: ["Agent completed", "The provider reported explicit lifecycle completion."],
  };
  activity.notifications.forEach((notification) => {
    const [title, meaning] = notificationLabels[notification.kind];
    elements.agentActivity.append(
      intelligenceItem(
        `${title} · agent ${notification.runId.slice(0, 8)}`,
        `${meaning} · ${runtimeAssociationLabel(notification.association)} · ${relativeTime(notification.observedAt)}`,
        ["approvalRequested", "question"].includes(notification.kind)
          ? "waiting"
          : notification.kind === "failure"
            ? "failed"
            : "completed",
      ),
    );
  });
  if (activity.items.length === 0) {
    elements.agentActivity.append(emptyMemory("No registered agent activity."));
  } else {
    activity.items.forEach((item) => {
      const origin = item.terminalBacked
        ? item.correspondingTerminal
          ? "terminal-backed"
          : item.terminalPresent
            ? "terminal-backed · association does not correspond"
            : "terminal-backed · terminal unavailable"
        : "external";
      const evidence = Number(item.evidenceCount || 0);
      const events = Number(item.eventCount || 0);
      const observed = item.lastEventAt ? ` · last event ${relativeTime(item.lastEventAt)}` : "";
      const row = intelligenceItem(
          `${item.state[0].toUpperCase()}${item.state.slice(1)} agent · ${item.runId.slice(0, 8)}`,
          `${origin} · ${runtimeAssociationLabel(item.association)} · ${evidence} evidence signal${evidence === 1 ? "" : "s"} · ${events} structured event${events === 1 ? "" : "s"}${observed}`,
          item.state,
        );
      if (item.ownership) {
        row.querySelector(".intelligence-detail")?.append(
          document.createTextNode(" · explicit task owner"),
        );
      }
      if (item.worktree?.verified) {
        row.querySelector(".intelligence-detail")?.append(
          document.createTextNode(
            ` · worktree verified · ${item.worktree.isolated ? "isolated checkout" : "project checkout"} · CWD ${item.worktree.cwdMatches ? "matches" : "does not match"}`,
          ),
        );
      }
      const taskId = Number(item.association?.taskId || 0);
      const revision = Number(item.association?.revision || 0);
      if (item.live && taskId > 0 && revision > 0) {
        const owned = Boolean(item.ownership);
        const button = document.createElement("button");
        button.type = "button";
        button.className = "button-secondary agent-ownership-action";
        button.dataset.mutationFocusKey = `ownership:${item.runId}`;
        button.textContent = owned ? "Release ownership" : "Claim task";
        button.setAttribute(
          "aria-label",
          `${owned ? "Release ownership of" : "Claim"} task #${taskId} for agent ${item.runId.slice(0, 8)}`,
        );
        button.addEventListener("click", () => {
          void runMutation(
            (generation) => api().SetAgentTaskOwnershipV2(
              generation,
              item.runId,
              revision,
              !owned,
            ),
            `${owned ? "Releasing" : "Claiming"} task #${taskId}…`,
            `Could not ${owned ? "release" : "claim"} task ownership`,
          );
        });
        row.append(button);
      }
      if (item.live && activity.worktrees.length > 0) {
        const controls = document.createElement("div");
        controls.className = "agent-worktree-controls";
        const select = document.createElement("select");
        select.dataset.mutationFocusKey = `worktree-select:${item.runId}`;
        select.dataset.worktreeRunId = item.runId;
        select.setAttribute(
          "aria-label",
          `Existing worktree for agent ${item.runId.slice(0, 8)}`,
        );
        activity.worktrees.forEach((worktree) => {
          const option = document.createElement("option");
          option.value = worktree.root;
          option.textContent = `${worktree.branch || "detached"} · ${worktree.head.slice(0, 8)} · ${worktree.root}`;
          select.append(option);
        });
        select.value = worktreeSelectionForRerender(
          activity.worktrees.map((entry) => entry.root),
          item.worktree?.identity?.root,
          focusedWorktreeSelection,
          item.runId,
        );
        const associate = document.createElement("button");
        associate.type = "button";
        associate.className = "button-secondary agent-ownership-action";
        associate.dataset.mutationFocusKey = `worktree-associate:${item.runId}`;
        associate.textContent = "Associate worktree";
        associate.addEventListener("click", () => {
          void runMutation(
            (generation) => api().SetAgentWorktreeV2(
              generation,
              item.runId,
              revision,
              select.value,
              true,
            ),
            "Verifying existing worktree…",
            "Could not associate worktree",
          );
        });
        controls.append(select, associate);
        if (item.worktree) {
          const detach = document.createElement("button");
          detach.type = "button";
          detach.className = "button-secondary agent-ownership-action";
          detach.dataset.mutationFocusKey = `worktree-detach:${item.runId}`;
          detach.textContent = "Detach worktree";
          detach.addEventListener("click", () => {
            void runMutation(
              (generation) => api().SetAgentWorktreeV2(
                generation,
                item.runId,
                revision,
                "",
                false,
              ),
              "Detaching worktree metadata…",
              "Could not detach worktree",
            );
          });
          controls.append(detach);
        }
        row.append(controls);
      }
      elements.agentActivity.append(row);
    });
  }
  restoreMutationFocus(focusKey);
}

function captureFocusedWorktreeSelection() {
  const active = document.activeElement;
  if (!(active instanceof HTMLElement)) return null;
  const controls = active.closest(".agent-worktree-controls");
  const select = controls?.querySelector("select[data-worktree-run-id]");
  if (!(select instanceof HTMLSelectElement)) return null;
  return { runId: select.dataset.worktreeRunId || "", value: select.value };
}

function renderAgentWorkflows(items, inbox, targets, targetsIncomplete) {
	const previousRun = elements.agentWorkflowRun.value;
	const previousTarget = elements.agentWorkflowTarget.value;
	const live = items.filter((item) => item.live && item.runId);
	elements.agentWorkflowRun.replaceChildren();
	live.forEach((item) => {
		const option = document.createElement("option");
		option.value = item.runId;
		option.textContent = `Agent ${item.runId.slice(0, 8)} · ${runtimeAssociationLabel(item.association)}`;
		elements.agentWorkflowRun.append(option);
	});
	if (live.some((item) => item.runId === previousRun)) elements.agentWorkflowRun.value = previousRun;
	elements.agentWorkflowTarget.replaceChildren();
	targets.forEach((branch) => {
		const option = document.createElement("option");
		option.value = branch;
		option.textContent = branch;
		elements.agentWorkflowTarget.append(option);
	});
	if (targets.includes(previousTarget)) elements.agentWorkflowTarget.value = previousTarget;
	const needsTarget = ["pullRequest", "merge"].includes(elements.agentWorkflowKind.value);
	elements.agentWorkflowTarget.disabled = !needsTarget;
	elements.agentWorkflowPrepare.disabled = live.length === 0 || (needsTarget && targets.length === 0);
	elements.agentWorkflowInbox.replaceChildren();
	if (targetsIncomplete) {
		elements.agentWorkflowInbox.append(intelligenceItem(
			"Target branches incomplete",
			"Only branches present in the bounded read-only Git snapshot can be selected.",
			"stale",
		));
	}
	if (inbox.incomplete) {
		elements.agentWorkflowInbox.append(intelligenceItem(
			"Workflow inbox incomplete",
			"Some bounded runtime rows were omitted; absence of a proposal is not conclusive.",
			"stale",
		));
	}
	if (inbox.items.length === 0) {
		elements.agentWorkflowInbox.append(emptyMemory("No workflow proposals. Nothing has been approved or executed."));
		return;
	}
	inbox.items.forEach((proposal) => {
		const target = proposal.targetBranch
			? ` → ${proposal.targetBranch} ${proposal.targetHead.slice(0, 8)}`
			: "";
		const status = proposal.status;
		const row = intelligenceItem(
			`${proposal.kind} · ${proposal.state} · agent ${proposal.runId.slice(0, 8)}`,
			`${proposal.branch}${target} · ${proposal.head.slice(0, 8)} · staged ${status.staged} · unstaged ${status.unstaged} · untracked ${status.untracked} · conflicts ${status.conflicted} · proposal only; no execution`,
			proposal.state === "approved" ? "completed" : "waiting",
		);
		if (proposal.state === "proposed") {
			const approve = document.createElement("button");
			approve.type = "button";
			approve.className = "button-secondary agent-ownership-action";
			approve.dataset.mutationFocusKey = workflowMutationFocusKey("approve", proposal.id);
			approve.textContent = "Approve proposal";
			approve.addEventListener("click", () => {
				void runMutation(
					(generation) => api().ApproveAgentWorkflowV2(generation, proposal.id),
					"Revalidating workflow proposal…",
					"Could not approve workflow proposal",
				);
			});
			row.append(approve);
		}
		const dismiss = document.createElement("button");
		dismiss.type = "button";
		dismiss.className = "button-secondary agent-ownership-action";
		dismiss.dataset.mutationFocusKey = workflowMutationFocusKey("dismiss", proposal.id);
		dismiss.textContent = "Dismiss";
		dismiss.addEventListener("click", () => {
			void runMutation(
				(generation) => api().DismissAgentWorkflowV2(generation, proposal.id),
				"Dismissing workflow proposal…",
				"Could not dismiss workflow proposal",
			);
		});
		row.append(dismiss);
		elements.agentWorkflowInbox.append(row);
	});
}

function renderAgentHandoffs(items, inbox) {
  const previousSource = elements.agentHandoffSource.value;
  const previousTarget = elements.agentHandoffTarget.value;
  elements.agentHandoffSource.replaceChildren();
  elements.agentHandoffTarget.replaceChildren();
  const live = items.filter((item) => item.live && item.runId);
  live.forEach((item) => {
    const label = `Agent ${item.runId.slice(0, 8)} · ${runtimeAssociationLabel(item.association)}`;
    for (const select of [elements.agentHandoffSource, elements.agentHandoffTarget]) {
      const option = document.createElement("option");
      option.value = item.runId;
      option.textContent = label;
      select.append(option);
    }
  });
  if (live.some((item) => item.runId === previousSource)) elements.agentHandoffSource.value = previousSource;
  if (live.some((item) => item.runId === previousTarget)) elements.agentHandoffTarget.value = previousTarget;
  if (elements.agentHandoffTarget.value === elements.agentHandoffSource.value && live.length > 1) {
    elements.agentHandoffTarget.value = live[1].runId;
  }
  elements.agentHandoffSend.disabled = live.length < 2;
  elements.agentHandoffInbox.replaceChildren();
  if (inbox.incomplete) {
    elements.agentHandoffInbox.append(
      intelligenceItem("Handoff inbox incomplete", "Some runtime rows were omitted; proposals may be unavailable.", "stale"),
    );
  }
  if (inbox.items.length === 0) {
    elements.agentHandoffInbox.append(emptyMemory("No pending handoff proposals."));
    return;
  }
  inbox.items.forEach((handoff) => {
    const row = intelligenceItem(
      `Handoff proposal · ${handoff.sourceRunId.slice(0, 8)} → ${handoff.targetRunId.slice(0, 8)}`,
      `Created ${relativeTime(handoff.createdAt)} · expires ${relativeTime(handoff.expiresAt)} · proposal only; no authority granted.`,
      "waiting",
    );
    const preview = document.createElement("pre");
    preview.className = "intelligence-detail agent-handoff-preview";
    preview.textContent = handoff.preview.text;
    const acknowledge = document.createElement("button");
    acknowledge.type = "button";
    acknowledge.className = "button-secondary agent-ownership-action";
    acknowledge.dataset.mutationFocusKey = `handoff:${handoff.id}`;
    acknowledge.textContent = "Acknowledge / dismiss";
    acknowledge.addEventListener("click", () => {
      void runMutation(
        (generation) => api().AcknowledgeAgentHandoffV2(
          generation,
          handoff.id,
          handoff.targetRunId,
        ),
        "Acknowledging handoff proposal…",
        "Could not acknowledge handoff proposal",
      );
    });
    row.append(preview, acknowledge);
    elements.agentHandoffInbox.append(row);
  });
}

function snapshotDialogIsOpen() {
  return applicationOverlayCoordinator.isOpen();
}

const applicationOverlaySelector =
  "body > .modal, body > [data-terminal-overlay]";

function applicationOverlayChanges(records = []) {
  return records.flatMap((record) => {
    const overlay = record.target;
    if (
      record.attributeName !== "hidden" ||
      !(overlay instanceof HTMLElement) ||
      !overlay.matches(applicationOverlaySelector)
    ) return [];
    return [{ overlay, open: record.oldValue !== null }];
  });
}

function syncApplicationOverlayState(records = []) {
  applicationOverlayCoordinator.reconcile(applicationOverlayChanges(records));
}

function hideApplicationOverlay(overlay) {
  overlay.hidden = true;
  applicationOverlayCoordinator.reconcile([{ overlay, open: false }]);
}

const applicationOverlayCoordinator = new ApplicationOverlayCoordinator(() =>
  document.querySelectorAll(applicationOverlaySelector),
  elements.app,
);
const applicationOverlayObserver = new MutationObserver(syncApplicationOverlayState);
applicationOverlayObserver.observe(document.body, {
  attributes: true,
  attributeFilter: ["hidden"],
  attributeOldValue: true,
  subtree: true,
});
nativeEventDisposers.push(() => applicationOverlayObserver.disconnect());
syncApplicationOverlayState();

async function loadSnapshot(
  planId = board?.planId || 0,
  quiet = false,
  queueIfBusy = true,
) {
  if (workspaceController.state.status !== "open") return false;
  if (!refreshGate.tryBegin(!quiet && queueIfBusy)) {
    if (!quiet && queueIfBusy) queuedSnapshotPlanId = Number(planId);
    return false;
  }
  if (
    quiet &&
    (snapshotDialogIsOpen() ||
      draggedTask ||
      elements.taskTitle.value.trim().length > 0 ||
      planRenameActive ||
      planContextMenu !== null)
  ) {
    refreshGate.finish();
    return false;
  }

  const ticket = workspaceController.capture();
  const request = ++snapshotSequence;
  activeSnapshotRequest = request;
  if (!quiet) setStatus("Refreshing project snapshot…");
  try {
    const response = await api().GetWorkspaceSnapshot(ticket.generation, Number(planId));
    if (request !== snapshotSequence || !workspaceController.accepts(ticket, response.generation)) {
      return true;
    }
    response.git = preserveSectionOnError(snapshot?.git, response.git);
    snapshot = response;
    board = response.tracking.board;
    elements.workspace.dataset.snapshotState = "ready";
    renderBoard();
    recordProjectLayout();
    renderIntelligence();
    openPendingTaskDetail();
    if (view === "overview" && heatmapRequested) void loadHeatmap(true);
    if (view === "overview" && repoStatsRequested) void loadRepoStats(true);
    const now = new Date(response.capturedAt).toLocaleTimeString([], {
      hour: "numeric",
      minute: "2-digit",
    });
    setStatus(`Snapshot synced ${now}`);
  } catch (error) {
    if (request !== snapshotSequence || ticket.epoch !== workspaceController.capture().epoch) return;
    if (snapshot) {
      elements.workspace.dataset.snapshotState = "stale";
      setStatus(`Snapshot stale · ${messageFrom(error)}`);
    } else {
      setStatus("Snapshot failed");
    }
    showError(error);
  } finally {
    if (activeSnapshotRequest === request) activeSnapshotRequest = null;
    const rerun = refreshGate.finish();
    if (rerun && workspaceController.state.status === "open") {
      const queuedPlan = queuedSnapshotPlanId || board?.planId || 0;
      const queuedGeneration = workspaceController.state.generation;
      queuedSnapshotPlanId = 0;
      queueMicrotask(() => {
        if (workspaceController.state.status === "open" &&
          workspaceController.state.generation === queuedGeneration) {
          void loadSnapshot(queuedPlan);
        }
      });
    } else if (rerun) {
      refreshGate.reset();
    }
  }
  return true;
}

async function loadExactTaskTransitionSnapshot(planId, generation) {
  while (workspaceController.state.status === "open" &&
    workspaceController.state.generation === generation) {
    await refreshGate.whenIdle();
    if (workspaceController.state.status !== "open" ||
      workspaceController.state.generation !== generation ||
      Number(board?.planId) !== Number(planId)) return false;
    if (await loadSnapshot(planId, false, false)) {
      await refreshGate.whenIdle();
      return workspaceController.state.status === "open" &&
        workspaceController.state.generation === generation;
    }
  }
  return false;
}

async function runMutation(operation, progress, failed) {
  if (!board || workspaceController.state.status !== "open") return;
  const ticket = workspaceController.capture();
  const focusKey = document.activeElement?.dataset?.mutationFocusKey || "";
  setStatus(progress);
  try {
    const result = await operation(ticket.generation);
    if (result?.generation && !workspaceController.accepts(ticket, result.generation)) return;
    await loadSnapshot(board.planId);
    restoreMutationFocus(focusKey);
    if (detailTask && !elements.drawer.hidden) {
      // Sync from the fresh snapshot, then reload the full detail.
      const fresh = board?.columns
        ?.flatMap((column) => column.tasks)
        .find((task) => Number(task.id) === Number(detailTask.id));
      if (fresh) {
        detailTask = fresh;
        renderDrawerTask(fresh);
      }
      void loadTaskDetail(detailTask);
    }
  } catch (error) {
    if (ticket.epoch === workspaceController.capture().epoch) {
      showError(error);
      setStatus(failed);
      await loadSnapshot(board.planId, true);
      restoreMutationFocus(focusKey);
    }
  }
}

function restoreMutationFocus(focusKey) {
  if (!focusKey) return;
  const exact = Array.from(document.querySelectorAll("[data-mutation-focus-key]"))
    .find((element) => element.dataset.mutationFocusKey === focusKey);
  if (exact instanceof HTMLElement) {
    exact.focus();
    return;
  }
  const fallback = mutationFocusFallback(focusKey);
  if (fallback === "handoffSend") {
    elements.agentHandoffSend.focus();
  } else if (fallback === "workflowPrepare") {
    elements.agentWorkflowPrepare.focus();
  }
}

function boardTask(taskId) {
  return board?.columns
    ?.flatMap((column) => column.tasks)
    .find((task) => Number(task.id) === Number(taskId));
}

function taskTransitionRequestIsCurrent(request) {
  return taskTransitionRequest === request &&
    taskTransitionSequence === request.sequence &&
    workspaceController.state.status === "open" &&
    workspaceController.state.generation === request.generation;
}

function restoreTaskTransitionControl(request) {
  if (request.invoker instanceof HTMLSelectElement) {
    request.invoker.value = request.fromStatus;
  }
}

function focusTaskTransitionOrigin(request) {
  const intent = taskTransitionFocusIntent(
    request.origin,
    !elements.drawer.hidden,
    Boolean(detailTask && Number(detailTask.id) === request.taskId),
  );
  if (intent === "none") return;
  if (intent === "drawer-select") {
    elements.drawerStatusSelect.focus();
    return;
  }
  if (intent === "card-select") {
    const select = document.querySelector(
      `.card[data-task-id="${request.taskId}"] .card-actions select`,
    );
    if (select instanceof HTMLElement) {
      select.focus();
      return;
    }
    document.querySelector(
      `.card[data-task-id="${request.taskId}"] .card-drag-zone`,
    )?.focus?.();
    return;
  }
  if (request.invoker instanceof HTMLElement && request.invoker.isConnected) {
    request.invoker.focus();
    return;
  }
  document.querySelector(
    `.card[data-task-id="${request.taskId}"] .card-drag-zone`,
  )?.focus?.();
}

function closeTaskTransition(
  restoreState = true,
  restoreFocus = true,
  force = false,
) {
  if (taskTransitionBusy && !force) return;
  const request = taskTransitionRequest;
  taskTransitionSequence += 1;
  taskTransitionBusy = false;
  taskTransitionRequest = null;
  hideApplicationOverlay(elements.taskTransitionModal);
  elements.taskTransitionCancel.disabled = false;
  elements.taskTransitionSubmit.disabled = false;
  if (request?.invoker instanceof HTMLSelectElement) {
    request.invoker.disabled = false;
  }
  if (request && restoreState) restoreTaskTransitionControl(request);
  if (restoreFocus && request) focusTaskTransitionOrigin(request);
}

async function refreshTaskTransitionView(request) {
  const refreshed = await loadExactTaskTransitionSnapshot(
    request.planId,
    request.generation,
  );
  if (!refreshed) return false;
  if (workspaceController.state.status !== "open" ||
    workspaceController.state.generation !== request.generation ||
    Number(board?.planId) !== request.planId) return false;
  const fresh = boardTask(request.taskId);
  if (fresh && detailTask && !elements.drawer.hidden &&
    Number(detailTask.id) === request.taskId) {
    detailTask = fresh;
    renderDrawerTask(fresh);
    await loadTaskDetail(fresh);
  }
  if (workspaceController.state.status !== "open" ||
    workspaceController.state.generation !== request.generation ||
    Number(board?.planId) !== request.planId) return false;
  focusTaskTransitionOrigin(request);
  return true;
}

function openTaskTransitionConfirmation(request, result) {
  const confirmation = result.confirmation;
  request.confirmation = confirmation;
  elements.taskTransitionHeading.textContent =
    `Move task #${request.taskId} to ${statusTitles[request.toStatus]}?`;
  elements.taskTransitionDetail.textContent = taskTransitionConfirmationCopy(
    request.taskId,
    statusTitles[request.fromStatus],
    statusTitles[request.toStatus],
    confirmation,
  );
  elements.taskTransitionMessage.textContent =
    "Confirm to apply this one status change, or cancel to leave the board unchanged.";
  elements.taskTransitionCancel.disabled = false;
  elements.taskTransitionSubmit.disabled = false;
  elements.taskTransitionModal.hidden = false;
  requestAnimationFrame(() => {
    if (taskTransitionRequestIsCurrent(request)) {
      elements.taskTransitionCancel.focus();
    }
  });
}

async function moveTask(taskId, status, invoker = document.activeElement) {
  if (!board || workspaceController.state.status !== "open") return;
  const task = boardTask(taskId);
  if (!task || task.status === status || !statuses.includes(status)) return;
  if (!taskTransitionCanStart(Boolean(taskTransitionRequest), taskTransitionBusy)) {
    if (invoker instanceof HTMLSelectElement) invoker.value = task.status;
    setStatus("Finish the current task status change before starting another.");
    return;
  }
  const sequence = ++taskTransitionSequence;
  const request = {
    sequence,
    generation: workspaceController.state.generation,
    planId: Number(board.planId),
    taskId: Number(taskId),
    fromStatus: task.status,
    toStatus: status,
    invoker: invoker instanceof HTMLElement ? invoker : null,
    origin: invoker === elements.drawerStatusSelect
      ? "drawer-select"
      : invoker instanceof HTMLSelectElement
        ? "card-select"
        : "drag",
    confirmation: null,
  };
  taskTransitionRequest = request;
  taskTransitionBusy = true;
  if (request.invoker instanceof HTMLSelectElement) request.invoker.disabled = true;
  setStatus(`Checking linked resources for task #${taskId}…`);
  try {
    const result = await api().MoveTaskV3(
      request.generation,
      request.taskId,
      request.toStatus,
      "",
    );
    if (!taskTransitionRequestIsCurrent(request)) return;
    if (!taskTransitionResponseIsCurrent(result, request)) {
      throw new Error("Stale task transition response ignored");
    }
    taskTransitionBusy = false;
    if (result.applied) {
      closeTaskTransition(false, false);
      if (await refreshTaskTransitionView(request)) {
        setStatus(`Task #${taskId} moved to ${statusTitles[status]}.`);
      }
      return;
    }
    openTaskTransitionConfirmation(request, result);
  } catch (error) {
    if (!taskTransitionRequestIsCurrent(request)) return;
    taskTransitionBusy = false;
    closeTaskTransition(true, true);
    showError(error);
    if (await refreshTaskTransitionView(request)) {
      setStatus(`Could not move task #${taskId}`);
    }
  }
}

async function confirmTaskTransition() {
  const request = taskTransitionRequest;
  if (!request || taskTransitionBusy || !request.confirmation) return;
  taskTransitionBusy = true;
  elements.taskTransitionCancel.disabled = true;
  elements.taskTransitionSubmit.disabled = true;
  elements.taskTransitionMessage.textContent = "Revalidating linked resources…";
  try {
    const result = await api().MoveTaskV3(
      request.generation,
      request.taskId,
      request.toStatus,
      request.confirmation.token,
    );
    if (!taskTransitionRequestIsCurrent(request)) return;
    if (!taskTransitionResponseIsCurrent(result, request) || !result.applied) {
      throw new Error("Task or linked resources changed; status was not updated");
    }
    taskTransitionBusy = false;
    closeTaskTransition(false, false);
    if (await refreshTaskTransitionView(request)) {
      setStatus(`Task #${request.taskId} moved to ${statusTitles[request.toStatus]}.`);
    }
  } catch (error) {
    if (!taskTransitionRequestIsCurrent(request)) return;
    taskTransitionBusy = false;
    closeTaskTransition(true, true);
    showError(error);
    if (await refreshTaskTransitionView(request)) {
      setStatus(`Could not move task #${request.taskId}`);
    }
  }
}

function openRename(task) {
  dialogMode = "rename";
  editingTask = task;
  dialogReturnFocus = document.activeElement instanceof HTMLElement
    ? document.activeElement
    : null;
  elements.dialogEyebrow.textContent = "Edit card";
  elements.dialogHeading.textContent = `Rename task #${task.id}`;
  elements.dialogLabel.textContent = "Task title";
  elements.dialogLabel.htmlFor = "dialog-input";
  elements.dialogInput.value = task.title;
  elements.dialogInput.hidden = false;
  elements.dialogNote.hidden = true;
  elements.dialogHelp.textContent = "Titles are names; status is tracked separately on the board.";
  elements.dialogSubmit.textContent = "Save changes";
  elements.modal.hidden = false;
  requestAnimationFrame(() => {
    elements.dialogInput.focus();
    elements.dialogInput.select();
  });
}

function openMemory(task) {
  dialogMode = "memory";
  editingTask = task;
  dialogReturnFocus = document.activeElement instanceof HTMLElement
    ? document.activeElement
    : null;
  elements.dialogEyebrow.textContent = "p-track memory";
  elements.dialogHeading.textContent = `Record context for task #${task.id}`;
  elements.dialogLabel.textContent = "Decision or observation";
  elements.dialogLabel.htmlFor = "dialog-note";
  elements.dialogInput.hidden = true;
  elements.dialogNote.value = "";
  elements.dialogNote.hidden = false;
  elements.dialogHelp.textContent =
    "Capture a decision, constraint, or durable observation—not a narration of routine work.";
  elements.dialogSubmit.textContent = "Record memory";
  elements.modal.hidden = false;
  requestAnimationFrame(() => elements.dialogNote.focus());
}

function closeDialog() {
  if (elements.modal.hidden) return;
  editingTask = null;
  hideApplicationOverlay(elements.modal);
  dialogReturnFocus?.focus?.();
  dialogReturnFocus = null;
}

// ------------------------------------------------------ plan lifecycle

function closePlanContextMenu() {
  if (!planContextMenu) return;
  const menu = planContextMenu;
  planContextMenu = null;
  planContextMenuDispose?.();
  planContextMenuDispose = null;
  menu.remove();
  planContextMenuReturnFocus?.focus?.();
  planContextMenuReturnFocus = null;
}

function openPlanContextMenu(plan, titleElement, invoker, position) {
  closePlanContextMenu();
  const menu = document.createElement("div");
  menu.className = "context-menu";
  menu.setAttribute("role", "menu");
  menu.style.visibility = "hidden";
  planMenuItems().forEach((item) => {
    const button = document.createElement("button");
    button.type = "button";
    button.setAttribute("role", "menuitem");
    button.textContent = item.label;
    if (item.destructive) button.classList.add("context-menu-destructive");
    button.addEventListener("click", () => {
      closePlanContextMenu();
      if (item.action === "rename") beginPlanRename(titleElement, plan);
      else if (item.action === "delete") void openPlanDeleteDialog(plan);
      else void openPlanTransferDialog(plan, item.action);
    });
    menu.append(button);
  });
  document.body.append(menu);
  const bounds = menu.getBoundingClientRect();
  const clamped = clampMenuPosition(
    position,
    { width: bounds.width, height: bounds.height },
    { width: window.innerWidth, height: window.innerHeight },
  );
  menu.style.left = `${Math.round(clamped.x)}px`;
  menu.style.top = `${Math.round(clamped.y)}px`;
  menu.style.visibility = "";
  planContextMenu = menu;
  planContextMenuReturnFocus = invoker instanceof HTMLElement ? invoker : null;
  requestAnimationFrame(() => {
    menu.querySelector("button")?.focus();
  });
  const onOutsideClick = (event) => {
    if (!menu.contains(event.target)) closePlanContextMenu();
  };
  const onKeydown = (event) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    closePlanContextMenu();
  };
  const onFocusOut = (event) => {
    // WKWebView never focuses a button on mousedown, so clicking a menu item
    // blurs the focused item with a null relatedTarget; closing here would
    // remove the menu before its click ever dispatches. Outside clicks are
    // covered by the document listener below.
    if (!event.relatedTarget) return;
    if (menu.contains(event.relatedTarget)) return;
    closePlanContextMenu();
  };
  // Deferred so the click/contextmenu event that opened the menu doesn't
  // also register as the outside click that closes it.
  window.setTimeout(() => document.addEventListener("click", onOutsideClick), 0);
  document.addEventListener("keydown", onKeydown);
  menu.addEventListener("focusout", onFocusOut);
  planContextMenuDispose = () => {
    document.removeEventListener("click", onOutsideClick);
    document.removeEventListener("keydown", onKeydown);
    menu.removeEventListener("focusout", onFocusOut);
  };
}

function beginPlanRename(titleElement, plan) {
  if (workspaceController.state.status !== "open") return;
  const original = titleElement.textContent;
  let settled = false;
  planRenameActive = true;
  const input = document.createElement("input");
  input.type = "text";
  input.maxLength = 240;
  input.className = "plan-rename-input";
  input.value = plan.title;
  input.setAttribute("aria-label", `Rename plan #${plan.id}`);

  const restore = () => {
    if (settled) return;
    settled = true;
    planRenameActive = false;
    titleElement.textContent = original;
  };

  const commit = async () => {
    if (settled) return;
    settled = true;
    planRenameActive = false;
    const title = input.value.trim();
    if (!title || title === plan.title) {
      titleElement.textContent = original;
      return;
    }
    const ticket = workspaceController.capture();
    try {
      await api().RenamePlanV1(ticket.generation, Number(plan.id), title);
      await loadSnapshot(board?.planId || 0);
    } catch (error) {
      titleElement.textContent = original;
      showError(error);
      setStatus(`Could not rename plan #${plan.id}`);
    }
  };

  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      void commit();
    } else if (event.key === "Escape") {
      event.preventDefault();
      restore();
    }
  });
  input.addEventListener("blur", () => void commit());

  titleElement.textContent = "";
  titleElement.append(input);
  input.focus();
  input.select();
}

function closePlanDialog() {
  if (elements.planDialog.hidden) return;
  hideApplicationOverlay(elements.planDialog);
  planDialogReturnFocus?.focus?.();
  planDialogReturnFocus = null;
  planDialogMode = null;
  planDialogPlan = null;
  planDialogTransferState = null;
  elements.planDialogError.hidden = true;
  elements.planDialogError.textContent = "";
  elements.planDialogSubmit.classList.remove("dialog-danger");
}

function setPlanDialogError(error) {
  elements.planDialogError.textContent = messageFrom(error);
  elements.planDialogError.hidden = false;
}

function openPlanDialogShell() {
  planDialogReturnFocus =
    document.activeElement instanceof HTMLElement ? document.activeElement : null;
  elements.planDialogError.hidden = true;
  elements.planDialogError.textContent = "";
  elements.planDialogSubmit.classList.remove("dialog-danger");
  elements.planDialog.hidden = false;
}

async function openPlanDeleteDialog(plan) {
  if (workspaceController.state.status !== "open") return;
  const ticket = workspaceController.capture();
  let response;
  try {
    response = await api().DeletePlanV1(ticket.generation, Number(plan.id), false);
  } catch (error) {
    showError(error);
    setStatus(`Could not preview delete for plan #${plan.id}`);
    return;
  }
  if (!workspaceController.accepts(ticket, Number(response.generation))) return;
  planDialogMode = "delete";
  planDialogPlan = plan;
  planDialogTransferState = null;
  openPlanDialogShell();
  elements.planDialogEyebrow.textContent = "Delete plan";
  elements.planDialogHeading.textContent = `Delete “${response.summary.title}”?`;
  elements.planDialogBody.textContent = deleteConfirmationText(response.summary);
  elements.planDialogProjectLabel.hidden = true;
  elements.planDialogProject.hidden = true;
  elements.planDialogTitleLabel.hidden = true;
  elements.planDialogTitle.hidden = true;
  elements.planDialogSubmit.textContent = "Delete plan";
  elements.planDialogSubmit.classList.add("dialog-danger");
  elements.planDialogSubmit.disabled = false;
  requestAnimationFrame(() => elements.planDialogCancel.focus());
}

function syncPlanTransferState() {
  if (!planDialogTransferState) return;
  planDialogTransferState.targetPath = elements.planDialogProject.value;
  planDialogTransferState.title = elements.planDialogTitle.value;
  elements.planDialogSubmit.disabled = transferSubmitDisabled(planDialogTransferState);
}

async function openPlanTransferDialog(plan, mode) {
  if (workspaceController.state.status !== "open") return;
  const ticket = workspaceController.capture();
  let response;
  try {
    response = await api().ListProjectsV1(ticket.generation);
  } catch (error) {
    showError(error);
    setStatus("Could not list projects");
    return;
  }
  if (!workspaceController.accepts(ticket, Number(response.generation))) return;
  const projects = response.projects || [];
  const hasOtherProject = projects.some((project) => !project.current);
  planDialogMode = mode;
  planDialogPlan = plan;
  planDialogTransferState = { mode, projects, targetPath: "", title: "" };
  openPlanDialogShell();
  elements.planDialogEyebrow.textContent = mode === "move" ? "Move plan" : "Copy plan";
  elements.planDialogHeading.textContent =
    `${mode === "move" ? "Move" : "Copy"} “${plan.title}”`;
  elements.planDialogBody.textContent = mode === "move"
    ? "Choose a project to move this plan to."
    : "Choose a project to copy this plan into. Copying into the current project needs a new title.";
  if (!hasOtherProject) {
    elements.planDialogBody.textContent +=
      " No other projects registered — run ptrack init in another repository first.";
  }
  elements.planDialogProject.replaceChildren();
  const placeholder = document.createElement("option");
  placeholder.value = "";
  placeholder.textContent = "Choose a project…";
  elements.planDialogProject.append(placeholder);
  projects.forEach((project) => {
    const option = document.createElement("option");
    option.value = project.path;
    option.textContent = project.current
      ? `${project.name} — ${project.path} (this project)`
      : `${project.name} — ${project.path}`;
    elements.planDialogProject.append(option);
  });
  elements.planDialogProject.value = "";
  elements.planDialogTitle.value = "";
  elements.planDialogProjectLabel.hidden = false;
  elements.planDialogProject.hidden = false;
  elements.planDialogTitleLabel.hidden = false;
  elements.planDialogTitle.hidden = false;
  elements.planDialogSubmit.textContent = "OK";
  syncPlanTransferState();
  requestAnimationFrame(() => elements.planDialogProject.focus());
}

function openMemoryHistory() {
  memoryModalReturnFocus = document.activeElement;
  elements.memoryModal.hidden = false;
  requestAnimationFrame(() => elements.memoryDialogClose.focus());
}

function closeMemoryHistory() {
  hideApplicationOverlay(elements.memoryModal);
  memoryModalReturnFocus?.focus();
  memoryModalReturnFocus = null;
}

function renderUpdateState(nextState) {
  if (!nextState || !updateStateIsNewer(updateState, nextState)) return;
  updateState = nextState;
  const presentation = updatePresentation(nextState);
  const release = nextState.release || null;
  const progress = updateProgress(nextState);
  const currentVersion = appVersionLabel(nextState.currentVersion || "dev");

  setUpdateText(elements.updatesCurrentVersion, `Current version: ${currentVersion}`);
  setUpdateText(elements.aboutVersion, currentVersion);
  setUpdateText(elements.aboutBuild, aboutBuildLabel(nextState.phase));
  elements.updatesAutomatic.checked = Boolean(nextState.automaticChecks);
  elements.settingsUpdatesAutomatic.checked = Boolean(nextState.automaticChecks);
  elements.updatesStatus.dataset.tone = presentation.tone;
  setAriaBoolean(elements.updatesStatus, "aria-busy", presentation.busy);
  setUpdateText(elements.updatesStatusTitle, presentation.title);
  setUpdateText(elements.updatesStatusDetail, presentation.detail);
  elements.updatesProgressWrap.hidden = nextState.phase !== "downloading";
  elements.updatesProgress.value = progress.percent;
  elements.updatesProgressLabel.textContent = progress.total > 0
    ? `${progress.percent}% · ${formatUpdateBytes(progress.downloaded)} of ${formatUpdateBytes(progress.total)}`
    : `${formatUpdateBytes(progress.downloaded)} downloaded`;

  elements.updatesRelease.hidden = !release;
  elements.updatesReleaseVersion.textContent = release ? `Version ${release.version}` : "";
  elements.updatesReleaseMeta.textContent = release ? updateReleaseMeta(release) : "";
  elements.updatesReleaseNotes.textContent = release?.notes || "No release notes were provided.";
  elements.updatesReleasePage.hidden = !release?.pageUrl;
  elements.updatesVerified.hidden = !nextState.checksumVerified;
  elements.updatesCancel.hidden = !presentation.cancel;
  elements.updatesCancel.disabled = !presentation.cancel;
  elements.updatesPrimary.hidden = !presentation.primaryAction;
  elements.updatesPrimary.disabled = updateActionBusy || !presentation.primaryAction;
  elements.updatesPrimary.dataset.action = presentation.primaryAction || "";
  elements.updatesPrimary.textContent = presentation.primaryLabel;
}

function setUpdateText(element, value) {
  if (element.textContent !== value) element.textContent = value;
}

// The build line states the platform this window runs on and whether the
// build can receive packaged updates at all.
function aboutBuildLabel(phase) {
  const platform = navigator.userAgentData?.platform || navigator.platform ||
    "desktop";
  return `${platform} · ${phase === "unavailable" ? "unpackaged build" : "packaged release"}`;
}

function setAriaBoolean(element, attribute, value) {
  if (value) element.setAttribute(attribute, "true");
  else element.removeAttribute(attribute);
}

function updateReleaseMeta(release) {
  const parts = [];
  if (release.publishedAt) {
    const published = new Date(release.publishedAt);
    if (Number.isFinite(published.getTime())) {
      parts.push(published.toLocaleDateString(undefined, {
        year: "numeric",
        month: "short",
        day: "numeric",
      }));
    }
  }
  if (Number(release.sizeBytes) > 0) parts.push(formatUpdateBytes(release.sizeBytes));
  return parts.join(" · ");
}

async function refreshUpdateState() {
  try {
    renderUpdateState(await api().GetUpdateState());
  } catch {
    showError(new Error("Could not load update status."));
  }
}

function openAboutUpdates(invoker = document.activeElement) {
  if (
    firstRunState.phase !== "idle" ||
    firstPlanState.phase !== "idle" ||
    recentProjectOperationActive()
  ) return false;
  const competingOverlayOpen = nativeMenuOpenOverlayIDs().some(
    (overlayID) => overlayID !== "updates-modal",
  );
  if (competingOverlayOpen) return false;
  const transition = updateModalOpenTransition(
    elements.updatesModal.hidden,
    updatesModalReturnFocus,
    invoker instanceof HTMLElement ? invoker : null,
  );
  updatesModalReturnFocus = transition.returnFocus;
  if (transition.makeVisible) elements.updatesModal.hidden = false;
  renderUpdateState(updateState);
  void refreshUpdateState();
  if (transition.scheduleOpeningFocus) {
    requestAnimationFrame(() => elements.updatesClose.focus());
  }
  return true;
}

function closeAboutUpdates() {
  if (elements.updatesModal.hidden) return;
  hideApplicationOverlay(elements.updatesModal);
  updatesModalReturnFocus?.focus?.();
  updatesModalReturnFocus = null;
}

async function runUpdateAction(action) {
  if (updateActionBusy) return;
  if (!["check", "download", "apply"].includes(action)) return;
  updateCancelRequested = false;
  const version = updateState.release?.version || "";
  if ((action === "download" || action === "apply") && !version) {
    await refreshUpdateState();
    return;
  }
  updateActionBusy = true;
  renderUpdateState(updateState);
  try {
    let state;
    if (action === "check") state = await api().CheckForUpdates();
    if (action === "download") state = await api().DownloadUpdate(version);
    if (action === "apply") state = await api().ApplyUpdate(version);
    if (state) renderUpdateState(state);
  } catch {
    await refreshUpdateState();
    if (!updateCancelRequested) {
      showError(new Error("The update action could not continue safely."));
    }
  } finally {
    updateActionBusy = false;
    updateCancelRequested = false;
    renderUpdateState(updateState);
  }
}

async function setAutomaticUpdateChecks(enabled) {
  elements.updatesAutomatic.disabled = true;
  elements.settingsUpdatesAutomatic.disabled = true;
  try {
    renderUpdateState(await api().SetAutomaticUpdateChecks(Boolean(enabled)));
  } catch {
    await refreshUpdateState();
    showError(new Error("Could not save the automatic update preference."));
  } finally {
    elements.updatesAutomatic.disabled = false;
    elements.settingsUpdatesAutomatic.disabled = false;
  }
}

const projectRepositoryURL = "https://github.com/ro-ag/ptrack";
const projectLicenseURL = `${projectRepositoryURL}/blob/main/LICENSE`;

// External links always travel through the validated native opener; the
// browser fallback only matters in a plain dev server.
function openProjectURL(url) {
  if (!url.startsWith(projectRepositoryURL)) return;
  if (typeof window.runtime?.BrowserOpenURL === "function") {
    window.runtime.BrowserOpenURL(url);
    return;
  }
  window.open(url, "_blank", "noopener,noreferrer");
}

function openUpdateReleasePage() {
  const page = updateState.release?.pageUrl || "";
  if (!page.startsWith(`${projectRepositoryURL}/releases/`)) return;
  openProjectURL(page);
}

// ------------------------------------------------------------ settings

function applyPreferences(next) {
  // The startup opt-in is what decides the Welcome preselect, so turning it
  // off clears the highlight now rather than at the next list load. Every
  // other preference leaves the list alone: rebuilding it would drop focus.
  const startupChanged =
    preferences.startup.restoreLastProject !== next.startup.restoreLastProject ||
    preferences.startup.lastProjectRoot !== next.startup.lastProjectRoot;
  preferences = next;
  themeController.setTheme(next.appearance.theme);
  const root = document.documentElement;
  root.dataset.density = next.appearance.density;
  if (next.appearance.reducedMotion === "system") {
    delete root.dataset.reducedMotion;
  } else {
    root.dataset.reducedMotion = next.appearance.reducedMotion;
  }
  applyPreferenceMirrors(localStorage, next);
  renderPreferences();
  if (startupChanged) renderRecentProjects();
}

function renderPreferences() {
  elements.settingsStartupRestore.checked = preferences.startup.restoreLastProject;
  elements.settingsTheme.value = preferences.appearance.theme;
  elements.settingsDensity.value = preferences.appearance.density;
  elements.settingsReducedMotion.value = preferences.appearance.reducedMotion;
  renderTerminalProfilePreference();
  elements.settingsTerminalFontFamily.value = preferences.terminal.fontFamily;
  elements.settingsTerminalFontSize.value = String(preferences.terminal.fontSize);
  elements.settingsTerminalUnicode.value = preferences.terminal.unicodeMode;
  elements.settingsTerminalScrollback.value = String(preferences.terminal.scrollback);
  elements.settingsTerminalRenderer.value = preferences.terminal.renderer;
}

// A stored default profile that no longer resolves is reported as
// unavailable instead of being coerced onto an installed profile.
function renderTerminalProfilePreference() {
  const stored = preferences.terminal.defaultProfileId || "";
  const select = elements.settingsTerminalProfile;
  const missing = select.querySelector("[data-unavailable-profile]");
  if (missing) missing.remove();
  if (stored && !select.querySelector(`option[value="${CSS.escape(stored)}"]`)) {
    const option = document.createElement("option");
    option.value = stored;
    option.textContent = `${stored} · unavailable`;
    option.dataset.unavailableProfile = "true";
    select.append(option);
  }
  select.value = stored;
}

async function loadTerminalProfileOptions() {
  let profiles = [];
  try {
    profiles = await api().GetTerminalProfiles();
  } catch {
    profiles = [];
  }
  const select = elements.settingsTerminalProfile;
  const installed = select.querySelectorAll("option:not([value=''])");
  installed.forEach((option) => option.remove());
  for (const profile of profiles) {
    const option = document.createElement("option");
    option.value = profile.id;
    option.textContent = `${profile.name}${profile.kind === "agent" ? " · agent" : ""}`;
    select.append(option);
  }
  renderTerminalProfilePreference();
}

function renderSettingsStorageNotice(status) {
  const notice = storageStatusNotice(status);
  elements.settingsStorageNotice.textContent = notice;
  elements.settingsStorageNotice.hidden = notice === "";
}

// The dialog's single live region. It sits outside the aria-busy wrapper, so
// a long reset is still announced.
//
// The element never leaves the DOM — removing it is what breaks announcements —
// but its text is transient: a confirmation that stays on screen stops reading
// as "that just happened" and starts reading as a permanent label. Clearing the
// text does not retract what was already announced. Nothing moves or fades, so
// there is no motion for a reduced-motion preference to have an opinion about.
// A failure stays until the next action: it is the one thing left to act on.
function setSettingsStatus(message, failed = false, sticky = false) {
  clearTimeout(settingsStatusTimer);
  elements.settingsSaveStatus.textContent = message;
  elements.settingsSaveStatus.dataset.tone = failed ? "error" : "";
  if (message === "" || failed || sticky) return;
  settingsStatusTimer = setTimeout(() => {
    elements.settingsSaveStatus.textContent = "";
  }, settingsStatusClearDelay);
}

function setSettingsSaveStatus(phase) {
  setSettingsStatus(
    phase ? preferenceSaveMessage(phase) : "",
    phase === "failed",
    // "Saving…" is superseded by its own outcome, so it must not time out and
    // leave a slow save looking like nothing was ever asked for.
    phase === "saving",
  );
}

async function loadPreferences() {
  try {
    const response = preferencesResponse(await api().GetPreferences());
    applyPreferences(response.preferences);
    renderSettingsStorageNotice(response.storage);
  } catch {
    // The cached values are what this window is already using, so they are
    // shown as-is rather than being replaced with defaults.
    preferences = preferencesFromMirrors(localStorage);
    renderPreferences();
    renderSettingsStorageNotice("unavailable");
  }
}

async function savePreferences(patch) {
  const sequence = ++settingsSaveSequence;
  setSettingsSaveStatus("saving");
  elements.settingsBody.setAttribute("aria-busy", "true");
  try {
    const response = preferencesResponse(await api().SetPreferences(patch));
    if (sequence !== settingsSaveSequence) return;
    applyPreferences(response.preferences);
    renderSettingsStorageNotice(response.storage);
    setSettingsSaveStatus("saved");
  } catch {
    if (sequence !== settingsSaveSequence) return;
    renderPreferences();
    setSettingsSaveStatus("failed");
  } finally {
    if (sequence === settingsSaveSequence) {
      elements.settingsBody.removeAttribute("aria-busy");
    }
  }
}

async function resetPreferences() {
  const sequence = ++settingsSaveSequence;
  setSettingsSaveStatus("saving");
  elements.settingsReset.disabled = true;
  try {
    const response = preferencesResponse(await api().ResetPreferences());
    if (sequence !== settingsSaveSequence) return;
    applyPreferences(response.preferences);
    renderSettingsStorageNotice(response.storage);
    setSettingsSaveStatus("reset");
  } catch {
    if (sequence === settingsSaveSequence) setSettingsSaveStatus("failed");
  } finally {
    elements.settingsReset.disabled = false;
  }
}

async function resetWindowLayout(invoker) {
  if (!(await showConfirmation(resetWindowLayoutConfirmation, invoker))) return;
  elements.settingsResetWindowLayout.disabled = true;
  try {
    applyLayoutState(normalizeLayoutState(await api().ResetWindowLayout()));
    // Sticky: a reset outcome is the result of an explicit destructive action
    // and the one thing left to read. Clearing it also collapses several
    // wrapped lines out of the footer, which moves the button underneath it
    // six seconds after anyone last touched anything.
    setSettingsStatus("Window layout reset to defaults.", false, true);
  } catch (error) {
    setSettingsStatus(messageFrom(error), true);
  } finally {
    elements.settingsResetWindowLayout.disabled = false;
  }
}

// The runtime cannot reach WebView storage, so the saved terminal workspaces
// are cleared here. A dock that is still open keeps its live tabs and saves
// them again on the next change.
function clearTerminalWorkspaceDescriptors() {
  try {
    for (const key of Object.keys(localStorage)) {
      if (key.startsWith(terminalWorkspaceStoragePrefix)) localStorage.removeItem(key);
    }
  } catch {
    // Persistence is optional.
  }
}

async function resetApplicationState(invoker) {
  if (!(await showConfirmation(resetApplicationStateConfirmation, invoker))) return;
  elements.settingsResetApplicationState.disabled = true;
  try {
    const result = await api().ResetApplicationState();
    clearTerminalWorkspaceDescriptors();
    applyLayoutState(defaultLayoutState());
    await loadPreferences();
    void refreshUpdateState();
    // Sticky for the same reason as the layout reset, and more so: this
    // message is three clauses long and wraps to about four lines.
    setSettingsStatus(resetApplicationStateMessage(result), false, true);
  } catch (error) {
    setSettingsStatus(messageFrom(error), true);
  } finally {
    elements.settingsResetApplicationState.disabled = false;
  }
}

async function loadDiagnosticsReport() {
  const request = ++settingsDiagnosticsRequest;
  try {
    const report = await api().GetDiagnosticsReport();
    if (request === settingsDiagnosticsRequest) renderDiagnosticsReport(report);
  } catch {
    if (request === settingsDiagnosticsRequest) renderDiagnosticsReport(null);
  }
}

function renderDiagnosticsReport(report) {
  const rows = diagnosticsRows(report);
  elements.settingsDiagnostics.replaceChildren();
  if (rows.length === 0) {
    const empty = document.createElement("dt");
    empty.className = "dialog-help";
    empty.textContent = "No diagnostics are available yet.";
    elements.settingsDiagnostics.append(empty, document.createElement("dd"));
    return;
  }
  for (const row of rows) {
    const group = document.createElement("div");
    group.className = "settings-diagnostic";
    const term = document.createElement("dt");
    term.textContent = row.label;
    const description = document.createElement("dd");
    const value = document.createElement("span");
    value.className = "settings-diagnostic-value";
    value.textContent = row.value;
    description.append(value);
    if (row.detail) {
      const detail = document.createElement("span");
      detail.className = "settings-diagnostic-detail";
      detail.textContent = row.detail;
      description.append(detail);
    }
    // A word-wrapping "Copy" label is what broke this column, so the control is
    // an icon that cannot break. Its accessible name says what it copies, and
    // the title repeats it for pointer users who get no label at all.
    if (row.copy) {
      const copy = document.createElement("button");
      copy.type = "button";
      copy.className = "settings-diagnostic-copy";
      copy.setAttribute("aria-label", row.copy);
      copy.title = row.copy;
      const icon = svgElement("svg", { viewBox: "0 0 16 16", "aria-hidden": "true" });
      icon.append(
        svgElement("rect", { x: "5.5", y: "5.5", width: "8.5", height: "8.5", rx: "2" }),
        svgElement("path", { d: "M10.5 5.5V4a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2v4.5a2 2 0 0 0 2 2h1.5" }),
      );
      copy.append(icon);
      copy.addEventListener("click", () => void copyDiagnosticValue(row));
      description.append(copy);
    }
    group.append(term, description);
    elements.settingsDiagnostics.append(group);
  }
}

// Copy confirmations go through the one live region the dialog has, so they are
// announced and then clear on the same terms as every other status.
async function copyDiagnosticValue(row) {
  try {
    if ((await window.runtime?.ClipboardSetText?.(row.value)) !== true) {
      throw new Error("clipboard unavailable");
    }
    setSettingsStatus(`${row.label} copied.`);
  } catch {
    showError(new Error(`Could not copy ${row.label}.`));
  }
}

function settingsAvailable() {
  return firstRunState.phase === "idle" &&
    firstPlanState.phase === "idle" &&
    !recentProjectOperationActive();
}

function openSettings(invoker = document.activeElement) {
  if (!settingsAvailable()) return false;
  const competingOverlayOpen = nativeMenuOpenOverlayIDs().some(
    (overlayID) => overlayID !== "settings-modal",
  );
  if (competingOverlayOpen) return false;
  const wasHidden = elements.settingsModal.hidden;
  if (wasHidden) {
    settingsModalReturnFocus = invoker instanceof HTMLElement ? invoker : null;
    elements.settingsModal.hidden = false;
    setSettingsSaveStatus("");
  }
  void loadPreferences();
  void loadTerminalProfileOptions();
  void loadDiagnosticsReport();
  void refreshUpdateState();
  if (wasHidden) {
    requestAnimationFrame(() => selectSettingsSection(settingsSection, true));
  }
  return true;
}

function closeSettings() {
  if (elements.settingsModal.hidden) return;
  hideApplicationOverlay(elements.settingsModal);
  settingsModalReturnFocus?.focus?.();
  settingsModalReturnFocus = null;
}

function selectSettingsSection(section, focus = false) {
  settingsSection = section;
  for (const entry of settingsSections) {
    const tab = document.getElementById(settingsTabId(entry.id));
    const panel = document.getElementById(settingsPanelId(entry.id));
    const active = entry.id === section;
    tab.setAttribute("aria-selected", String(active));
    tab.tabIndex = active ? 0 : -1;
    panel.hidden = !active;
  }
  if (focus) document.getElementById(settingsTabId(section)).focus();
  if (section === "data") void loadDiagnosticsReport();
}

const paletteKindLabels = {
  plan: "Plan",
  task: "Task",
  note: "Note",
};

function openPalette() {
  if (
    workspaceController.state.status !== "open" ||
    firstPlanState.phase !== "idle"
  ) return;
  paletteReturnFocus = document.activeElement;
  elements.palette.hidden = false;
  renderPaletteResults();
  if (elements.paletteInput.value.trim()) void runPaletteSearch();
  requestAnimationFrame(() => {
    elements.paletteInput.focus();
    elements.paletteInput.select();
  });
}

function closePalette() {
  if (elements.palette.hidden) return;
  window.clearTimeout(paletteTimer);
  paletteSequence += 1;
  hideApplicationOverlay(elements.palette);
  paletteItems = [];
  paletteActive = -1;
  paletteReturnFocus?.focus?.();
  paletteReturnFocus = null;
}

function schedulePaletteSearch() {
  window.clearTimeout(paletteTimer);
  paletteTimer = window.setTimeout(() => void runPaletteSearch(), 150);
}

async function runPaletteSearch() {
  const query = elements.paletteInput.value.trim();
  const request = ++paletteSequence;
  if (!query) {
    paletteItems = [];
    paletteActive = -1;
    renderPaletteResults();
    return;
  }
  try {
    const results = await api().SearchV2(query);
    if (request !== paletteSequence || elements.palette.hidden) return;
    paletteItems = results;
    paletteActive = results.length ? 0 : -1;
    renderPaletteResults();
  } catch (error) {
    if (request !== paletteSequence || elements.palette.hidden) return;
    showError(error);
  }
}

function paletteEmptyState(message) {
  const empty = document.createElement("div");
  empty.className = "palette-empty";
  empty.textContent = message;
  return empty;
}

function renderPaletteResults() {
  elements.paletteResults.replaceChildren();
  if (!elements.paletteInput.value.trim()) {
    elements.paletteResults.append(
      paletteEmptyState("Search across plans, tasks, and memory notes."),
    );
    elements.paletteInput.removeAttribute("aria-activedescendant");
    return;
  }
  if (paletteItems.length === 0) {
    elements.paletteResults.append(paletteEmptyState("No matches."));
    elements.paletteInput.removeAttribute("aria-activedescendant");
    return;
  }
  let flatIndex = 0;
  groupSearchResults(paletteItems).forEach((group) => {
    const section = document.createElement("div");
    section.className = "palette-group";
    const label = document.createElement("p");
    label.className = "palette-group-label";
    label.textContent = group.label;
    section.append(label);
    group.items.forEach((result) => {
      const index = flatIndex;
      const option = document.createElement("div");
      option.className = "palette-option";
      option.id = `palette-option-${index}`;
      option.role = "option";
      option.setAttribute("aria-selected", String(index === paletteActive));
      if (index === paletteActive) option.classList.add("active");
      const badge = document.createElement("span");
      badge.className = "palette-kind";
      badge.dataset.kind = result.kind;
      badge.textContent = paletteKindLabels[result.kind] || result.kind;
      const body = document.createElement("div");
      body.className = "palette-option-body";
      const title = document.createElement("p");
      title.className = "palette-option-title";
      title.textContent =
        result.kind === "note" ? result.title : `#${result.id} ${result.title}`;
      body.append(title);
      if (result.snippet) {
        const snippet = document.createElement("p");
        snippet.className = "palette-option-snippet";
        snippet.textContent = result.snippet;
        body.append(snippet);
      }
      option.append(badge, body);
      option.addEventListener("click", () => activatePaletteResult(result));
      option.addEventListener("mousemove", () => {
        if (paletteActive !== index) {
          paletteActive = index;
          renderPaletteResults();
        }
      });
      section.append(option);
      flatIndex += 1;
    });
    elements.paletteResults.append(section);
  });
  const active = elements.paletteResults.querySelector(".palette-option.active");
  if (active) {
    elements.paletteInput.setAttribute("aria-activedescendant", active.id);
    active.scrollIntoView({ block: "nearest" });
  } else {
    elements.paletteInput.removeAttribute("aria-activedescendant");
  }
}

function movePaletteActive(delta) {
  if (paletteItems.length === 0) return;
  paletteActive = focusCycleIndex(
    paletteItems.length,
    paletteActive,
    delta < 0,
  );
  renderPaletteResults();
}

function activatePaletteResult(result) {
  if (!result) return;
  const target = paletteTarget(result);
  closePalette();
  if (target.view === "overview") {
    setView("overview");
    return;
  }
  pendingDetailTaskId = target.taskId;
  setView("board");
  if (Number(board?.planId) === Number(target.planId)) {
    openPendingTaskDetail();
  } else {
    selectPlan(target.planId);
  }
}

// Opens the drawer for a task chosen in the palette once the board for its
// plan has loaded. Called from the snapshot success path and directly when
// the task's plan is already selected.
function openPendingTaskDetail() {
  if (!pendingDetailTaskId || !board) return;
  const task = board.columns
    .flatMap((column) => column.tasks)
    .find((candidate) => Number(candidate.id) === Number(pendingDetailTaskId));
  pendingDetailTaskId = 0;
  if (task) openTaskDetail(task);
}

function drawerEmptyState(message) {
  const empty = document.createElement("div");
  empty.className = "drawer-empty";
  empty.textContent = message;
  return empty;
}

function renderDrawerTask(task) {
  elements.drawerEyebrow.textContent = `Task · #${task.id}`;
  elements.drawerTitle.textContent = task.title;
  elements.drawerStatus.dataset.status = task.status;
  elements.drawerStatus.textContent = statusTitles[task.status] || task.status;
  elements.drawerUpdated.textContent = task.updatedAt
    ? `updated ${relativeTime(task.updatedAt)}`
    : "";
  elements.drawerStatusSelect.replaceChildren();
  statuses.forEach((status) => {
    const option = document.createElement("option");
    option.value = status;
    option.textContent = statusTitles[status];
    option.selected = status === task.status;
    elements.drawerStatusSelect.append(option);
  });
  renderDrawerRuntimeSummary(task.linkedRuntime);
}

function renderDrawerRuntimeSummary(summary) {
  const presentation = linkedTaskRuntimePresentation(summary);
  elements.drawerRuntimeCount.textContent = presentation
    ? presentation.compact
    : "0";
  elements.drawerRuntime.replaceChildren(
    drawerEmptyState(
      presentation
        ? presentation.detail
        : "No current terminal or agent is linked to this task.",
    ),
  );
}

function renderDrawerRuntimeDetail(linkedRuntime, agentIntelligence = []) {
  const summary = linkedRuntime?.summary;
  const presentation = linkedTaskRuntimePresentation(summary);
  const terminals = linkedRuntime?.terminals || [];
  const agents = linkedRuntime?.agents || [];
  const intelligenceByRun = new Map(
    (agentIntelligence || []).map((entry) => [entry.runId, entry]),
  );
  elements.drawerRuntimeCount.textContent = presentation
    ? presentation.compact
    : "0";
  elements.drawerRuntime.replaceChildren();
  if (!presentation) {
    elements.drawerRuntime.append(
      drawerEmptyState("No current terminal or agent is linked to this task."),
    );
    return;
  }
  terminals.forEach((session) => {
    elements.drawerRuntime.append(
      intelligenceItem(
        `Terminal · ${session.profileKind}`,
        `${session.live ? "live" : "historical"} · ${session.state} · ${session.profileKind}`,
        session.state === "failed" ? "error" : "",
      ),
    );
  });
  agents.forEach((run) => {
    const origin = run.terminalBacked
      ? run.correspondingTerminal
        ? "paired with linked terminal"
        : run.terminalPresent
          ? "terminal present · association does not correspond"
          : "terminal unavailable"
      : "external";
    elements.drawerRuntime.append(
      intelligenceItem(
        `${run.terminalBacked ? "Terminal-backed" : "External"} agent`,
        `${run.live ? "live" : "historical"} · lifecycle ${run.state} · process ${run.processState} · lease ${run.leaseState} · ${origin}` +
          `${agentIntelligenceLabel(run.intelligence) ? ` · ${agentIntelligenceLabel(run.intelligence)}` : ""}`,
        run.state === "stale" ? "stale" : "",
      ),
    );
    const intelligence = intelligenceByRun.get(run.runId);
    if (intelligence) {
      const intelligenceEntry = intelligenceItem(
        `Agent intelligence · ${intelligence.intelligence.state}`,
        `${intelligence.intelligence.confidence || "low"} confidence · ${intelligence.eventBounds?.total || 0} retained structured events`,
        intelligence.intelligence.state === "failed" ? "error" :
          intelligence.intelligence.state === "potentiallyDrifting" ? "stale" : "",
      );
      const handoffButton = document.createElement("button");
      handoffButton.type = "button";
      handoffButton.className = "button-secondary";
      handoffButton.textContent = "Preview handoff";
      const handoffPreview = document.createElement("pre");
      handoffPreview.className = "intelligence-detail";
      handoffPreview.hidden = true;
      handoffPreview.style.whiteSpace = "pre-wrap";
      const handoffTaskId = Number(detailTask?.id || 0);
      const handoffAssociation = intelligence.association;
      handoffButton.addEventListener("click", async () => {
        const ticket = workspaceController.capture();
        handoffButton.disabled = true;
        handoffButton.textContent = "Generating preview…";
        try {
          const result = await api().PreviewAgentHandoffV2(ticket.generation, run.runId);
          if (!workspaceController.accepts(ticket, Number(result.generation))) return;
          if (!handoffPreviewResponseIsCurrent(
            handoffTaskId,
            handoffAssociation,
            result.association,
            Number(detailTask?.id || 0),
          )) return;
          handoffPreview.textContent = `${result.preview.text}\n\nPreview only · project memory was not changed.`;
          handoffPreview.hidden = false;
        } catch (error) {
          showError(error);
        } finally {
          handoffButton.disabled = false;
          handoffButton.textContent = "Refresh handoff preview";
        }
      });
      intelligenceEntry.append(handoffButton, handoffPreview);
      elements.drawerRuntime.append(intelligenceEntry);
      (intelligence.suggestions || []).forEach((suggestion) => {
        elements.drawerRuntime.append(
          intelligenceItem(
            `Suggestion · ${suggestion.kind}`,
            `${suggestion.label} · ${suggestion.reason}`,
          ),
        );
      });
    }
  });
  const terminalRowsMore = Number(linkedRuntime?.terminalRowsMore || 0);
  const agentRowsMore = Number(linkedRuntime?.agentRowsMore || 0);
  if (terminalRowsMore || agentRowsMore) {
    elements.drawerRuntime.append(
      drawerEmptyState(
        `${terminalRowsMore} more terminal${terminalRowsMore === 1 ? "" : "s"} · ` +
        `${agentRowsMore} more agent${agentRowsMore === 1 ? "" : "s"}`,
      ),
    );
  }
}

function renderDrawerLoading() {
  elements.drawerRuntimeCount.textContent = "…";
  elements.drawerRuntime.replaceChildren(drawerEmptyState("Loading linked runtime…"));
  elements.drawerNotesCount.textContent = "…";
  elements.drawerCommitsCount.textContent = "…";
  elements.drawerIssuesCount.textContent = "…";
  elements.drawerNotes.replaceChildren(drawerEmptyState("Loading notes…"));
  elements.drawerCommits.replaceChildren(drawerEmptyState("Loading commits…"));
  elements.drawerIssues.replaceChildren(drawerEmptyState("Loading issues…"));
}

function drawerNoteElement(note) {
  const item = document.createElement("article");
  item.className = "drawer-note";
  const body = document.createElement("p");
  body.className = "drawer-note-body";
  body.textContent = note.body;
  const meta = document.createElement("span");
  meta.className = "drawer-item-meta";
  meta.textContent = `${note.kind || "note"} · ${relativeTime(note.occurredAt)}`;
  item.append(body, meta);
  return item;
}

function drawerCommitElement(commit) {
  const item = document.createElement("article");
  item.className = "drawer-commit";
  const row = document.createElement("p");
  row.className = "drawer-commit-title";
  const sha = document.createElement("span");
  sha.className = "drawer-sha";
  sha.textContent = commit.sha.slice(0, 8);
  row.append(sha, document.createTextNode(commit.subject));
  const meta = document.createElement("span");
  meta.className = "drawer-item-meta";
  meta.textContent = relativeTime(commit.occurredAt);
  item.append(row, meta);
  return item;
}

function drawerIssueElement(issue) {
  const item = document.createElement("article");
  item.className = "drawer-issue";
  item.style.setProperty(
    "--issue-color",
    severityColors[issue.severity] || "var(--muted)",
  );
  const title = document.createElement("p");
  title.className = "drawer-issue-title";
  title.textContent = issue.title;
  const meta = document.createElement("span");
  meta.className = "drawer-item-meta";
  meta.textContent = `${issue.severity} · issue #${issue.id}`;
  item.append(title, meta);
  return item;
}

function renderDrawerSections(detail) {
  renderDrawerRuntimeDetail(detail.linkedRuntime, detail.agentIntelligence);
  elements.drawerNotesCount.textContent = detail.notes.length;
  elements.drawerCommitsCount.textContent = detail.commits.length;
  elements.drawerIssuesCount.textContent = detail.issues.length;
  elements.drawerNotes.replaceChildren();
  if (detail.notes.length === 0) {
    elements.drawerNotes.append(
      drawerEmptyState("No memory recorded yet. Use “Record memory” to capture a decision."),
    );
  } else {
    detail.notes.forEach((note) => elements.drawerNotes.append(drawerNoteElement(note)));
  }
  elements.drawerCommits.replaceChildren();
  if (detail.commits.length === 0) {
    elements.drawerCommits.append(
      drawerEmptyState("No commits linked to this task yet."),
    );
  } else {
    detail.commits.forEach((commit) =>
      elements.drawerCommits.append(drawerCommitElement(commit)),
    );
  }
  elements.drawerIssues.replaceChildren();
  if (detail.issues.length === 0) {
    elements.drawerIssues.append(drawerEmptyState("No issues linked to this task."));
  } else {
    detail.issues.forEach((issue) =>
      elements.drawerIssues.append(drawerIssueElement(issue)),
    );
  }
}

async function loadTaskDetail(task) {
  const request = ++detailRequest;
  const ticket = workspaceController.capture();
  try {
    const detail = await api().GetTaskDetailV2(ticket.generation, Number(task.id));
    if (
      request !== detailRequest ||
      !detailTask ||
      Number(detailTask.id) !== Number(task.id) ||
      !workspaceController.accepts(ticket, Number(detail.generation))
    ) {
      return;
    }
    detailTask = detail.task;
    renderDrawerTask(detail.task);
    renderDrawerSections(detail);
  } catch (error) {
    if (request !== detailRequest) return;
    if (ticket.epoch !== workspaceController.capture().epoch) return;
    showError(error);
    closeTaskDetail();
  }
}

function openTaskDetail(task) {
  if (workspaceController.state.status !== "open") return;
  detailTask = task;
  drawerReturnFocus = document.activeElement;
  renderDrawerTask(task);
  renderDrawerLoading();
  elements.drawer.hidden = false;
  requestAnimationFrame(() => elements.drawerClose.focus());
  void loadTaskDetail(task);
}

function closeTaskDetail() {
  if (elements.drawer.hidden) return;
  hideApplicationOverlay(elements.drawer);
  detailRequest += 1;
  const taskId = detailTask?.id;
  detailTask = null;
  const card =
    taskId &&
    document.querySelector(`.card[data-task-id="${taskId}"] .card-drag-zone`);
  (card || drawerReturnFocus)?.focus?.();
  drawerReturnFocus = null;
}

async function openAgentLaunchPicker(target, invoker = document.activeElement) {
  if (workspaceController.state.status !== "open") return;
  let association;
  try {
    association = linkedAssociationPointer(
      Number(target.planId),
      target.task ? Number(target.task.id) : undefined,
    );
  } catch (error) {
    showError(error);
    return;
  }
  closeAgentLaunchPicker(false, true);
  const sequence = ++agentLaunchSequence;
  const generation = workspaceController.state.generation;
  agentLaunchReturnFocus = invoker instanceof HTMLElement ? invoker : null;
  agentLaunchProfiles = [];
  elements.agentLaunchHeading.textContent = target.task
    ? `Launch agent for task #${target.task.id}`
    : `Launch agent for plan #${target.planId}`;
  elements.agentLaunchDetail.textContent = target.task
    ? target.task.title
    : board?.planTitle || `Plan #${target.planId}`;
  elements.agentLaunchMessage.textContent = "Discovering installed agent profiles…";
  elements.agentLaunchSelect.replaceChildren();
  elements.agentLaunchSelect.disabled = true;
  elements.agentLaunchCancel.disabled = false;
  elements.agentLaunchSubmit.disabled = true;
  elements.agentLaunchModal.hidden = false;
  requestAnimationFrame(() => elements.agentLaunchCancel.focus());

  try {
    await ensureTerminalDock(
      generation,
      workspaceState.project?.root || terminalProjectRoot,
    );
    const handle = terminalHandle;
    if (!handle) throw new Error("Terminal workspace is unavailable");
    const profiles = await handle.agentProfiles();
    if (
      sequence !== agentLaunchSequence ||
      elements.agentLaunchModal.hidden ||
      workspaceController.state.status !== "open" ||
      workspaceController.state.generation !== generation ||
      terminalHandle !== handle
    ) return;
    agentLaunchProfiles = profiles;
    agentLaunchRequest = {
      association,
      generation,
      handle,
      title: target.task
        ? `Task #${target.task.id} · agent`
        : `Plan #${target.planId} · agent`,
    };
    if (profiles.length === 0) {
      elements.agentLaunchMessage.textContent =
        "No installed agent profiles were discovered. Install a supported agent to launch it here.";
      return;
    }
    for (const profile of profiles) {
      const option = document.createElement("option");
      option.value = profile.id;
      option.textContent = profile.name;
      elements.agentLaunchSelect.append(option);
    }
    elements.agentLaunchMessage.textContent =
      "Only installed agent profiles are available; this link grants no capabilities.";
    elements.agentLaunchSelect.disabled = false;
    elements.agentLaunchSubmit.disabled = false;
    elements.agentLaunchSelect.focus();
  } catch (error) {
    if (sequence !== agentLaunchSequence || elements.agentLaunchModal.hidden) return;
    elements.agentLaunchMessage.textContent = messageFrom(error);
    showError(error);
  }
}

function closeAgentLaunchPicker(restoreFocus = true, force = false) {
  if (agentLaunchBusy && !force) return;
  agentLaunchSequence += 1;
  agentLaunchBusy = false;
  agentLaunchRequest = null;
  agentLaunchProfiles = [];
  hideApplicationOverlay(elements.agentLaunchModal);
  elements.agentLaunchSelect.disabled = true;
  elements.agentLaunchCancel.disabled = false;
  elements.agentLaunchSubmit.disabled = true;
  if (restoreFocus) agentLaunchReturnFocus?.focus?.();
  agentLaunchReturnFocus = null;
}

function terminalAssociationTargets() {
  if (!board?.planId) return [];
  const planId = Number(board.planId);
  const targets = [{
    value: `plan:${planId}`,
    label: `Plan #${planId} · ${board.planTitle || "Selected plan"}`,
    association: linkedAssociationPointer(planId),
  }];
  for (const column of board.columns || []) {
    for (const task of column.tasks || []) {
      targets.push({
        value: `task:${Number(task.id)}`,
        label: `Task #${task.id} · ${task.title}`,
        association: linkedAssociationPointer(planId, Number(task.id)),
      });
    }
  }
  return targets;
}

function openTerminalAssociationEditor(invoker = document.activeElement) {
  if (workspaceController.state.status !== "open" || !terminalHandle) return;
  const active = terminalHandle.associationState();
  if (!active || active.generation !== workspaceController.state.generation) {
    showError(new Error("A live single-pane terminal tab is required"));
    return;
  }
  closeTerminalAssociationEditor(false, true);
  const sequence = ++terminalAssociationSequence;
  const targets = terminalAssociationTargets();
  terminalAssociationReturnFocus = invoker instanceof HTMLElement ? invoker : null;
  terminalAssociationRequest = {
    active,
    generation: active.generation,
    handle: terminalHandle,
    sequence,
    targets,
  };
  elements.terminalAssociationHeading.textContent = active.pointer
    ? "Relink terminal context"
    : "Link terminal context";
  elements.terminalAssociationDetail.textContent =
    `Live session ${active.sessionId} · revision ${active.revision}`;
  elements.terminalAssociationTarget.replaceChildren();
  for (const target of targets) {
    const option = document.createElement("option");
    option.value = target.value;
    option.textContent = target.label;
    elements.terminalAssociationTarget.append(option);
  }
  const selected = targets.find((target) =>
    target.association.planId === active.pointer?.planId &&
    target.association.taskId === active.pointer?.taskId
  );
  if (selected) elements.terminalAssociationTarget.value = selected.value;
  elements.terminalAssociationMessage.textContent = targets.length === 0
    ? "Select a plan before linking this terminal. You can still detach its existing link."
    : "Linking changes context only and grants no capabilities.";
  elements.terminalAssociationTarget.disabled = targets.length === 0;
  elements.terminalAssociationCancel.disabled = false;
  elements.terminalAssociationDetach.disabled = active.pointer === undefined;
  elements.terminalAssociationSubmit.disabled = targets.length === 0;
  elements.terminalAssociationModal.hidden = false;
  requestAnimationFrame(() => {
    if (terminalAssociationSequence !== sequence) return;
    (targets.length === 0
      ? elements.terminalAssociationCancel
      : elements.terminalAssociationTarget).focus();
  });
}

function closeTerminalAssociationEditor(restoreFocus = true, force = false) {
  if (terminalAssociationBusy && !force) return;
  terminalAssociationSequence += 1;
  terminalAssociationBusy = false;
  terminalAssociationRequest = null;
  hideApplicationOverlay(elements.terminalAssociationModal);
  elements.terminalAssociationTarget.disabled = true;
  elements.terminalAssociationCancel.disabled = false;
  elements.terminalAssociationDetach.disabled = true;
  elements.terminalAssociationSubmit.disabled = true;
  if (restoreFocus) terminalAssociationReturnFocus?.focus?.();
  terminalAssociationReturnFocus = null;
}

async function submitTerminalAssociation(detach = false) {
  const request = terminalAssociationRequest;
  if (!request || terminalAssociationBusy) return;
  const selected = detach
    ? null
    : request.targets.find(
      (target) => target.value === elements.terminalAssociationTarget.value,
    );
  if (!detach && !selected) {
    showError(new Error("Select the current plan or one of its tasks"));
    return;
  }
  terminalAssociationBusy = true;
  elements.terminalAssociationTarget.disabled = true;
  elements.terminalAssociationCancel.disabled = true;
  elements.terminalAssociationDetach.disabled = true;
  elements.terminalAssociationSubmit.disabled = true;
  elements.terminalAssociationMessage.textContent = detach
    ? "Detaching terminal context…"
    : "Relinking terminal context…";
  try {
    const result = await request.handle.mutateAssociation(
      request.active,
      selected?.association,
      () =>
        terminalAssociationSequence === request.sequence &&
        !elements.terminalAssociationModal.hidden &&
        workspaceController.state.status === "open" &&
        workspaceController.state.generation === request.generation &&
        terminalHandle === request.handle,
    );
    if (
      terminalAssociationSequence !== request.sequence ||
      workspaceController.state.status !== "open" ||
      workspaceController.state.generation !== request.generation ||
      terminalHandle !== request.handle ||
      result.generation !== request.generation
    ) return;
    terminalAssociationBusy = false;
    closeTerminalAssociationEditor(true);
    setStatus(detach
      ? "Terminal context detached."
      : "Terminal context relinked.");
  } catch (error) {
    if (
      terminalAssociationSequence !== request.sequence ||
      elements.terminalAssociationModal.hidden
    ) return;
    terminalAssociationBusy = false;
    elements.terminalAssociationMessage.textContent = messageFrom(error);
    elements.terminalAssociationTarget.disabled = request.targets.length === 0;
    elements.terminalAssociationCancel.disabled = false;
    elements.terminalAssociationDetach.disabled = request.active.pointer === undefined;
    elements.terminalAssociationSubmit.disabled = request.targets.length === 0;
    showError(error);
  }
}

function terminalWritebackAssociationLabel(active) {
  if (active.pointer?.taskId) return `Task #${active.pointer.taskId}`;
  if (active.pointer?.planId) return `Plan #${active.pointer.planId}`;
  return active.pointer ? "Project" : "Detached terminal";
}

function invalidateTerminalWritebackPreview() {
  const request = terminalWritebackRequest;
  if (!request || terminalWritebackBusy) return;
  request.preview = null;
  request.requestID = null;
  elements.terminalWritebackPreview.hidden = true;
  elements.terminalWritebackSummaryWarning.hidden = true;
  elements.terminalWritebackSummaryConfirm.checked = false;
  elements.terminalWritebackSave.disabled = true;
  const policy = terminalWritebackContentPolicy(elements.terminalWritebackContent.value);
  elements.terminalWritebackMessage.textContent = policy.message;
}

function openTerminalWriteback(invoker = document.activeElement) {
  if (workspaceController.state.status !== "open" || !terminalHandle) return;
  const active = terminalHandle.associationState();
  if (!active?.pointer || active.generation !== workspaceController.state.generation) {
    showError(new Error("A live linked terminal tab is required for write-back"));
    return;
  }
  closeTerminalWriteback(false, true);
  const sequence = ++terminalWritebackSequence;
  terminalWritebackReturnFocus = invoker instanceof HTMLElement ? invoker : null;
  terminalWritebackRequest = {
    active,
    generation: active.generation,
    handle: terminalHandle,
    sequence,
    preview: null,
    requestID: null,
  };
  elements.terminalWritebackTarget.textContent =
    `${terminalWritebackAssociationLabel(active)} · live revision ${active.revision}. ` +
    "The backend will derive and revalidate this destination.";
  elements.terminalWritebackKind.value = "decision";
  elements.terminalWritebackContent.value = "";
  elements.terminalWritebackContent.disabled = false;
  elements.terminalWritebackKind.disabled = false;
  elements.terminalWritebackCancel.disabled = false;
  elements.terminalWritebackPreviewButton.disabled = false;
  elements.terminalWritebackPreview.hidden = true;
  elements.terminalWritebackSummaryWarning.hidden = true;
  elements.terminalWritebackSummaryConfirm.checked = false;
  elements.terminalWritebackSave.disabled = true;
  elements.terminalWritebackMessage.textContent =
    "Enter memory, then preview its authoritative destination.";
  elements.terminalWritebackModal.hidden = false;
  requestAnimationFrame(() => {
    if (terminalWritebackSequence === sequence) {
      elements.terminalWritebackKind.focus();
    }
  });
}

function closeTerminalWriteback(restoreFocus = true, force = false) {
  if (terminalWritebackBusy && !force) return;
  terminalWritebackSequence += 1;
  terminalWritebackBusy = false;
  terminalWritebackRequest = null;
  hideApplicationOverlay(elements.terminalWritebackModal);
  elements.terminalWritebackContent.value = "";
  elements.terminalWritebackContent.disabled = false;
  elements.terminalWritebackKind.disabled = false;
  elements.terminalWritebackCancel.disabled = false;
  elements.terminalWritebackPreviewButton.disabled = false;
  elements.terminalWritebackSave.disabled = true;
  elements.terminalWritebackPreview.hidden = true;
  elements.terminalWritebackSummaryWarning.hidden = true;
  elements.terminalWritebackSummaryConfirm.checked = false;
  if (restoreFocus) terminalWritebackReturnFocus?.focus?.();
  terminalWritebackReturnFocus = null;
}

function terminalWritebackRequestIsCurrent(request) {
  return terminalWritebackSequence === request.sequence &&
    !elements.terminalWritebackModal.hidden &&
    workspaceController.state.status === "open" &&
    workspaceController.state.generation === request.generation &&
    terminalHandle === request.handle;
}

async function previewTerminalWriteback() {
  const request = terminalWritebackRequest;
  if (!request || terminalWritebackBusy) return;
  const kind = elements.terminalWritebackKind.value;
  const policy = terminalWritebackContentPolicy(elements.terminalWritebackContent.value);
  if (!policy.valid) {
    elements.terminalWritebackMessage.textContent = policy.message;
    return;
  }
  terminalWritebackBusy = true;
  elements.terminalWritebackKind.disabled = true;
  elements.terminalWritebackContent.disabled = true;
  elements.terminalWritebackCancel.disabled = true;
  elements.terminalWritebackPreviewButton.disabled = true;
  elements.terminalWritebackSave.disabled = true;
  elements.terminalWritebackMessage.textContent = "Validating write-back preview…";
  try {
    const preview = await request.handle.previewWriteback(
      request.active,
      kind,
      policy.normalized,
      () => terminalWritebackRequestIsCurrent(request),
    );
    if (!terminalWritebackRequestIsCurrent(request)) return;
    terminalWritebackBusy = false;
    request.preview = preview;
    request.requestID = stableTerminalWritebackRequestID(
      request.requestID,
      () => `writeback-${crypto.randomUUID()}`,
    );
    elements.terminalWritebackContent.value = preview.content;
    elements.terminalWritebackPreviewTarget.textContent =
      `Destination: ${preview.destination} · associated with ${preview.associationTarget}`;
    elements.terminalWritebackPreviewContent.textContent = preview.content;
    elements.terminalWritebackPreview.hidden = false;
    elements.terminalWritebackSummaryWarning.hidden = !preview.replacesSummary;
    elements.terminalWritebackSummaryConfirm.checked = false;
    elements.terminalWritebackMessage.textContent =
      `${preview.contentBytes} bytes validated. Review before writing.`;
    elements.terminalWritebackKind.disabled = false;
    elements.terminalWritebackContent.disabled = false;
    elements.terminalWritebackCancel.disabled = false;
    elements.terminalWritebackPreviewButton.disabled = false;
    elements.terminalWritebackSave.disabled = preview.replacesSummary;
    (preview.replacesSummary
      ? elements.terminalWritebackSummaryConfirm
      : elements.terminalWritebackSave).focus();
  } catch (error) {
    if (!terminalWritebackRequestIsCurrent(request)) return;
    terminalWritebackBusy = false;
    elements.terminalWritebackMessage.textContent = messageFrom(error);
    elements.terminalWritebackKind.disabled = false;
    elements.terminalWritebackContent.disabled = false;
    elements.terminalWritebackCancel.disabled = false;
    elements.terminalWritebackPreviewButton.disabled = false;
    showError(error);
  }
}

async function commitTerminalWriteback() {
  const request = terminalWritebackRequest;
  if (!request || terminalWritebackBusy || !request.preview || !request.requestID) return;
  const policy = terminalWritebackContentPolicy(elements.terminalWritebackContent.value);
  if (!policy.valid || policy.normalized !== request.preview.content ||
    elements.terminalWritebackKind.value !== request.preview.kind) {
    invalidateTerminalWritebackPreview();
    return;
  }
  const confirmSummary = request.preview.replacesSummary &&
    elements.terminalWritebackSummaryConfirm.checked;
  if (request.preview.replacesSummary && !confirmSummary) {
    elements.terminalWritebackMessage.textContent =
      "Confirm replacement of the entire project rolling summary.";
    return;
  }
  terminalWritebackBusy = true;
  elements.terminalWritebackKind.disabled = true;
  elements.terminalWritebackContent.disabled = true;
  elements.terminalWritebackCancel.disabled = true;
  elements.terminalWritebackPreviewButton.disabled = true;
  elements.terminalWritebackSave.disabled = true;
  elements.terminalWritebackMessage.textContent = "Writing explicit project memory…";
  try {
    const result = await request.handle.writeback(
      request.active,
      request.requestID,
      request.preview.kind,
      request.preview.content,
      confirmSummary,
      () => terminalWritebackRequestIsCurrent(request),
    );
    if (!terminalWritebackRequestIsCurrent(request)) return;
    terminalWritebackBusy = false;
    closeTerminalWriteback(true);
    setStatus(`${result.kind} written to ${result.destination}.`);
    await loadSnapshot(board?.planId || 0);
  } catch (error) {
    if (!terminalWritebackRequestIsCurrent(request)) return;
    terminalWritebackBusy = false;
    elements.terminalWritebackMessage.textContent =
      `${messageFrom(error)} Retry keeps the same request identity.`;
    elements.terminalWritebackKind.disabled = false;
    elements.terminalWritebackContent.disabled = false;
    elements.terminalWritebackCancel.disabled = false;
    elements.terminalWritebackPreviewButton.disabled = false;
    elements.terminalWritebackSave.disabled =
      request.preview.replacesSummary && !elements.terminalWritebackSummaryConfirm.checked;
    showError(error);
  }
}

async function submitAgentLaunch() {
  const request = agentLaunchRequest;
  if (!request || elements.agentLaunchSubmit.disabled) return;
  let profile;
  try {
    profile = selectedInstalledAgentProfile(
      agentLaunchProfiles,
      elements.agentLaunchSelect.value,
    );
  } catch (error) {
    showError(error);
    return;
  }
  const sequence = agentLaunchSequence;
  agentLaunchBusy = true;
  elements.agentLaunchSelect.disabled = true;
  elements.agentLaunchCancel.disabled = true;
  elements.agentLaunchSubmit.disabled = true;
  elements.agentLaunchMessage.textContent = `Launching ${profile.name}…`;
  try {
    await request.handle.launchLinked({
      profileId: profile.id,
      title: request.title.replace("agent", profile.name),
      association: request.association,
    });
    if (
      sequence !== agentLaunchSequence ||
      workspaceController.state.status !== "open" ||
      workspaceController.state.generation !== request.generation ||
      terminalHandle !== request.handle
    ) return;
    agentLaunchBusy = false;
    closeAgentLaunchPicker(false);
    if (!elements.drawer.hidden) closeTaskDetail();
    setStatus(`${profile.name} launched in a linked terminal tab.`);
    await loadSnapshot(board?.planId || 0, false);
  } catch (error) {
    if (sequence !== agentLaunchSequence || elements.agentLaunchModal.hidden) return;
    agentLaunchBusy = false;
    elements.agentLaunchMessage.textContent = messageFrom(error);
    elements.agentLaunchSelect.disabled = agentLaunchProfiles.length === 0;
    elements.agentLaunchCancel.disabled = false;
    elements.agentLaunchSubmit.disabled = agentLaunchProfiles.length === 0;
    showError(error);
  }
}

function showConfirmation(copy, returnFocus = document.activeElement) {
  confirmReturnFocus = returnFocus;
  elements.confirmEyebrow.textContent = copy.eyebrow;
  elements.confirmHeading.textContent = copy.heading;
  elements.confirmDetail.textContent = copy.detail;
  elements.confirmCancel.textContent = copy.cancel;
  elements.confirmSubmit.textContent = copy.submit;
  elements.confirmModal.hidden = false;
  requestAnimationFrame(() => elements.confirmCancel.focus());
  return new Promise((resolve) => {
    confirmResolve = resolve;
  });
}

function showWorkspaceConfirmation(action, resources, returnFocus = document.activeElement) {
  const copy = confirmationCopy(
    action,
    resources.terminals,
    resources.agentRuns,
    resources.pendingAdmissions || 0,
  );
  return showConfirmation({
    eyebrow: "Active project resources",
    heading: copy.heading,
    detail: copy.detail,
    cancel: "Stay here",
    submit: copy.submit,
  }, returnFocus);
}

function showRecentRelocationConfirmation(entry, resolution) {
  return showConfirmation({
    eyebrow: "Recent project location",
    heading: "Open a different project?",
    detail:
      `“${entry.name}” at ${entry.canonicalPath} now resolves to “${resolution.name}” at ${resolution.canonicalRoot}. Update this recent entry only after the different project opens?`,
    cancel: "Keep Current Entry",
    submit: "Update and Open",
  }, null);
}

function showForgetRecentProjectConfirmation(entry) {
  return showConfirmation({
    eyebrow: "Recent project",
    heading: "Forget this recent project?",
    detail:
      `Remove “${entry.name}” at ${entry.canonicalPath} from Recent projects only. Project files will not be changed.`,
    cancel: "Keep Recent Entry",
    submit: "Remove Recent Entry",
  }, null);
}

function setRecentProjectsState(event) {
  recentProjectsState = reduceRecentProjects(recentProjectsState, event);
  renderRecentProjects();
}

function recentProjectActionElement(focusKey) {
  if (focusKey === "recent-project-heading") return elements.recentHeading;
  return [...elements.recents.querySelectorAll("[data-recent-focus-key]")]
    .find((element) => element.dataset.recentFocusKey === focusKey) ||
    elements.recentHeading;
}

function restoreRecentProjectFocus(focusKey, operationId = recentOperationSequence) {
  requestAnimationFrame(() => {
    if (
      operationId !== recentOperationSequence ||
      firstRunState.phase !== "idle" ||
      workspaceController.state.status === "open" ||
      workspaceController.state.status === "loading"
    ) return;
    recentProjectActionElement(focusKey)?.focus();
  });
}

function finishWorkspaceConfirmation(confirmed) {
  if (!confirmResolve) return;
  const resolve = confirmResolve;
  confirmResolve = null;
  hideApplicationOverlay(elements.confirmModal);
  confirmReturnFocus?.focus();
  confirmReturnFocus = null;
  resolve(confirmed);
}

function recentProjectStateLabel(availability) {
  if (availability === "missing") return "Folder not found";
  if (availability === "permission-required") return "Permission required";
  if (availability === "changed") return "Project changed";
  return "";
}

function recentProjectPrimaryLabel(availability) {
  if (availability === "available") return "Open";
  if (availability === "permission-required") return "Try Again";
  return "Locate…";
}

function recentProjectOperationActive() {
  return !["idle", "loading", "error"].includes(recentProjectsState.phase);
}

function updateAboutUpdatesAvailability() {
  elements.appVersion.disabled = firstRunState.phase !== "idle" ||
    firstPlanState.phase !== "idle" ||
    recentProjectOperationActive();
  elements.settingsOpen.disabled = elements.appVersion.disabled;
}

function recentProjectActionButton(entry, action, label, describedBy, handler) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "button-secondary";
  button.textContent = label;
  button.dataset.recentFocusKey = recentProjectFocusKey(entry.entryId, action);
  button.setAttribute(
    "aria-label",
    action === "forget"
      ? `Forget ${entry.name} from Recent projects only`
      : `${label.replace("…", "")} ${entry.name}`,
  );
  button.setAttribute("aria-describedby", describedBy);
  button.disabled = recentProjectsState.listLoading ||
    !["idle", "error"].includes(recentProjectsState.phase);
  button.addEventListener("click", handler);
  return button;
}

function renderRecentProjects() {
  elements.recents.replaceChildren();
  const projects = recentProjectsState.projects;
  const operationActive = recentProjectOperationActive();
  updateAboutUpdatesAvailability();
  elements.stateInitialize.disabled = operationActive;
  elements.stateOpen.disabled = operationActive;
  elements.recents.setAttribute(
    "aria-busy",
    String(recentProjectsState.listLoading || operationActive),
  );
  // The opted-in last project that did not auto-open is pointed at rather than
  // opened: the row says so, the live region says so, and nothing takes focus.
  const preselectedEntryId = preselectedRecentProject(projects, preferences.startup);
  const preselected = projects.find(
    (project) => project.entryId === preselectedEntryId,
  );
  elements.recentStatus.textContent = recentProjectsState.announcement ||
    (preselected
      ? `“${preselected.name}” is preselected as the last project p-track recorded. Confirm it to continue.`
      : "");
  elements.recentError.textContent = recentProjectsState.message ||
    recentProjectsState.listError;
  const active = projects.find(
    (project) => project.entryId === recentProjectsState.activeEntryId,
  );
  if (active && operationActive) {
    elements.recentStatus.textContent = {
      picking: `Choose the folder for “${active.name}”.`,
      resolving: `Checking “${active.name}”…`,
      "confirming-relocation": `Waiting for confirmation for “${active.name}”.`,
      opening: `Opening “${active.name}”…`,
      "confirming-forget": `Waiting for confirmation to remove “${active.name}” from Recent projects.`,
      forgetting: `Removing “${active.name}” from Recent projects…`,
    }[recentProjectsState.phase] || "";
  }
  if (projects.length === 0) {
    const message = recentProjectsState.listLoading
      ? "Loading recent projects…"
      : "No recent projects yet.";
    const empty = emptyMemory(message);
    empty.setAttribute("role", "listitem");
    elements.recents.append(empty);
    return;
  }
  projects.forEach((project, index) => {
    const item = document.createElement("article");
    item.className = "recent-project";
    item.setAttribute("role", "listitem");
    item.setAttribute(
      "aria-busy",
      String(operationActive && project.entryId === recentProjectsState.activeEntryId),
    );
    const content = document.createElement("div");
    const name = document.createElement("p");
    name.className = "recent-project-name";
    name.id = `recent-project-name-${index}`;
    name.textContent = project.name;
    item.setAttribute("aria-labelledby", name.id);
    const path = document.createElement("p");
    path.className = "recent-project-path";
    path.id = `recent-project-path-${index}`;
    path.append(`${project.canonicalPath} · `);
    const lastOpened = document.createElement("time");
    lastOpened.dateTime = project.lastOpenedAt;
    lastOpened.title = new Date(project.lastOpenedAt).toLocaleString();
    lastOpened.textContent = relativeTime(project.lastOpenedAt);
    path.append(lastOpened);
    content.append(name, path);
    const descriptionIDs = [path.id];
    const stateLabel = recentProjectStateLabel(project.availability);
    if (stateLabel) {
      const state = document.createElement("p");
      state.className = "recent-project-state";
      state.id = `recent-project-state-${index}`;
      state.textContent = stateLabel;
      content.append(state);
      descriptionIDs.push(state.id);
    }
    if (project.entryId === preselectedEntryId) {
      item.setAttribute("aria-current", "true");
      const preselect = document.createElement("p");
      preselect.className = "recent-project-preselect";
      preselect.id = `recent-project-preselect-${index}`;
      preselect.textContent = "Preselected — last project p-track recorded";
      content.append(preselect);
      descriptionIDs.push(preselect.id);
    }
    const actions = document.createElement("div");
    actions.className = "recent-project-actions";
    const primaryAction = recentProjectPrimaryAction(project.availability);
    actions.append(recentProjectActionButton(
      project,
      primaryAction,
      recentProjectPrimaryLabel(project.availability),
      descriptionIDs.join(" "),
      (event) => {
        if (primaryAction === "open") {
          void openAvailableRecentProject(project, event.currentTarget);
        } else if (primaryAction === "retry") {
          void retryRecentProject(project, event.currentTarget);
        } else {
          void locateRecentProject(project, event.currentTarget);
        }
      },
    ));
    if (project.availability !== "available") {
      actions.append(recentProjectActionButton(
        project,
        "forget",
        "Forget",
        descriptionIDs.join(" "),
        (event) => void forgetRecentProject(project, event.currentTarget),
      ));
    }
    item.append(content, actions);
    elements.recents.append(item);
  });
}

function beginRecentProjectOperation(entry, intent) {
  if (
    recentProjectsState.listLoading ||
    !["idle", "error"].includes(recentProjectsState.phase)
  ) return null;
  const operationId = ++recentOperationSequence;
  const workspace = workspaceController.capture();
  setRecentProjectsState({ type: "begin", operationId, entry, intent });
  return {
    operationId,
    entryId: entry.entryId,
    base: entry.base,
    epoch: workspace.epoch,
    generation: workspace.generation,
    intent,
  };
}

function recentProjectOperationMatches(ticket) {
  return recentOperationSequence === ticket.operationId &&
    recentProjectsState.operationId === ticket.operationId &&
    recentProjectsState.activeEntryId === ticket.entryId &&
    recentProjectsState.activeBase === ticket.base;
}

function recentProjectOperationIsCurrent(ticket) {
  const workspace = workspaceController.capture();
  return recentProjectOperationMatches(ticket) &&
    workspace.epoch === ticket.epoch &&
    workspace.generation === ticket.generation &&
    workspaceController.state.status !== "open" &&
    workspaceController.state.status !== "loading";
}

function settleRecentProjectOperation(ticket, announcement, focusKey) {
  if (!recentProjectOperationIsCurrent(ticket)) return;
  setRecentProjectsState({ type: "settled", announcement });
  restoreRecentProjectFocus(focusKey, ticket.operationId);
}

function failRecentProjectOperation(ticket, error, focusKey) {
  if (!recentProjectOperationIsCurrent(ticket)) return;
  const message =
    `p-track could not confirm the recent-project action: ${messageFrom(error)}`;
  setRecentProjectsState({ type: "settled" });
  void loadRecentProjects({
    focusKey,
    errorMessage: message,
  });
}

async function refreshRecentProjectsAfterOpen() {
  const ticket = workspaceController.capture();
  try {
    const projects = parseRecentProjects(await api().GetRecentProjectsV1());
    if (!workspaceController.accepts(ticket, ticket.generation)) return null;
    recentProjectsState = reduceRecentProjects(recentProjectsState, {
      type: "loadStarted",
    });
    recentProjectsState = reduceRecentProjects(recentProjectsState, {
      type: "loaded",
      projects,
    });
    return projects;
  } catch {
    return null;
  }
}

async function reconcileRecentProjectOpenFailure(ticket, entry, resolution, error) {
  try {
    const state = await api().GetWorkspaceState();
    const exactOpen = state?.status === "open" &&
      state.project?.root === resolution.canonicalRoot;
    if (exactOpen) {
      recentProjectsState = reduceRecentProjects(recentProjectsState, { type: "settled" });
      workspaceController.publish({
        status: state.status,
        generation: Number(state.generation || 0),
      });
      renderWorkspaceState(state, true);
      await loadSnapshot(board?.planId || 0);
      if (resolution.canonicalRoot !== entry.canonicalPath) {
        await refreshRecentProjectsAfterOpen();
        showError(new Error(RECENT_RELOCATION_UNCONFIRMED));
      }
      return;
    }
    workspaceController.publish({
      status: state.status,
      generation: Number(state.generation || 0),
    });
    renderWorkspaceState(state, false);
    if (state.status === "open") {
      showError(error);
      return;
    }
    if (!recentProjectOperationMatches(ticket)) return;
    setRecentProjectsState({
      type: "failed",
      message:
        `p-track could not confirm that “${entry.name}” opened. The recent entry was not replayed: ${messageFrom(error)}`,
    });
    restoreRecentProjectFocus(
      recentProjectFocusKey(entry.entryId, ticket.intent),
      ticket.operationId,
    );
  } catch (stateError) {
    workspaceController.publish({ status: "error", generation: 0 });
    renderWorkspaceState(
      { status: "error", generation: 0, error: messageFrom(stateError) },
      true,
    );
  }
}

async function openResolvedRecentProject(ticket, entry, resolution) {
  if (!recentProjectOperationIsCurrent(ticket)) return;
  setRecentProjectsState({ type: "opening" });
  let transition = beginWorkspaceTransition();
  try {
    let result = parseRecentProjectOpenResult(
      await api().OpenRecentProjectV1(
        entry.entryId,
        entry.base,
        resolution.canonicalRoot,
        resolution.confirmationToken,
        "",
      ),
      entry,
    );
    if (!recentProjectOperationMatches(ticket)) return;
    if (result.open.requiresConfirmation) {
      if (!publishBackendState(result.open.state, transition, false, true)) return;
      const confirmed = await showWorkspaceConfirmation(
        "switch",
        result.open.activeResources,
        null,
      );
      if (!recentProjectOperationMatches(ticket)) return;
      if (!confirmed) {
        await api().CancelWorkspaceChange(result.open.confirmationToken);
        setRecentProjectsState({
          type: "settled",
          announcement: "Project unchanged.",
        });
        renderWorkspaceState(result.open.state, false);
        restoreRecentProjectFocus(
          recentProjectFocusKey(entry.entryId, ticket.intent),
          ticket.operationId,
        );
        return;
      }
      transition = beginWorkspaceTransition();
      result = parseRecentProjectOpenResult(
        await api().OpenRecentProjectV1(
          entry.entryId,
          entry.base,
          resolution.canonicalRoot,
          resolution.confirmationToken,
          result.open.confirmationToken,
        ),
        entry,
      );
      if (!recentProjectOperationMatches(ticket)) return;
    }
    recentProjectsState = reduceRecentProjects(recentProjectsState, { type: "settled" });
    if (!publishBackendState(result.open.state, transition, true)) return;
    const warnings = [];
    if (result.open.warning) warnings.push(result.open.warning);
    if (result.registryStatus === "stale") {
      warnings.push(
        "Project opened, but its recent entry changed elsewhere and was not updated.",
      );
      void refreshRecentProjectsAfterOpen();
    }
    if (warnings.length > 0) showError(new Error(warnings.join(" ")));
  } catch (error) {
    await reconcileRecentProjectOpenFailure(ticket, entry, resolution, error);
  }
}

async function openAvailableRecentProject(entry) {
  const ticket = beginRecentProjectOperation(entry, "open");
  if (!ticket) return;
  await openResolvedRecentProject(ticket, entry, {
    entryId: entry.entryId,
    base: entry.base,
    canonicalRoot: entry.canonicalPath,
    name: entry.name,
    resolution: "ready",
    confirmationToken: "",
  });
}

async function retryRecentProject(entry) {
  const ticket = beginRecentProjectOperation(entry, "retry");
  if (!ticket) return;
  const focusKey = recentProjectFocusKey(entry.entryId, "retry");
  try {
    const resolution = parseRecentProjectResolution(
      await api().ResolveRecentProjectV1(entry.entryId, entry.base, entry.canonicalPath),
      entry,
    );
    if (!recentProjectOperationIsCurrent(ticket)) return;
    if (resolution.resolution === "confirmation-required") {
      setRecentProjectsState({ type: "settled" });
      void loadRecentProjects({
        focusKey,
        errorMessage:
          "This path now contains a different project. Review it with Locate…; Try Again never changes a recent entry.",
      });
      return;
    }
    await openResolvedRecentProject(ticket, entry, resolution);
  } catch (error) {
    failRecentProjectOperation(ticket, error, focusKey);
  }
}

async function locateRecentProject(entry) {
  const ticket = beginRecentProjectOperation(entry, "locate");
  if (!ticket) return;
  const focusKey = recentProjectFocusKey(entry.entryId, "locate");
  try {
    const candidatePath = await chooseProjectDirectory("locate-recent-project");
    if (!recentProjectOperationIsCurrent(ticket)) return;
    if (!candidatePath) {
      settleRecentProjectOperation(
        ticket,
        "Folder selection canceled. Recent entry unchanged.",
        focusKey,
      );
      return;
    }
    setRecentProjectsState({ type: "resolving" });
    const resolution = parseRecentProjectResolution(
      await api().ResolveRecentProjectV1(entry.entryId, entry.base, candidatePath),
      entry,
    );
    if (!recentProjectOperationIsCurrent(ticket)) return;
    if (resolution.resolution === "confirmation-required") {
      setRecentProjectsState({ type: "confirmRelocation" });
      const confirmed = await showRecentRelocationConfirmation(entry, resolution);
      if (!recentProjectOperationIsCurrent(ticket)) return;
      if (!confirmed) {
        settleRecentProjectOperation(ticket, "Recent entry unchanged.", focusKey);
        return;
      }
    }
    await openResolvedRecentProject(ticket, entry, resolution);
  } catch (error) {
    failRecentProjectOperation(ticket, error, focusKey);
  }
}

async function forgetRecentProject(entry) {
  const ticket = beginRecentProjectOperation(entry, "forget");
  if (!ticket) return;
  const focusKey = recentProjectFocusKey(entry.entryId, "forget");
  const confirmed = await showForgetRecentProjectConfirmation(entry);
  if (!recentProjectOperationIsCurrent(ticket)) return;
  if (!confirmed) {
    settleRecentProjectOperation(ticket, "Recent entry unchanged.", focusKey);
    return;
  }
  setRecentProjectsState({ type: "forgetting" });
  try {
    parseForgetRecentProjectResult(
      await api().ForgetRecentProjectV1(entry.entryId, entry.base),
      entry,
    );
    if (!recentProjectOperationIsCurrent(ticket)) return;
    const nextFocus = focusAfterForgottenProject(
      recentProjectsState.projects,
      entry.entryId,
    );
    setRecentProjectsState({ type: "settled" });
    await loadRecentProjects({
      focusKey: nextFocus,
      announcement:
        `Removed “${entry.name}” from Recent projects. Project files were not changed.`,
    });
  } catch (error) {
    failRecentProjectOperation(ticket, error, focusKey);
  }
}

function setFirstRunState(event, focus = false) {
  firstRunState = reduceFirstRun(firstRunState, event);
  renderFirstRunFlow(focus);
}

function setSetupContent({ progress, eyebrow, heading, detail, status = "", error = "" }) {
  elements.setupProgress.textContent = progress;
  elements.setupEyebrow.textContent = eyebrow;
  elements.setupHeading.textContent = heading;
  elements.setupDetail.textContent = detail;
  elements.setupStatus.textContent = status;
  elements.setupError.textContent = error;
}

function focusFirstRunTarget() {
  const target = document.getElementById(firstRunFocusTarget(firstRunState));
  requestAnimationFrame(() => target?.focus());
}

function setFirstRunSectionVisible(element, visible) {
  element.hidden = !visible;
  element.inert = !visible;
}

function guideActionLabel(action) {
  if (action === "create") return "Create";
  if (action === "update") return "Update";
  return "No change";
}

function renderProjectGuideFiles(files) {
  elements.setupGuideFiles.replaceChildren();
  files.forEach((file) => {
    const item = document.createElement("article");
    item.className = "setup-guide-file";
    const header = document.createElement("div");
    header.className = "setup-guide-file-header";
    const path = document.createElement("p");
    path.className = "setup-guide-path";
    path.textContent = file.path;
    const counts = document.createElement("p");
    counts.className = "setup-guide-counts";
    counts.textContent = file.action === "no-change"
      ? "No change"
      : `${guideActionLabel(file.action)} · +${file.additions} −${file.deletions}`;
    header.append(path, counts);
    item.append(header);
    if (file.diff) {
      const diff = document.createElement("pre");
      diff.className = "setup-guide-diff";
      diff.tabIndex = 0;
      diff.setAttribute("aria-label", `${file.path} bounded diff`);
      const code = document.createElement("code");
      code.textContent = file.diff;
      diff.append(code);
      item.append(diff);
    }
    elements.setupGuideFiles.append(item);
  });
}

function appendTextItems(target, items) {
  target.replaceChildren();
  items.forEach((text) => {
    const item = document.createElement("li");
    item.textContent = text;
    target.append(item);
  });
}

function renderFirstRunFlow(focus = false) {
  const idle = firstRunState.phase === "idle";
  updateAboutUpdatesAvailability();
  elements.openProject.disabled = !idle;
  elements.switchProject.disabled = !idle;
  elements.closeProject.disabled = !idle;
  setFirstRunSectionVisible(elements.welcomePanel, idle);
  setFirstRunSectionVisible(elements.setupPanel, !idle);
  // Keep the focused heading and dedicated status/alert regions outside the
  // busy subtree so progress stays announceable while actions are locked.
  elements.stateCard.removeAttribute("aria-busy");
  setAriaBoolean(
    elements.setupOperation,
    "aria-busy",
    firstRunState.phase === "validating" ||
      firstRunState.phase === "guide-previewing" ||
      firstRunState.phase === "committing" ||
      firstRunState.phase === "reconciling",
  );
  elements.setupTargetSummary.hidden = true;
  setFirstRunSectionVisible(elements.setupGoalForm, false);
  setFirstRunSectionVisible(elements.setupGuide, false);
  setFirstRunSectionVisible(elements.setupReview, false);
  setFirstRunSectionVisible(elements.setupExistingActions, false);
  setFirstRunSectionVisible(elements.setupNewTargetActions, false);
  setFirstRunSectionVisible(elements.setupRecoveryActions, false);
  setFirstRunSectionVisible(elements.setupUncertainActions, false);
  elements.setupRetry.hidden = true;
  elements.setupResume.hidden = true;
  elements.setupOpenRecovery.hidden = true;
  elements.setupRecoveryHelp.hidden = true;
  elements.setupRecoveryChoose.hidden = false;
  elements.setupReturnWelcome.hidden = false;
  elements.setupReturnWelcome.textContent = "Return to Welcome";
  elements.setupGoalError.textContent = "";
  elements.setupGoal.removeAttribute("aria-invalid");

  if (idle) {
    if (focus) focusFirstRunTarget();
    return;
  }

  const showTarget = () => {
    elements.setupTargetSummary.hidden = false;
    elements.setupTarget.textContent = firstRunState.canonicalRoot;
  };
  const showCommittedGuideRecoveryActions = () => {
    if (!firstRunState.resumeLocked || firstRunState.checkpoint === "none") return;
    setFirstRunSectionVisible(elements.setupRecoveryActions, true);
    elements.setupOpenRecovery.hidden = !canOpenPreservedFirstRunProject(
      firstRunState,
    );
    elements.setupRecoveryHelp.hidden = false;
    elements.setupRecoveryChoose.hidden = true;
    elements.setupReturnWelcome.hidden = true;
  };
  switch (firstRunState.phase) {
    case "picking":
      setSetupContent({
        progress: "Step 1 of 4",
        eyebrow: firstRunState.intent === "initialize" ? "Initialize project" : "Open project",
        heading: "Choose a project folder",
        detail: "Use the native folder picker to continue.",
      });
      break;
    case "validating":
      setSetupContent({
        progress: "Step 1 of 4",
        eyebrow: firstRunState.intent === "initialize" ? "Initialize project" : "Open project",
        heading: "Checking this folder…",
        detail: "p-track is resolving the canonical folder and checking project state without writing files.",
        status: "Validating the selected folder without making changes.",
      });
      break;
    case "existing":
      showTarget();
      setFirstRunSectionVisible(elements.setupExistingActions, true);
      setSetupContent({
        progress: "Step 1 of 4",
        eyebrow: "Existing project found",
        heading: "This folder already has a p-track project",
        detail: "Open the existing project, or choose another folder to initialize.",
      });
      break;
    case "target-new":
      showTarget();
      setFirstRunSectionVisible(elements.setupNewTargetActions, true);
      setSetupContent({
        progress: "Step 1 of 4",
        eyebrow: "Initialize project",
        heading: "Continue with this folder?",
        detail:
          "Your north-star goal is preserved. Continue to edit it, choose another folder, or explicitly cancel setup.",
      });
      break;
    case "goal":
      showTarget();
      setFirstRunSectionVisible(elements.setupGoalForm, true);
      elements.setupGoal.value = firstRunState.goal;
      elements.setupGoalError.textContent = firstRunState.goalError;
      setAriaBoolean(
        elements.setupGoal,
        "aria-invalid",
        Boolean(firstRunState.goalError),
      );
      setSetupContent({
        progress: "Step 2 of 4",
        eyebrow: "Project direction",
        heading: "Set the north-star goal",
        detail: "Describe the durable outcome this project is working toward.",
      });
      break;
    case "guide": {
      showTarget();
      setFirstRunSectionVisible(elements.setupGuide, true);
      const hasPreview = firstRunState.guideAvailable === true &&
        firstRunState.guideFiles.length > 0;
      setFirstRunSectionVisible(elements.setupGuidePreview, hasPreview);
      setFirstRunSectionVisible(elements.setupGuideInstallActions, hasPreview);
      setFirstRunSectionVisible(elements.setupGuideDefaultActions, !hasPreview);
      setFirstRunSectionVisible(elements.setupGuideStaleActions, false);
      elements.setupGuideDefaultChoice.hidden = !firstRunState.guideSkipAllowed;
      elements.setupGuideSkip.hidden = !firstRunState.guideSkipAllowed;
      elements.setupGuidePreviewSkip.hidden = !firstRunState.guideSkipAllowed;
      elements.setupGuidePreviewButton.hidden = firstRunState.guideAvailable === false;
      elements.setupGuidePreviewBack.hidden = firstRunState.resumeLocked;
      elements.setupGuidePreviewCancel.hidden = firstRunState.resumeLocked;
      elements.setupGuideBack.hidden = firstRunState.resumeLocked;
      elements.setupGuideCancel.hidden = firstRunState.resumeLocked;
      showCommittedGuideRecoveryActions();
      if (hasPreview) renderProjectGuideFiles(firstRunState.guideFiles);
      setSetupContent({
        progress: "Step 3 of 4",
        eyebrow: "Project guidance",
        heading: hasPreview ? "Review exact guide changes" : "Choose project guidance",
        detail: hasPreview
          ? "The diff is read-only. Install only if every target and line is expected."
          : firstRunState.resumeNoWrite
          ? "No project files were written. You can try again safely."
          : !firstRunState.guideSkipAllowed
          ? firstRunState.guidePartiallyApplied
            ? projectGuideRecoveryCopy("partially-applied").detail
            : "This initialization operation already has durable progress. Preview the current guide files to resume safely."
          : "Skip Guide is selected by default. Previewing does not write files.",
        status: firstRunState.guideAvailable === false
          ? PROJECT_GUIDANCE_UNAVAILABLE
          : "",
        error: firstRunState.guideAvailable === null ? firstRunState.message : "",
      });
      break;
    }
    case "guide-previewing":
      showTarget();
      setSetupContent({
        progress: "Step 3 of 4",
        eyebrow: "Project guidance",
        heading: "Preparing the guide preview…",
        detail: "p-track is reading only AGENTS.md and CLAUDE.md and will not write files.",
        status: "Checking current guide files and generating a bounded diff.",
      });
      break;
    case "guide-stale":
      showTarget();
      setFirstRunSectionVisible(elements.setupGuide, true);
      setFirstRunSectionVisible(elements.setupGuidePreview, false);
      setFirstRunSectionVisible(elements.setupGuideInstallActions, false);
      setFirstRunSectionVisible(elements.setupGuideDefaultActions, false);
      setFirstRunSectionVisible(elements.setupGuideStaleActions, true);
      elements.setupGuideDefaultChoice.hidden = true;
      elements.setupGuideStaleBack.hidden = firstRunState.resumeLocked;
      elements.setupGuideStaleCancel.hidden = firstRunState.resumeLocked;
      elements.setupGuideStaleSkip.hidden = !firstRunState.guideSkipAllowed;
      showCommittedGuideRecoveryActions();
      const recoveryCopy = projectGuideRecoveryCopy(
        firstRunState.guidePartiallyApplied ? "partially-applied" : "stale",
      );
      setSetupContent({
        progress: "Guide review required",
        eyebrow: "Project guidance",
        heading: recoveryCopy.heading,
        detail: firstRunState.resumeNoWrite
          ? "No project files were written. You can try again safely."
          : firstRunState.guidePartiallyApplied
          ? recoveryCopy.detail
          : firstRunState.resumeLocked
          ? firstRunState.storageAlreadyCreated
            ? `Private project storage is already durable. Review the current guide files${firstRunState.guideSkipAllowed ? " or explicitly skip them" : ""} to finish initialization.`
            : "This initialization operation already has durable progress. Review the current guide files to resume safely."
          : "Nothing was written. Review the current guide files or explicitly skip them.",
        error: firstRunState.message,
      });
      break;
    case "review":
      showTarget();
      setFirstRunSectionVisible(elements.setupReview, true);
      showCommittedGuideRecoveryActions();
      const storagePath = firstRunStoragePath(firstRunState.canonicalRoot);
      elements.setupStorageSummary.textContent = firstRunState.storageAlreadyCreated
        ? `${storagePath} is already durable for this operation.`
        : firstRunState.resumedOperation || firstRunState.guidePostCommit
        ? `Resume this operation and create ${storagePath}.`
        : `Create ${storagePath}.`;
      elements.setupUntouchedRoot.textContent =
        `No files inside ${firstRunState.canonicalRoot} beyond this complete list will change.`;
      elements.setupReviewGoal.textContent = firstRunState.goal;
      const guideCopy = (firstRunState.resumedOperation || firstRunState.guidePostCommit) &&
          firstRunState.guideFiles.length === 0
        ? durableProjectGuideReviewCopy(firstRunState.guideChoice)
        : projectGuideReviewCopy(
          firstRunState.guideChoice,
          firstRunState.guideFiles,
        );
      elements.setupReviewGuideChoice.textContent = guideCopy.label;
      elements.setupReviewGuideDetail.textContent = guideCopy.detail;
      appendTextItems(elements.setupReviewGuideChanges, guideCopy.changes);
      appendTextItems(elements.setupCompleteChanges, [
        `${storagePath} · ${firstRunState.storageAlreadyCreated ? "already created" : "create"} private project database`,
        ...guideCopy.changes,
      ]);
      elements.setupReviewBack.hidden = firstRunState.resumeLocked;
      elements.setupReviewCancel.hidden = firstRunState.resumeLocked;
      elements.setupCommit.textContent = firstRunState.resumedOperation ||
          firstRunState.guidePostCommit
        ? firstRunState.resumeNoWrite ? "Resume Initialization" : "Finish Initialization"
        : "Initialize Project";
      setSetupContent({
        progress: "Step 4 of 4",
        eyebrow: "Review changes",
        heading: firstRunState.resumedOperation || firstRunState.guidePostCommit
          ? firstRunState.resumeNoWrite
            ? "Resume this project initialization?"
            : "Finish this project initialization?"
          : "Initialize this project?",
        detail: firstRunState.resumeNoWrite
          ? "No project files were written. You can try again safely. Confirm to resume the same operation."
          : firstRunState.resumedOperation || firstRunState.guidePostCommit
          ? firstRunState.storageAlreadyCreated
            ? "Project storage is already durable. Confirm the reviewed guide choice to resume the same operation."
            : "This operation already has durable progress. Confirm the reviewed guide choice to resume before project storage is created."
          : "Review every proposed change. Nothing is written until you confirm.",
      });
      break;
    case "committing":
      showTarget();
      setSetupContent({
        progress: "Step 4 of 4",
        eyebrow: "Initializing project",
        heading: firstRunState.resumedOperation || firstRunState.guidePostCommit
          ? "Finishing project initialization…"
          : "Creating local project state…",
        detail: firstRunState.resumedOperation || firstRunState.guidePostCommit
          ? firstRunState.storageAlreadyCreated
            ? "Private project storage is already durable. p-track is resuming the same operation without replaying completed steps."
            : "p-track is resuming the same durable operation before creating private project storage."
          : "Keep p-track open while it completes the recoverable initialization sequence.",
        status: "Initialization has started and can no longer be canceled.",
      });
      break;
    case "reconciling":
      showTarget();
      setSetupContent({
        progress: "Checking status",
        eyebrow: "Initialization status",
        heading: "Checking the durable operation…",
        detail: "p-track is reading the saved operation status without replaying initialization.",
        status: "Project navigation remains locked until the operation reaches a definitive state.",
      });
      break;
    case "uncertain":
      showTarget();
      setFirstRunSectionVisible(elements.setupUncertainActions, true);
      setSetupContent({
        progress: "Status unavailable",
        eyebrow: "Initialization status",
        heading: "Keep this operation open",
        detail: firstRunState.message || "p-track could not confirm whether initialization is still running.",
        error: firstRunState.checkpoint && firstRunState.checkpoint !== "none"
          ? `Last durable checkpoint: ${firstRunState.checkpoint}`
          : "No definitive completion state is available yet.",
      });
      break;
    case "recovery":
      if (firstRunState.canonicalRoot) showTarget();
      setFirstRunSectionVisible(elements.setupRecoveryActions, true);
      const recoveryActions = firstRunRecoveryActions(
        firstRunState.recoveryMode,
        firstRunState.checkpoint,
      );
      elements.setupResume.hidden = !recoveryActions.resume;
      elements.setupOpenRecovery.hidden = !recoveryActions.open;
      elements.setupRecoveryHelp.hidden = !recoveryActions.help;
      elements.setupRecoveryChoose.hidden = !recoveryActions.chooseAnother;
      elements.setupReturnWelcome.hidden = !recoveryActions.returnToWelcome;
      setSetupContent({
        progress: "Recovery required",
        eyebrow: "Project recovery",
        heading: firstRunState.recoveryMode === "durable"
          ? "Resume this project setup"
          : "This project needs recovery",
        detail: firstRunState.message || "This folder contains project state that cannot be changed safely.",
        error: firstRunState.checkpoint && firstRunState.checkpoint !== "none"
          ? `Last durable checkpoint: ${firstRunState.checkpoint}. The project and its setup choices are preserved.`
          : firstRunState.recoveryMode === "durable"
          ? "The initialization operation is preserved. Check its authoritative state before continuing."
          : firstRunState.recoveryMode === "blocked"
          ? "The preserved checkpoint cannot be resumed automatically. p-track will not repair or remove project files."
          : "p-track will not repair or remove project files automatically.",
      });
      break;
    case "failed":
      if (firstRunState.canonicalRoot) showTarget();
      setFirstRunSectionVisible(elements.setupRecoveryActions, true);
      elements.setupRetry.hidden = !firstRunState.canonicalRoot;
      elements.setupRecoveryChoose.hidden = !firstRunState.canonicalRoot;
      elements.setupReturnWelcome.textContent = firstRunState.errorKind === "project-not-found"
        ? "Cancel Setup"
        : "Return to Welcome";
      setSetupContent({
        progress: "Setup stopped",
        eyebrow: "Project setup",
        heading: firstRunState.intent === "open"
          ? "This project could not be opened"
          : firstRunState.errorKind === "project-not-found"
          ? "This folder is no longer available."
          : "This project was not initialized",
        detail: firstRunState.message || "p-track could not safely complete setup.",
        error: "No project files were written by this attempt. Retry checks the folder again before any write.",
      });
      break;
  }
  if (focus) focusFirstRunTarget();
}

function setFirstPlanState(event, focus = false) {
  firstPlanState = reduceFirstPlan(firstPlanState, event);
  renderFirstPlanOnboarding(focus);
}

function focusFirstPlanTarget() {
  const target = document.getElementById(firstPlanFocusTarget(firstPlanState));
  requestAnimationFrame(() => target?.focus());
}

function renderFirstPlanOnboarding(focus = false) {
  const active = firstPlanState.phase !== "idle" &&
    workspaceController.state.status === "open";
  updateAboutUpdatesAvailability();
  setFirstRunSectionVisible(elements.onboarding, active);
  elements.planList.inert = active;
  elements.sidebarToggle.disabled = active;
  elements.sidebarResize.inert = active;
  if (terminalHandle) terminalHandle.setLayoutLocked(active);
  else if (active) {
    elements.boardPanelToggle.disabled = true;
    elements.terminalPanelToggle.disabled = true;
  }
  if (!active) return;

  elements.stateScreen.hidden = false;
  elements.workspace.hidden = true;
  elements.overviewPage.hidden = true;
  elements.welcomePanel.hidden = true;
  elements.welcomePanel.inert = true;
  elements.setupPanel.hidden = true;
  elements.setupPanel.inert = true;
  elements.navBoard.disabled = true;
  elements.navOverview.disabled = true;
  elements.switchProject.disabled = true;
  elements.closeProject.disabled = true;

  elements.stateCard.removeAttribute("aria-busy");
  setFirstRunSectionVisible(elements.onboardingPlanForm, false);
  setFirstRunSectionVisible(elements.onboardingTaskForm, false);
  setFirstRunSectionVisible(elements.onboardingStartFailedActions, false);
  elements.onboardingPlanError.textContent = "";
  elements.onboardingTaskError.textContent = "";
  elements.onboardingStatus.textContent = "";
  elements.onboardingError.textContent = "";
  elements.onboardingPlanTitle.removeAttribute("aria-invalid");
  elements.onboardingTaskTitle.removeAttribute("aria-invalid");
  setAriaBoolean(
    elements.onboardingOperation,
    "aria-busy",
    ["creating-plan", "creating-task", "starting-task"].includes(
      firstPlanState.phase,
    ),
  );

  if (["plan", "creating-plan", "plan-failed"].includes(firstPlanState.phase)) {
    setFirstRunSectionVisible(elements.onboardingPlanForm, true);
    elements.onboardingProgress.textContent = "Next step · Plan";
    elements.onboardingHeading.textContent = "Create the first plan";
    elements.onboardingDetail.textContent =
      "Give this project an active plan, or skip for now and use the empty workspace.";
    elements.onboardingPlanTitle.value = firstPlanState.planTitle;
    elements.onboardingPlanError.textContent = firstPlanState.planError;
    setAriaBoolean(
      elements.onboardingPlanTitle,
      "aria-invalid",
      Boolean(firstPlanState.planError),
    );
    const actions = postProjectOnboardingActions(
      firstPlanState.phase === "plan-failed" ? "plan-failed" : "plan",
    );
    elements.onboardingCreatePlan.textContent = actions.primary;
    elements.onboardingSkipPlan.textContent = actions.secondary;
    elements.onboardingPlanForm.inert = firstPlanState.phase === "creating-plan";
    if (firstPlanState.phase === "creating-plan") {
      elements.onboardingStatus.textContent = "Saving or reconciling the first plan…";
    } else if (firstPlanState.phase === "plan-failed") {
      elements.onboardingError.textContent = firstPlanState.message;
    }
  } else if (["task", "creating-task", "task-create-failed"].includes(
    firstPlanState.phase,
  )) {
    setFirstRunSectionVisible(elements.onboardingTaskForm, true);
    elements.onboardingProgress.textContent = "Next step · Task";
    elements.onboardingHeading.textContent = "Add the first task";
    elements.onboardingDetail.textContent =
      "The plan is active. Add one task, then choose whether to start it now.";
    elements.onboardingActivePlan.textContent = firstPlanState.activePlanTitle;
    elements.onboardingTaskTitle.value = firstPlanState.taskTitle;
    elements.onboardingStartNow.checked = firstPlanState.startNow;
    elements.onboardingTaskError.textContent = firstPlanState.taskError;
    setAriaBoolean(
      elements.onboardingTaskTitle,
      "aria-invalid",
      Boolean(firstPlanState.taskError),
    );
    const actions = postProjectOnboardingActions(
      firstPlanState.phase === "task-create-failed" ? "task-create-failed" : "task",
    );
    elements.onboardingCreateTask.textContent = actions.primary;
    elements.onboardingFinishWithPlan.textContent = actions.secondary;
    elements.onboardingTaskForm.inert = firstPlanState.phase === "creating-task";
    if (firstPlanState.phase === "creating-task") {
      elements.onboardingStatus.textContent = "Saving or reconciling the first task…";
    } else if (firstPlanState.phase === "task-create-failed") {
      elements.onboardingError.textContent = firstPlanState.message;
    }
  } else if (firstPlanState.phase === "starting-task") {
    elements.onboardingProgress.textContent = "Next step · Start task";
    elements.onboardingHeading.textContent = "Reconciling the requested start…";
    elements.onboardingDetail.textContent =
      `Task #${firstPlanState.taskId} is durable. p-track is reconciling the requested start.`;
    elements.onboardingStatus.textContent = "Checking the explicit start request…";
  } else if (firstPlanState.phase === "task-start-failed") {
    setFirstRunSectionVisible(elements.onboardingStartFailedActions, true);
    elements.onboardingProgress.textContent = "Task saved · Start stopped";
    const actions = postProjectOnboardingActions("task-start-failed");
    elements.onboardingHeading.textContent = "Check whether the task started";
    elements.onboardingDetail.textContent =
      `Task #${firstPlanState.taskId} and plan “${firstPlanState.activePlanTitle}” are durable. Retry safely to reconcile its status.`;
    elements.onboardingRetryStart.textContent = actions.primary;
    elements.onboardingFinishSetup.textContent = actions.secondary;
    elements.onboardingError.textContent = firstPlanState.message;
  }
  if (focus) focusFirstPlanTarget();
}

function beginFirstPlanOnboarding(generation) {
  setFirstPlanState({ type: "begin", generation }, true);
}

function sidebarHeadingUnavailableForFocus() {
  return sidebarHidden || window.matchMedia("(max-width: 600px)").matches;
}

async function finishFirstPlanOnboarding(planId = firstPlanState.planId) {
  setFirstPlanState({ type: "finish" });
  elements.stateCard.removeAttribute("aria-busy");
  elements.stateScreen.hidden = true;
  elements.workspace.inert = false;
  elements.workspace.removeAttribute("aria-busy");
  elements.overviewPage.inert = false;
  elements.overviewPage.removeAttribute("aria-busy");
  elements.navBoard.disabled = false;
  elements.navOverview.disabled = false;
  elements.switchProject.disabled = false;
  elements.closeProject.disabled = false;
  view = "board";
  applyView();
  document.getElementById(
    firstPlanExitFocusTarget(planId, sidebarHeadingUnavailableForFocus()),
  )?.focus();
  await loadSnapshot(planId > 0 ? planId : 0);
}

function onboardingContextIsCurrent(ticket, phase) {
  const current = workspaceController.capture();
  return workspaceController.state.status === "open" &&
    current.epoch === ticket.epoch &&
    current.generation === ticket.generation &&
    firstPlanState.generation === ticket.generation &&
    firstPlanState.phase === phase;
}

function onboardingResponseIsCurrent(ticket, phase, generation) {
  return onboardingContextIsCurrent(ticket, phase) &&
    workspaceController.accepts(ticket, generation);
}

async function submitFirstPlan(event) {
  event.preventDefault();
  if (!(firstPlanState.phase === "plan" || firstPlanState.phase === "plan-failed")) return;
  const validation = validateOnboardingTitle(elements.onboardingPlanTitle.value, "plan");
  if (validation.error) {
    setFirstPlanState({
      type: "planInvalid",
      title: elements.onboardingPlanTitle.value,
      message: validation.error,
    }, true);
    return;
  }
  const ticket = workspaceController.capture();
  setFirstPlanState({ type: "createPlan", title: validation.value }, true);
  try {
    const result = await createFirstPlan(
      api(),
      ticket.generation,
      validation.value,
    );
    if (!onboardingResponseIsCurrent(
      ticket,
      "creating-plan",
      result.state.generation,
    )) return;
    setFirstPlanState({
      type: "planCreated",
      planId: result.plan.id,
      title: result.plan.title,
    }, true);
  } catch (error) {
    if (!onboardingContextIsCurrent(ticket, "creating-plan")) return;
    setFirstPlanState({
      type: "planFailed",
      message: `p-track could not confirm the first plan. Try Again to reconcile safely: ${messageFrom(error)}`,
    }, true);
  }
}

async function submitFirstTask(event) {
  event.preventDefault();
  if (!(firstPlanState.phase === "task" || firstPlanState.phase === "task-create-failed")) return;
  const validation = validateOnboardingTitle(elements.onboardingTaskTitle.value, "task");
  const startNow = elements.onboardingStartNow.checked;
  if (validation.error) {
    setFirstPlanState({
      type: "taskInvalid",
      title: elements.onboardingTaskTitle.value,
      startNow,
      message: validation.error,
    }, true);
    return;
  }
  const ticket = workspaceController.capture();
  const planId = firstPlanState.planId;
  setFirstPlanState({
    type: "createTask",
    title: validation.value,
    startNow,
  }, true);
  try {
    const result = await createFirstTask(
      api(),
      ticket.generation,
      planId,
      validation.value,
    );
    if (!onboardingResponseIsCurrent(
      ticket,
      "creating-task",
      result.state.generation,
    )) return;
    setFirstPlanState({
      type: "taskCreated",
      taskId: result.task.id,
      updatedAt: result.task.updatedAt,
      status: result.task.status,
    }, true);
    if (result.task.status === "doing" || !startNow) {
      await finishFirstPlanOnboarding(planId);
      return;
    }
    await startFirstTask();
  } catch (error) {
    if (!onboardingContextIsCurrent(ticket, "creating-task")) return;
    setFirstPlanState({
      type: "taskFailed",
      message: `p-track could not confirm the first task. Try Again to reconcile safely: ${messageFrom(error)}`,
    }, true);
  }
}

async function startFirstTask() {
  if (firstPlanState.phase !== "starting-task") return;
  const ticket = workspaceController.capture();
  const planId = firstPlanState.planId;
  const taskId = firstPlanState.taskId;
  const taskTitle = firstPlanState.taskTitle;
  const expectedUpdatedAt = firstPlanState.taskUpdatedAt;
  try {
    const result = await runStartFirstTask(
      api(),
      ticket.generation,
      planId,
      taskId,
      taskTitle,
      expectedUpdatedAt,
    );
    if (!onboardingResponseIsCurrent(
      ticket,
      "starting-task",
      result.state.generation,
    )) return;
    setFirstPlanState({ type: "taskStarted" });
    await finishFirstPlanOnboarding(planId);
  } catch (error) {
    if (!onboardingContextIsCurrent(ticket, "starting-task")) return;
    setFirstPlanState({
      type: "taskStartFailed",
      message: `p-track could not confirm whether the task started. Try Starting Again to reconcile safely: ${messageFrom(error)}`,
    }, true);
  }
}

function retryFirstTaskStart() {
  if (firstPlanState.phase !== "task-start-failed") return;
  setFirstPlanState({ type: "retryStart" }, true);
  void startFirstTask();
}

async function loadRecentProjects({
  focusKey = "",
  announcement = "",
  errorMessage = "",
} = {}) {
  if (
    workspaceController.state.status === "open" ||
    workspaceController.state.status === "loading" ||
    recentProjectsState.phase !== "idle" ||
    recentProjectsState.listLoading
  ) return false;
  const request = ++recentListRequest;
  const ticket = workspaceController.capture();
  const operationSequence = recentOperationSequence;
  setRecentProjectsState({ type: "loadStarted" });
  try {
    const projects = parseRecentProjects(await api().GetRecentProjectsV1());
    const current = workspaceController.capture();
    if (
      request !== recentListRequest ||
      operationSequence !== recentOperationSequence ||
      current.epoch !== ticket.epoch ||
      current.generation !== ticket.generation ||
      workspaceController.state.status === "open" ||
      workspaceController.state.status === "loading"
    ) return false;
    setRecentProjectsState({ type: "loaded", projects, announcement });
    if (errorMessage) {
      setRecentProjectsState({ type: "alert", message: errorMessage });
    }
    if (focusKey) {
      requestAnimationFrame(() => {
        const focusCurrent = workspaceController.capture();
        if (
          request !== recentListRequest ||
          operationSequence !== recentOperationSequence ||
          recentProjectsState.phase !== "idle" ||
          recentProjectsState.listLoading ||
          firstRunState.phase !== "idle" ||
          focusCurrent.epoch !== ticket.epoch ||
          focusCurrent.generation !== ticket.generation ||
          workspaceController.state.status === "open" ||
          workspaceController.state.status === "loading"
        ) return;
        recentProjectActionElement(focusKey)?.focus();
      });
    }
    return true;
  } catch (error) {
    const current = workspaceController.capture();
    if (
      request !== recentListRequest ||
      operationSequence !== recentOperationSequence ||
      current.epoch !== ticket.epoch ||
      current.generation !== ticket.generation ||
      workspaceController.state.status === "open" ||
      workspaceController.state.status === "loading"
    ) return false;
    setRecentProjectsState({
      type: "loadFailed",
      message: errorMessage
        ? `${errorMessage} Registry reload also failed: ${messageFrom(error)}`
        : `Recent projects are unavailable: ${messageFrom(error)}`,
    });
    if (focusKey) restoreRecentProjectFocus(focusKey);
    return false;
  }
}

function applyView() {
  const open = workspaceState.status === "open";
  elements.workspace.hidden = !open || view !== "board";
  elements.overviewPage.hidden = !open || view !== "overview";
  elements.navBoard.classList.toggle("active", view === "board");
  elements.navOverview.classList.toggle("active", view === "overview");
  if (view === "board") elements.navBoard.setAttribute("aria-current", "page");
  else elements.navBoard.removeAttribute("aria-current");
  if (view === "overview") elements.navOverview.setAttribute("aria-current", "page");
  else elements.navOverview.removeAttribute("aria-current");
  terminalHandle?.setVisible(open && view === "board");
}

function setView(nextView, focusHeading = false) {
  if (firstPlanState.phase !== "idle") return;
  view = nextView === "overview" ? "overview" : "board";
  applyView();
  if (view === "overview") {
    requestAnimationFrame(fitRecentMemory);
    void loadHeatmap();
    void loadRepoStats();
  }
  recordProjectLayout();
  if (focusHeading) {
    const focusedView = view;
    requestAnimationFrame(() => {
      if (view !== focusedView || workspaceController.state.status !== "open") return;
      const heading = {
        board: elements.planTitle,
        overview: elements.overviewHeading,
      }[focusedView];
      heading?.focus();
    });
  }
}

function renderWorkspaceState(state, focus = false) {
  const wasOpen = workspaceState.status === "open";
  workspaceState = state;
  if (typeof state.version === "string") {
    const version = appVersionLabel(state.version);
    elements.appVersion.textContent = version;
    elements.appVersion.setAttribute(
      "aria-label",
      `About p-track version ${version} and check for updates`,
    );
  }
  const open = state.status === "open";
  if (open && !wasOpen) restoreProjectLayout(state.project?.root || "");
  applyView();
  elements.stateScreen.hidden = open;
  elements.navBoard.disabled = !open;
  elements.navOverview.disabled = !open;
  elements.switchProject.hidden = !open;
  elements.closeProject.hidden = !open;
  elements.openProject.hidden = true;
  elements.workspace.removeAttribute("aria-busy");
  elements.workspace.inert = false;
  elements.overviewPage.removeAttribute("aria-busy");
  elements.overviewPage.inert = false;
  elements.switchProject.disabled = false;
  elements.closeProject.disabled = false;

  if (open) {
    firstRunState = { ...initialFirstRunState };
    renderFirstRunFlow(false);
    elements.projectName.textContent = state.project?.name || "Project workspace";
    if (!wasOpen) {
      elements.planTotal.textContent = "0";
      elements.planList.replaceChildren(emptyMemory("Loading plans…"));
    }
    void loadRecentProjects();
    void ensureTerminalDock(state.generation, state.project.root);
    // The restored view loads its own page the same way a click on it would.
    if (!wasOpen) setView(view);
    if (firstPlanState.phase !== "idle") {
      renderFirstPlanOnboarding(focus);
      return;
    }
    if (focus) {
      requestAnimationFrame(() => {
        (sidebarHeadingUnavailableForFocus()
          ? elements.planTitle
          : elements.projectName).focus();
      });
    }
    return;
  }

  firstPlanState = { ...initialFirstPlanState };
  renderFirstPlanOnboarding(false);
  elements.stateCard.removeAttribute("aria-busy");
  snapshotSequence += 1;
  activeSnapshotRequest = null;
  queuedSnapshotPlanId = 0;
  refreshGate.cancelQueued();
  runtimeRefreshes.cancel();
  agentActivityAnnouncementKey = "";
  elements.agentActivityLive.textContent = "";
  closeAgentLaunchPicker(false, true);
  closeTerminalAssociationEditor(false, true);
  closeTerminalWriteback(false, true);
  closeTaskTransition(false, false, true);
  disposeTerminalDock();
  closeTaskDetail();
  closePalette();
  heatmapRequested = false;
  repoStatsRequested = false;
  repoStats = null;
  board = null;
  snapshot = null;
  elements.projectName.textContent = "Project workspace";
  elements.planTotal.textContent = "0";
  elements.planList.replaceChildren(emptyMemory("No project open."));
  const copy = workspaceStateCopy(state.status, state.error);
  elements.stateEyebrow.textContent = copy.eyebrow;
  elements.stateHeading.textContent = copy.heading;
  elements.stateDetail.textContent = copy.detail;
  elements.stateOpen.hidden = state.status === "loading";
  elements.stateInitialize.hidden = state.status === "loading" || state.status !== "welcome";
  if (firstRunState.phase === "idle") renderFirstRunFlow(false);
  if (state.status !== "loading") void loadRecentProjects();
  if (focus) {
    requestAnimationFrame(() => {
      if (state.status === "welcome" && !elements.stateInitialize.hidden) {
        elements.stateInitialize.focus();
      } else if (!elements.stateOpen.hidden) elements.stateOpen.focus();
      else elements.stateHeading.focus();
    });
  }
}

function publishBackendState(state, transition, focus = false, keepInert = false) {
  const published = workspaceController.publish(
    { status: state.status, generation: Number(state.generation || 0) },
    transition,
  );
  if (!published) return false;
  renderWorkspaceState(state, focus);
  if (state.status === "open" && keepInert) {
    elements.workspace.inert = true;
    elements.workspace.setAttribute("aria-busy", "true");
    elements.overviewPage.inert = true;
    elements.overviewPage.setAttribute("aria-busy", "true");
  }
  if (state.status === "open" && !keepInert) {
    void loadSnapshot(restoredPlanId(state.project?.root));
  }
  return true;
}

function beginWorkspaceTransition() {
  closeTerminalAssociationEditor(false, true);
  closeTerminalWriteback(false, true);
  closeTaskTransition(false, false, true);
  const transition = workspaceController.beginTransition();
  if (workspaceState.status === "open") {
    elements.workspace.inert = true;
    elements.workspace.setAttribute("aria-busy", "true");
    elements.overviewPage.inert = true;
    elements.overviewPage.setAttribute("aria-busy", "true");
    elements.switchProject.disabled = true;
    elements.closeProject.disabled = true;
    setStatus("Preparing project transition…");
  } else {
    renderWorkspaceState({
      status: "loading",
      generation: transition.generation,
    });
  }
  return transition;
}

async function recoverWorkspaceState(error) {
  showError(error);
  try {
    const state = await api().GetWorkspaceState();
    workspaceController.publish({
      status: state.status,
      generation: Number(state.generation || 0),
    });
    renderWorkspaceState(state, true);
    if (state.status === "open") await loadSnapshot(board?.planId || 0);
  } catch (stateError) {
    workspaceController.publish({ status: "error", generation: 0 });
    renderWorkspaceState(
      { status: "error", generation: 0, error: messageFrom(stateError) },
      true,
    );
  }
}

async function chooseProjectDirectory(purpose) {
  const path = await api().PickProjectDirectory(purpose);
  return typeof path === "string" ? path : "";
}

function firstRunStoragePath(root) {
  const separator = root.includes("\\") ? "\\" : "/";
  return `${root.replace(/[\\/]$/, "")}${separator}.ptrack${separator}ptrack.redb`;
}

async function openExactProject(root) {
  let transition = beginWorkspaceTransition();
  const outcome = await runExactProjectOpen(
    api(),
    root,
    async (result) => {
      if (!publishBackendState(result.state, transition, false, true)) {
        return "abort";
      }
      return await showWorkspaceConfirmation("switch", result.activeResources)
        ? "confirm"
        : "cancel";
    },
    () => {
      transition = beginWorkspaceTransition();
    },
  );
  if (outcome.kind === "aborted") return;
  if (outcome.kind === "cancelled") {
    renderWorkspaceState(outcome.result.state, true);
    return false;
  }
  const result = outcome.result;
  if (!publishBackendState(result.state, transition, true)) return false;
  if (result.warning) showError(result.warning);
  return result.state;
}

async function requestOpenProject(
  selectedPath = "",
  returnFocus = null,
  pickerCancelState = null,
) {
  if (recentProjectOperationActive()) return;
  const hadOpenWorkspace = workspaceState.status === "open";
  const focusId = returnFocus?.id || (hadOpenWorkspace
    ? "switch-project-button"
    : "state-open-project-button");
  let path = selectedPath;
  try {
    setFirstRunState({
      type: pickerCancelState ? "repick" : "pick",
      intent: "open",
      returnFocusId: focusId,
    });
    path ||= await chooseProjectDirectory("open");
    if (!path) {
      if (pickerCancelState) {
        setFirstRunState({ type: "pickerCancelled", restore: pickerCancelState });
      } else {
        setFirstRunState({ type: "pickerCancelled" });
      }
      if (hadOpenWorkspace) requestAnimationFrame(() => returnFocus?.focus());
      else if (pickerCancelState) requestAnimationFrame(() => returnFocus?.focus());
      else renderFirstRunFlow(true);
      return;
    }
    setFirstRunState({ type: "validate" }, !hadOpenWorkspace);
    if (hadOpenWorkspace) setStatus("Validating the selected project folder…");
    const validation = await validateInitializationTarget(api(), path);
    if (validation.kind !== "existing") {
      const message = validation.kind === "new"
        ? "This folder is not an initialized p-track project."
        : validation.reason || "This folder requires recovery before it can be opened.";
      if (hadOpenWorkspace) {
        setFirstRunState({ type: "reset", focusId });
        setStatus("Project unchanged");
        showError(new Error(message));
      } else {
        setFirstRunState({
          type: validation.kind === "recovery-required" ? "recovery" : "failed",
          canonicalRoot: validation.canonicalRoot,
          message,
        }, true);
      }
      return;
    }
    setFirstRunState({ type: "reset", focusId });
    await openExactProject(validation.canonicalRoot);
  } catch (error) {
    if (pickerCancelState && !path) {
      firstRunState = {
        ...pickerCancelState,
        message: `The folder picker is unavailable: ${messageFrom(error)}`,
      };
      renderFirstRunFlow(true);
      elements.setupError.textContent =
        `The folder picker is unavailable: ${messageFrom(error)}`;
      return;
    }
    if (hadOpenWorkspace) {
      setFirstRunState({ type: "reset", focusId });
      await recoverWorkspaceState(error);
    } else {
      setFirstRunState({
        type: "failed",
        canonicalRoot: path,
        operationId: "",
        message: messageFrom(error),
      }, true);
    }
  }
}

async function requestInitializeProject(
  returnFocus = elements.stateInitialize,
  pickerCancelState = null,
) {
  if (recentProjectOperationActive()) return;
  const focusId = returnFocus?.id || "state-initialize-project-button";
  let path = "";
  try {
    setFirstRunState({
      type: pickerCancelState ? "repick" : "pick",
      intent: "initialize",
      returnFocusId: focusId,
    });
    path = await chooseProjectDirectory("initialize");
    if (!path) {
      if (pickerCancelState) {
        setFirstRunState({ type: "pickerCancelled", restore: pickerCancelState });
        requestAnimationFrame(() => returnFocus?.focus());
      } else {
        setFirstRunState({ type: "pickerCancelled" });
        renderFirstRunFlow(true);
      }
      return;
    }
    await validateInitializeTarget(path);
  } catch (error) {
    if (pickerCancelState && !path) {
      firstRunState = {
        ...pickerCancelState,
        message: `The folder picker is unavailable: ${messageFrom(error)}`,
      };
      renderFirstRunFlow(true);
      elements.setupError.textContent =
        `The folder picker is unavailable: ${messageFrom(error)}`;
      return;
    }
    setFirstRunState({
      type: "failed",
      canonicalRoot: path,
      operationId: "",
      message: messageFrom(error),
    }, true);
  }
}

async function validateInitializeTarget(
  path,
  { durable = false, expectedOperationId = "" } = {},
  observedValidation = null,
) {
  const durableCheckpoint = firstRunState.checkpoint;
  const durableErrorKind = firstRunState.errorKind;
  try {
    setFirstRunState({ type: "validate" }, true);
    const validation = observedValidation ||
      await validateInitializationTarget(api(), path);
    if (durable && validation.kind === "recovery-required") {
      setFirstRunState({
        type: "recovery",
        canonicalRoot: validation.canonicalRoot,
        operationId: expectedOperationId,
        message: validation.reason ||
          "This preserved project setup cannot be resumed automatically.",
        checkpoint: durableCheckpoint,
        errorKind: durableErrorKind,
        durable: true,
        resumable: false,
      }, true);
      return;
    }
    if (durable && (
      validation.canonicalRoot !== path ||
      validation.kind !== "new" ||
      !validation.resume ||
      validation.operationId !== expectedOperationId
    )) {
      setFirstRunState({
        type: "recovery",
        canonicalRoot: path,
        operationId: expectedOperationId,
        message:
          "The preserved initialization operation changed during revalidation and cannot be resumed automatically.",
        checkpoint: durableCheckpoint,
        errorKind: durableErrorKind,
        durable: true,
        resumable: false,
      }, true);
      return;
    }
    if (validation.resume) {
      setFirstRunState({
        type: "resume",
        canonicalRoot: validation.canonicalRoot,
        operationId: validation.operationId,
        goal: validation.resume.goal,
        guideChoice: validation.resume.guideChoice,
        initialization: validation.resume.initialization,
      }, true);
      if (
        validation.resume.initialization.outcome === "complete" &&
        validation.resume.initialization.checkpoint === "desktop-bound"
      ) {
        await applyInitializationStatus(
          validation.operationId,
          validation.canonicalRoot,
          validation.resume.initialization,
        );
      }
      return;
    }
    if (validation.kind === "existing") {
      setFirstRunState({ type: "existing", canonicalRoot: validation.canonicalRoot }, true);
      return;
    }
    if (validation.kind === "recovery-required") {
      setFirstRunState({
        type: "recovery",
        canonicalRoot: validation.canonicalRoot,
        operationId: "",
        message: validation.reason || "This folder contains project state that cannot be changed safely.",
        checkpoint: durableCheckpoint,
        errorKind: durableErrorKind,
        durable,
      }, true);
      return;
    }
    setFirstRunState({
      type: "new",
      canonicalRoot: validation.canonicalRoot,
      operationId: validation.operationId,
    }, true);
  } catch (error) {
    setFirstRunState({
      type: durable ? "recovery" : "failed",
      canonicalRoot: path,
      operationId: durable ? expectedOperationId : "",
      message: durable
        ? `p-track could not revalidate the preserved operation: ${messageFrom(error)}`
        : messageFrom(error),
      checkpoint: durableCheckpoint,
      errorKind: durableErrorKind,
      durable,
    }, true);
  }
}

function hydratePendingInitialization(pending) {
  const event = pendingInitializationEvent(pending);
  if (!event) return false;
  setFirstRunState({ type: "validate" });
  setFirstRunState(event, true);
  return true;
}

function submitFirstRunGoal(event) {
  event.preventDefault();
  const validation = validateNorthStarGoal(elements.setupGoal.value);
  if (validation.error) {
    setFirstRunState({
      type: "goalInvalid",
      goal: elements.setupGoal.value,
      message: validation.error,
    }, true);
    return;
  }
  setFirstRunState({ type: "goalAccepted", goal: validation.value }, true);
}

function preserveFirstRunGoalDraft() {
  firstRunState = reduceFirstRun(firstRunState, {
    type: "goalDrafted",
    goal: elements.setupGoal.value,
  });
  elements.setupGoalError.textContent = "";
  elements.setupGoal.removeAttribute("aria-invalid");
}

function returnToSelectedFirstRunFolder() {
  preserveFirstRunGoalDraft();
  setFirstRunState({ type: "back" }, true);
}

async function previewFirstRunGuide() {
  if (!(firstRunState.phase === "guide" || firstRunState.phase === "guide-stale")) return;
  const operationId = firstRunState.operationId;
  const canonicalRoot = firstRunState.canonicalRoot;
  setFirstRunState({ type: "guidePreviewStarted" }, true);
  try {
    const preview = parseProjectGuidePreview(
      await api().PreviewProjectGuideV1({ operationId, root: canonicalRoot }),
    );
    if (
      firstRunState.operationId !== operationId ||
      firstRunState.canonicalRoot !== canonicalRoot
    ) return;
    setFirstRunState({ type: "guidePreviewed", preview }, true);
  } catch (error) {
    if (
      firstRunState.operationId !== operationId ||
      firstRunState.canonicalRoot !== canonicalRoot
    ) return;
    setFirstRunState({
      type: "guidePreviewFailed",
      message: `Guide preview is unavailable: ${messageFrom(error)}`,
    }, true);
  }
}

function continueFirstRunWithoutGuide() {
  setFirstRunState({ type: "guideSkipped" }, true);
}

function continueFirstRunWithGuide() {
  setFirstRunState({ type: "guideInstalled" }, true);
}

let publishedInitializationOperationId = "";

function workspaceControllerMatches(state) {
  return workspaceController.state.status === "open" &&
    workspaceController.state.generation === Number(state?.generation || 0);
}

async function rebindCompletedInitializationWorkspace(canonicalRoot) {
  let openError = new Error("The completed project could not be opened in this window.");
  try {
    const opened = await openExactProject(canonicalRoot);
    if (
      completedInitializationWorkspaceMatches(opened, canonicalRoot) &&
      workspaceControllerMatches(opened)
    ) return opened;
  } catch (error) {
    openError = error;
  }
  try {
    const refreshed = await api().GetWorkspaceState();
    workspaceController.publish({
      status: refreshed.status,
      generation: Number(refreshed.generation || 0),
    });
    renderWorkspaceState(refreshed, false);
    if (
      completedInitializationWorkspaceMatches(refreshed, canonicalRoot) &&
      workspaceControllerMatches(refreshed)
    ) return refreshed;
  } catch {
    // The exact operation remains recoverable through its durable status.
  }
  throw openError;
}

async function applyInitializationStatus(
  operationId,
  canonicalRoot,
  status,
  state = null,
  pollAttempt = 0,
) {
  if (
    firstRunState.operationId !== operationId ||
    firstRunState.canonicalRoot !== canonicalRoot
  ) return;
  if (!initializationStatusMatchesOperation(status, operationId, canonicalRoot)) {
    throw new Error("Initialization status does not match the committed operation.");
  }
  if (isProjectGuidePartiallyApplied(status.errorKind)) {
    const postCommit = status.outcome === "recovery-required" &&
      status.checkpoint === "project-committed";
    if (postCommit) {
      setFirstRunState({
        type: "guideStale",
        postCommit: true,
        checkpoint: status.checkpoint,
        skipAllowed: false,
        partiallyApplied: true,
        message: projectGuideRecoveryCopy("partially-applied").error,
      }, true);
      return;
    }
    throw new Error("A partial guide apply was reported at an unknown checkpoint.");
  }
  if (isProjectGuidePreviewStale(status.errorKind)) {
    const preCommit = status.outcome === "ready" && status.checkpoint === "none";
    const postCommit = status.outcome === "recovery-required" &&
      status.checkpoint === "project-committed";
    if (preCommit || postCommit) {
      setFirstRunState({
        type: "guideStale",
        postCommit,
        checkpoint: status.checkpoint,
      }, true);
      return;
    }
    throw new Error("Guide preview became stale at an unknown initialization checkpoint.");
  }
  if (status.outcome === "complete") {
    if (status.checkpoint !== "desktop-bound") {
      throw new Error("Initialization completed before the desktop workspace was bound.");
    }
    let workspace = state || await api().GetWorkspaceState();
    let rebound = false;
    if (!completedInitializationWorkspaceMatches(workspace, canonicalRoot)) {
      try {
        workspace = await rebindCompletedInitializationWorkspace(canonicalRoot);
        rebound = true;
      } catch (error) {
        setFirstRunState({
          type: "recovery",
          canonicalRoot,
          operationId,
          message:
            `Initialization is complete, but this window could not open the project: ${messageFrom(error)}`,
          checkpoint: status.checkpoint,
          errorKind: status.errorKind,
          durable: true,
        }, true);
        return;
      }
    }
    if (publishedInitializationOperationId === operationId) return;
    if (!rebound) {
      firstRunState = { ...initialFirstRunState };
      if (!publishBackendState(workspace, undefined, false, true)) return;
    } else if (!workspaceControllerMatches(workspace)) {
      return;
    }
    publishedInitializationOperationId = operationId;
    beginFirstPlanOnboarding(Number(workspace.generation));
    return;
  }
  if (status.outcome === "in-progress") {
    if (pollAttempt >= 20) {
      setFirstRunState({
        type: "recovery",
        message:
          "Initialization did not reach another checkpoint. Resume Setup will revalidate the preserved operation before continuing.",
        checkpoint: status.checkpoint,
        errorKind: status.errorKind,
        durable: true,
      }, true);
      return;
    }
    await new Promise((resolve) => window.setTimeout(resolve, 250));
    const next = await readInitializationStatus(api(), operationId);
    await applyInitializationStatus(
      operationId,
      canonicalRoot,
      next,
      null,
      pollAttempt + 1,
    );
    return;
  }
  if (status.outcome === "recovery-required") {
    setFirstRunState({
      type: "recovery",
      message: "Project setup made a durable change and now requires recovery before continuing.",
      checkpoint: status.checkpoint,
      errorKind: status.errorKind,
      durable: true,
    }, true);
    return;
  }
  setFirstRunState({
    type: "failed",
    message: initializationFailureMessage(status.errorKind),
    checkpoint: status.checkpoint,
    errorKind: status.errorKind,
  }, true);
}

async function reconcileInitializationStatus(
  error,
  operationId,
  canonicalRoot,
  observedStatus = null,
) {
  try {
    const status = observedStatus ||
      await readInitializationStatus(api(), operationId);
    if (isProjectGuidePartiallyApplied(error)) {
      const postCommit = status.outcome === "recovery-required" &&
        status.checkpoint === "project-committed";
      if (postCommit) {
        if (!initializationStatusMatchesOperation(status, operationId, canonicalRoot)) {
          throw new Error("Initialization status does not match the committed operation.");
        }
        setFirstRunState({
          type: "guideStale",
          postCommit: true,
          checkpoint: status.checkpoint,
          skipAllowed: false,
          partiallyApplied: true,
          message: projectGuideRecoveryCopy("partially-applied").error,
        }, true);
        return;
      }
    }
    if (isProjectGuidePreviewStale(error)) {
      const preCommit = status.outcome === "ready" && status.checkpoint === "none";
      const postCommit = status.outcome === "recovery-required" &&
        status.checkpoint === "project-committed";
      if (preCommit || postCommit) {
        if (!initializationStatusMatchesOperation(status, operationId, canonicalRoot)) {
          throw new Error("Initialization status does not match the committed operation.");
        }
        setFirstRunState({
          type: "guideStale",
          postCommit,
          checkpoint: status.checkpoint,
        }, true);
        return;
      }
    }
    await applyInitializationStatus(operationId, canonicalRoot, status);
  } catch (statusError) {
    if (
      firstRunState.operationId !== operationId ||
      firstRunState.canonicalRoot !== canonicalRoot
    ) return;
    setFirstRunState({
      type: "uncertain",
      message: `Initialization status is uncertain: ${messageFrom(statusError || error)}`,
      checkpoint: firstRunState.checkpoint,
    }, true);
  }
}

async function commitFirstRunProject() {
  if (firstRunState.phase !== "review") return;
  const operationId = firstRunState.operationId;
  const canonicalRoot = firstRunState.canonicalRoot;
  const goal = firstRunState.goal;
  const guide = projectGuideCommitFields(firstRunState);
  const request = initializeProjectRequest(
    operationId,
    canonicalRoot,
    goal,
    guide,
  );
  setFirstRunState({ type: "commit" }, true);
  const outcome = await commitInitialization(api(), request);
  if (outcome.kind === "status") {
    await reconcileInitializationStatus(
      outcome.error,
      operationId,
      canonicalRoot,
      outcome.status,
    );
    return;
  }
  if (outcome.kind === "uncertain") {
    if (
      firstRunState.operationId !== operationId ||
      firstRunState.canonicalRoot !== canonicalRoot
    ) return;
    setFirstRunState({
      type: "uncertain",
      message:
        `Initialization status is uncertain: ${messageFrom(outcome.statusError || outcome.error)}`,
      checkpoint: firstRunState.checkpoint,
    }, true);
    return;
  }
  try {
    await applyInitializationStatus(
      operationId,
      canonicalRoot,
      outcome.result.status,
      outcome.result.state,
    );
  } catch (error) {
    await reconcileInitializationStatus(error, operationId, canonicalRoot);
  }
}

async function retryInitializationStatus() {
  if (firstRunState.phase !== "uncertain") return;
  const operationId = firstRunState.operationId;
  const canonicalRoot = firstRunState.canonicalRoot;
  setFirstRunState({ type: "reconcile" }, true);
  await reconcileInitializationStatus(
    new Error("Initialization status remains unavailable."),
    operationId,
    canonicalRoot,
  );
}

function cancelFirstRunSetup() {
  if (
    firstRunState.resumeLocked ||
    firstRunState.recoveryMode === "durable" ||
    ["committing", "reconciling", "uncertain"].includes(firstRunState.phase)
  ) return;
  if (
    (firstRunState.goal || elements.setupGoal.value.trim()) &&
    !window.confirm("Cancel setup? Your project has not been initialized.")
  ) return;
  const focusId = firstRunState.returnFocusId;
  setFirstRunState({ type: "reset", focusId }, true);
}

async function openExistingFromSetup() {
  if (firstRunState.phase !== "existing" || !firstRunState.canonicalRoot) return;
  const root = firstRunState.canonicalRoot;
  setFirstRunState({ type: "reset", focusId: "state-open-project-button" });
  try {
    await openExactProject(root);
  } catch (error) {
    await recoverWorkspaceState(error);
  }
}

function chooseAnotherFirstRunFolder() {
  if (firstRunState.recoveryMode === "durable") return;
  const pickerCancelState = { ...firstRunState };
  const returnFocus = firstRunState.phase === "existing"
    ? elements.setupExistingChoose
    : firstRunState.phase === "target-new"
    ? elements.setupNewTargetChoose
    : elements.setupRecoveryChoose;
  if (firstRunState.intent === "open") {
    void requestOpenProject("", returnFocus, pickerCancelState);
  } else {
    void requestInitializeProject(returnFocus, pickerCancelState);
  }
}

function retryFirstRunValidation() {
  if (firstRunState.phase !== "failed" || !firstRunState.canonicalRoot) return;
  const root = firstRunState.canonicalRoot;
  if (firstRunState.intent === "open") {
    void requestOpenProject(root, elements.stateOpen);
  } else {
    void validateInitializeTarget(root);
  }
}

function returnFirstRunToWelcome() {
  if (
    firstRunState.phase === "failed" ||
    (firstRunState.phase === "recovery" &&
      ["blocked", "ambiguous"].includes(firstRunState.recoveryMode))
  ) {
    const focusId = firstRunState.returnFocusId;
    setFirstRunState({ type: "reset", focusId }, true);
    return;
  }
  cancelFirstRunSetup();
}

async function resumeFirstRunSetup() {
  if (
    firstRunState.phase !== "recovery" ||
    firstRunState.recoveryMode !== "durable" ||
    !firstRunState.canonicalRoot
  ) return;
  const operationId = firstRunState.operationId;
  const canonicalRoot = firstRunState.canonicalRoot;
  const checkpoint = firstRunState.checkpoint;
  setFirstRunState({ type: "reconcile" }, true);
  try {
    const outcome = await resumeInitialization(
      api(),
      operationId,
      canonicalRoot,
    );
    if (outcome.kind === "status") {
      if (!initializationStatusMatchesOperation(
        outcome.status,
        operationId,
        canonicalRoot,
      )) {
        throw new Error("Initialization status does not match the preserved operation.");
      }
      await applyInitializationStatus(operationId, canonicalRoot, outcome.status);
      return;
    }
    await validateInitializeTarget(canonicalRoot, {
      durable: true,
      expectedOperationId: operationId,
    }, outcome.validation);
  } catch (error) {
    if (
      firstRunState.operationId !== operationId ||
      firstRunState.canonicalRoot !== canonicalRoot
    ) return;
    setFirstRunState({
      type: "uncertain",
      message: `p-track could not confirm the preserved operation: ${messageFrom(error)}`,
      checkpoint,
    }, true);
  }
}

async function openProjectFromRecovery() {
  if (
    !canOpenPreservedFirstRunProject(firstRunState) ||
    !firstRunState.canonicalRoot
  ) return;
  const recovery = { ...firstRunState };
  try {
    const opened = await rebindCompletedInitializationWorkspace(
      recovery.canonicalRoot,
    );
    if (recovery.checkpoint === "desktop-bound") {
      publishedInitializationOperationId = recovery.operationId;
      beginFirstPlanOnboarding(Number(opened.generation));
    }
  } catch (error) {
    firstRunState = {
      ...recovery,
      message: `The preserved project could not be opened: ${messageFrom(error)}`,
    };
    renderFirstRunFlow(true);
  }
}

async function requestCloseProject() {
  if (workspaceController.state.status !== "open") return;
  try {
    let transition = beginWorkspaceTransition();
    let result = await api().CloseProject("");
    if (result.requiresConfirmation) {
      if (!publishBackendState(result.state, transition, false, true)) return;
      const confirmed = await showWorkspaceConfirmation("close", result.activeResources);
      if (!confirmed) {
        await api().CancelWorkspaceChange(result.confirmationToken);
        renderWorkspaceState(result.state, true);
        return;
      }
      transition = beginWorkspaceTransition();
      result = await api().CloseProject(result.confirmationToken);
    }
    if (!publishBackendState(result.state, transition, true)) return;
    if (result.warning) showError(result.warning);
    if (result.state.status === "closed") {
      window.setTimeout(async () => {
        try {
          const state = await api().GetWorkspaceState();
          workspaceController.publish({
            status: state.status,
            generation: Number(state.generation || 0),
          });
          renderWorkspaceState(state, true);
        } catch (error) {
          showError(error);
        }
      }, 350);
    }
  } catch (error) {
    await recoverWorkspaceState(error);
  }
}

function generationTerminalBackend(generation) {
  function assertGeneration(response) {
    if (Number(response.generation) !== generation) {
      throw new Error("Stale terminal response ignored");
    }
    return response;
  }
  return {
    async GetTerminalProfiles() {
      return assertGeneration(await api().GetTerminalProfilesV2(generation)).profiles;
    },
    async CreateTerminal(profileID, cwd, rows, columns) {
      return assertGeneration(
        await api().CreateTerminalV2(generation, profileID, cwd, rows, columns),
      );
    },
    async LaunchLinkedAgent(profileID, cwd, rows, columns, association) {
      return assertGeneration(
        await api().LaunchLinkedAgentV2(
          generation,
          profileID,
          cwd,
          rows,
          columns,
          association,
        ),
      );
    },
    RollbackLinkedAgent(sessionID) {
      return api().RollbackLinkedAgentLaunchV2(generation, sessionID);
    },
    // Both are fenced by the workspace generation inside the runtime, so their
    // responses carry no generation of their own to assert here.
    OpenTerminalWindow(sessions, shape) {
      return api().OpenTerminalWindow(sessions, shape);
    },
    ClaimTerminalStream(sessionID, fromSequence) {
      return api().ClaimTerminalStream(sessionID, fromSequence);
    },
    async MutateTerminalAssociation(sessionID, expectedRevision, association) {
      return assertGeneration(
        await api().MutateTerminalAssociationV2(
          generation,
          sessionID,
          expectedRevision,
          association === undefined,
          association ?? { version: 1 },
        ),
      );
    },
    async PreviewTerminalWriteback(sessionID, expectedRevision, kind, content) {
      return assertGeneration(
        await api().PreviewTerminalWritebackV2(
          generation,
          sessionID,
          expectedRevision,
          kind,
          content,
        ),
      );
    },
    async WriteTerminalMemory(
      sessionID,
      expectedRevision,
      requestID,
      kind,
      content,
      confirmSummary,
    ) {
      return assertGeneration(
        await api().WriteTerminalMemoryV2(
          generation,
          sessionID,
          expectedRevision,
          requestID,
          kind,
          content,
          confirmSummary,
        ),
      );
    },
    async ValidateTerminalCWDs(cwds) {
      return assertGeneration(
        await api().ValidateTerminalCWDsV2(generation, cwds),
      ).results;
    },
    ResizeTerminal(sessionID, rows, columns) {
      return api().ResizeTerminalV2(generation, sessionID, rows, columns);
    },
    CloseTerminal(sessionID, force) {
      return api().CloseTerminalV2(generation, sessionID, force);
    },
  };
}

async function ensureTerminalDock(generation, projectRoot) {
  if (
    terminalHandle &&
    terminalGeneration === generation &&
    terminalProjectRoot === projectRoot
  ) return;
  if (terminalHandle) {
    closeAgentLaunchPicker(false, true);
    closeTerminalAssociationEditor(false, true);
    closeTerminalWriteback(false, true);
    closeTaskTransition(false, false, true);
  }
  disposeTerminalDock();
  terminalGeneration = generation;
  terminalProjectRoot = projectRoot;
  try {
    const handle = mountTerminalDock({
      backend: generationTerminalBackend(generation),
      workspaceGeneration: generation,
      projectRoot,
      showError,
      saveUnicodeMode: (unicodeMode) => void savePreferences({ terminal: { unicodeMode } }),
    });
    terminalHandle = handle;
    handle.setLayoutLocked(firstPlanState.phase !== "idle");
    handle.setVisible(workspaceState.status === "open" && view === "board");
    applicationOverlayCoordinator.setDock(handle);
    await handle.ready;
    const current = workspaceController.state;
    if (
      terminalHandle !== handle ||
      current.generation !== generation ||
      !["open", "loading"].includes(current.status)
    ) {
      handle.dispose();
      if (terminalHandle === handle) {
        terminalHandle = null;
        applicationOverlayCoordinator.setDock(null);
      }
      return;
    }
    restorePanelLayout();
  } catch (error) {
    const current = workspaceController.state;
    if (current.status === "open" && current.generation === generation) {
      showError(error);
    }
  }
}

function disposeTerminalDock() {
  closeTerminalAssociationEditor(false, true);
  closeTerminalWriteback(false, true);
  closeTaskTransition(false, false, true);
  applicationOverlayCoordinator.setDock(null);
  terminalHandle?.dispose();
  terminalHandle = null;
  panelLayoutRestored = false;
  terminalGeneration = 0;
  terminalProjectRoot = "";
}

function boardShortcutIsBlocked(event) {
  if (
    event.isComposing ||
    workspaceController.state.status !== "open" ||
    firstRunState.phase !== "idle" ||
    firstPlanState.phase !== "idle"
  ) return true;
  const active = document.activeElement;
  const interactive =
    active instanceof HTMLElement &&
    (["INPUT", "SELECT", "TEXTAREA", "BUTTON"].includes(active.tagName) ||
      active.isContentEditable);
  const path = typeof event.composedPath === "function" ? event.composedPath() : [];
  const terminalFocused =
    (active instanceof Element &&
      Boolean(active.closest("#terminal-dock, [data-terminal-overlay]"))) ||
    path.some(
      (node) =>
        node instanceof Element &&
        (node.matches("#terminal-dock, [data-terminal-overlay]") ||
          Boolean(node.closest("#terminal-dock, [data-terminal-overlay]"))),
    );
  return interactive || terminalFocused || snapshotDialogIsOpen();
}

function nativeMenuOpenOverlayIDs() {
  return Array.from(
    document.querySelectorAll("body > .modal, body > [data-terminal-overlay]"),
  ).filter((overlay) => !overlay.hidden).map((overlay) =>
    overlay.id || (overlay.hasAttribute("data-terminal-overlay")
      ? "terminal-overlay"
      : "application-overlay")
  );
}

function nativeMenuFocusTarget() {
  const active = document.activeElement;
  if (active instanceof Element && active.closest("#terminal-dock")) {
    return "terminal";
  }
  if (
    active instanceof HTMLElement &&
    (["INPUT", "SELECT", "TEXTAREA"].includes(active.tagName) ||
      active.isContentEditable)
  ) return "input";
  return "other";
}

function nativeCommandAllowed(command) {
  if (
    firstRunState.phase !== "idle" ||
    firstPlanState.phase !== "idle" ||
    recentProjectOperationActive()
  ) return false;
  return nativeMenuCommandAllowed(command, {
    workspaceStatus: workspaceController.state.status,
    openOverlayIDs: nativeMenuOpenOverlayIDs(),
    focusTarget: nativeMenuFocusTarget(),
  });
}

function trapModalFocus(event) {
  if (event.key !== "Tab") return;
  const modal = applicationOverlayCoordinator.activeOverlay;
  if (!(modal instanceof HTMLElement)) return;
  const policy = applicationOverlayKeyboardPolicy(
    modal.id,
    modal.hasAttribute("data-terminal-overlay"),
  );
  if (!policy.trapTab) return;
  const focusable = Array.from(
    modal.querySelectorAll(
      [
        'button:not([disabled]):not([tabindex="-1"])',
        'input:not([disabled]):not([hidden])',
        'textarea:not([disabled]):not([hidden])',
        'select:not([disabled])',
        '[tabindex]:not([tabindex="-1"])',
      ].join(", "),
    ),
  ).filter((item) => !item.hidden && !item.closest("[hidden]"));
  if (focusable.length === 0) return;
  const first = focusable[0];
  const current = focusable.indexOf(document.activeElement);
  const next = focusCycleIndex(focusable.length, current, event.shiftKey);
  if (next < 0) return;
  event.preventDefault();
  (focusable[next] || first).focus();
}

function closeActiveApplicationOverlay(event) {
  if (event.key !== "Escape" || event.defaultPrevented) return false;
  const modal = applicationOverlayCoordinator.activeOverlay;
  if (!(modal instanceof HTMLElement)) return false;
  const { escapeAction } = applicationOverlayKeyboardPolicy(
    modal.id,
    modal.hasAttribute("data-terminal-overlay"),
  );
  if (!escapeAction) return false;
  event.preventDefault();
  event.stopImmediatePropagation();
  if (escapeAction === "dialog") closeDialog();
  else if (escapeAction === "memory") closeMemoryHistory();
  else if (escapeAction === "settings") closeSettings();
  else if (escapeAction === "updates") closeAboutUpdates();
  else if (escapeAction === "drawer") closeTaskDetail();
  else if (escapeAction === "agent-launch") closeAgentLaunchPicker();
  else if (escapeAction === "terminal-association") {
    closeTerminalAssociationEditor();
  } else if (escapeAction === "terminal-writeback") closeTerminalWriteback();
  else if (escapeAction === "task-transition") closeTaskTransition();
  else if (escapeAction === "workspace-confirm") {
    finishWorkspaceConfirmation(false);
  } else if (escapeAction === "palette") closePalette();
  return true;
}

function eventsOn(name, callback) {
  const runtime = window.runtime;
  if (typeof runtime?.EventsOnMultiple !== "function") return () => {};
  return runtime.EventsOnMultiple(name, callback, -1);
}

function registerNativeProjectActions() {
  const showNativeView = (command) => {
    if (!nativeCommandAllowed(command)) return;
    const target = nativeMenuViewTarget(command);
    if (target) setView(target, true);
  };
  nativeEventDisposers.push(
    ...registerNativeMenuActions(eventsOn, {
      openProject: () => {
        if (
          firstRunState.phase === "idle" &&
          nativeCommandAllowed("openProject")
        ) void requestOpenProject();
      },
      switchProject: () => {
        if (
          firstRunState.phase === "idle" &&
          nativeCommandAllowed("switchProject")
        ) void requestOpenProject();
      },
      closeProject: () => {
        if (
          firstRunState.phase === "idle" &&
          nativeCommandAllowed("closeProject")
        ) void requestCloseProject();
      },
      showSettings: () => {
        if (nativeCommandAllowed("showSettings")) openSettings(elements.settingsOpen);
      },
      showBoard: () => {
        showNativeView("showBoard");
      },
      showIntelligence: () => {
        showNativeView("showIntelligence");
      },
      toggleTerminalPanel: () => {
        if (nativeCommandAllowed("toggleTerminalPanel")) {
          document.querySelector("#terminal-panel-toggle")?.click();
        }
      },
      toggleCommandPalette: () => {
        if (!nativeCommandAllowed("toggleCommandPalette")) return;
        if (elements.palette.hidden) openPalette();
        else closePalette();
      },
      installShellCommand: () => {
        if (nativeCommandAllowed("installShellCommand")) {
          void api().InstallShellCommand();
        }
      },
      checkForUpdates: () => {
        if (
          nativeCommandAllowed("checkForUpdates") &&
          openAboutUpdates(elements.appVersion)
        ) void runUpdateAction("check");
      },
    }),
    eventsOn("update:state-changed", (state) => renderUpdateState(state)),
    eventsOn("workspace:data-changed", () =>
      void loadSnapshot(board?.planId || 0, true),
    ),
    eventsOn("workspace:runtime-changed", (generation) => {
      if (!runtimeEventIsCurrent(
        generation,
        workspaceController.state.generation,
        workspaceController.state.status === "open",
      )) return;
      runtimeRefreshes.request(Number(generation));
    }),
  );
}

initializeSidebarLayout();
elements.sidebarToggle.addEventListener("click", () => {
  if (firstPlanState.phase !== "idle") return;
  setSidebarHidden(!sidebarHidden);
});
elements.sidebarResize.addEventListener("pointerdown", beginSidebarResize);
elements.sidebarResize.addEventListener("keydown", resizeSidebarFromKeyboard);
window.addEventListener("resize", () => setSidebarWidth(sidebarWidth, false));

elements.navBoard.addEventListener("click", () => setView("board"));
elements.navOverview.addEventListener("click", () => setView("overview"));
elements.agentHandoffForm.addEventListener("submit", (event) => {
  event.preventDefault();
  const sourceRunId = elements.agentHandoffSource.value;
  const targetRunId = elements.agentHandoffTarget.value;
  if (!sourceRunId || !targetRunId || sourceRunId === targetRunId) {
    showError(new Error("Choose two distinct live agents for the handoff."));
    return;
  }
  const source = snapshot?.agentActivity?.items?.find((item) => item.runId === sourceRunId);
  const target = snapshot?.agentActivity?.items?.find((item) => item.runId === targetRunId);
  const sourceRevision = Number(source?.association?.revision || 0);
  const targetRevision = Number(target?.association?.revision || 0);
  void runMutation(
    (generation) => api().SendAgentHandoffV2(
      generation,
      sourceRunId,
      targetRunId,
      sourceRevision,
      targetRevision,
    ),
    "Sending bounded handoff proposal…",
    "Could not send handoff proposal",
  );
});
elements.agentWorkflowKind.addEventListener("change", () => {
	const needsTarget = ["pullRequest", "merge"].includes(elements.agentWorkflowKind.value);
	elements.agentWorkflowTarget.disabled = !needsTarget;
	elements.agentWorkflowPrepare.disabled = !elements.agentWorkflowRun.value ||
		(needsTarget && !elements.agentWorkflowTarget.value);
});
elements.agentWorkflowForm.addEventListener("submit", (event) => {
	event.preventDefault();
	const runId = elements.agentWorkflowRun.value;
	const kind = elements.agentWorkflowKind.value;
	const needsTarget = ["pullRequest", "merge"].includes(kind);
	const target = needsTarget ? elements.agentWorkflowTarget.value : "";
	const run = snapshot?.agentActivity?.items?.find((item) => item.runId === runId);
	if (!runId || !run?.live || (needsTarget && !target)) {
		showError(new Error("Choose a live agent and an eligible target branch."));
		return;
	}
	void runMutation(
		(generation) => api().PrepareAgentWorkflowV2(
			generation,
			runId,
			Number(run.association?.revision || 0),
			kind,
			target,
		),
		"Preparing exact workflow proposal…",
		"Could not prepare workflow proposal",
	);
});
window.addEventListener("focus", () => {
  if (workspaceController.state.status !== "open") return;
  void loadSnapshot(board?.planId || 0, true);
});

elements.paletteInput.addEventListener("input", schedulePaletteSearch);
elements.paletteInput.addEventListener("keydown", (event) => {
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    movePaletteActive(event.key === "ArrowDown" ? 1 : -1);
  } else if (event.key === "Enter") {
    event.preventDefault();
    activatePaletteResult(paletteItems[paletteActive]);
  } else if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    closePalette();
  }
});
document.querySelectorAll("[data-close-palette]").forEach((element) => {
  element.addEventListener("click", closePalette);
});

const themeController = initTheme({
  root: document.documentElement,
  storage: localStorage,
  media: matchMedia("(prefers-color-scheme: light)"),
  onChange: (theme) => {
    // Show the theme a click switches to: sun in dark mode, moon in light.
    elements.themeToggle.textContent = theme === "dark" ? "☀" : "☾";
    elements.themeToggle.title =
      theme === "dark" ? "Switch to light theme" : "Switch to dark theme";
  },
});
elements.themeToggle.addEventListener("click", () => {
  // The topbar toggle is the same setting as Appearance ▸ Color theme, so it
  // writes through to the stored record instead of only the cache.
  void savePreferences({ appearance: { theme: themeController.toggle() } });
});

elements.openProject.addEventListener("click", (event) =>
  void requestOpenProject("", event.currentTarget),
);
elements.switchProject.addEventListener("click", (event) =>
  void requestOpenProject("", event.currentTarget),
);
elements.closeProject.addEventListener("click", () => void requestCloseProject());
elements.stateInitialize.addEventListener("click", (event) =>
  void requestInitializeProject(event.currentTarget),
);
elements.stateOpen.addEventListener("click", (event) =>
  void requestOpenProject("", event.currentTarget),
);
elements.setupGoalForm.addEventListener("submit", submitFirstRunGoal);
elements.setupGoal.addEventListener("input", preserveFirstRunGoalDraft);
elements.setupGoalBack.addEventListener("click", returnToSelectedFirstRunFolder);
elements.setupGoalCancel.addEventListener("click", cancelFirstRunSetup);
elements.setupGuidePreviewButton.addEventListener("click", () => void previewFirstRunGuide());
elements.setupGuideReviewAgain.addEventListener("click", () => void previewFirstRunGuide());
elements.setupGuideSkip.addEventListener("click", continueFirstRunWithoutGuide);
elements.setupGuidePreviewSkip.addEventListener("click", continueFirstRunWithoutGuide);
elements.setupGuideStaleSkip.addEventListener("click", continueFirstRunWithoutGuide);
elements.setupGuideInstall.addEventListener("click", continueFirstRunWithGuide);
elements.setupGuideBack.addEventListener("click", () =>
  setFirstRunState({ type: "back" }, true),
);
elements.setupGuidePreviewBack.addEventListener("click", () =>
  setFirstRunState({ type: "back" }, true),
);
elements.setupGuideStaleBack.addEventListener("click", () =>
  setFirstRunState({ type: "back" }, true),
);
elements.setupGuideCancel.addEventListener("click", cancelFirstRunSetup);
elements.setupGuidePreviewCancel.addEventListener("click", cancelFirstRunSetup);
elements.setupGuideStaleCancel.addEventListener("click", cancelFirstRunSetup);
elements.setupReviewBack.addEventListener("click", () =>
  setFirstRunState({ type: "back" }, true),
);
elements.setupReviewCancel.addEventListener("click", cancelFirstRunSetup);
elements.setupCommit.addEventListener("click", () => void commitFirstRunProject());
elements.setupOpenExisting.addEventListener("click", () => void openExistingFromSetup());
elements.setupExistingChoose.addEventListener("click", chooseAnotherFirstRunFolder);
elements.setupExistingCancel.addEventListener("click", cancelFirstRunSetup);
elements.setupNewTargetContinue.addEventListener("click", () =>
  setFirstRunState({ type: "continueToGoal" }, true)
);
elements.setupNewTargetChoose.addEventListener("click", chooseAnotherFirstRunFolder);
elements.setupNewTargetCancel.addEventListener("click", cancelFirstRunSetup);
elements.setupRetry.addEventListener("click", retryFirstRunValidation);
elements.setupResume.addEventListener("click", () => void resumeFirstRunSetup());
elements.setupOpenRecovery.addEventListener("click", () => void openProjectFromRecovery());
elements.setupRecoveryHelp.addEventListener("click", () =>
  openHelpDestination("project-recovery")
);
elements.setupRecoveryChoose.addEventListener("click", chooseAnotherFirstRunFolder);
elements.setupReturnWelcome.addEventListener("click", returnFirstRunToWelcome);
elements.setupCheckStatus.addEventListener("click", () => void retryInitializationStatus());
elements.onboardingPlanForm.addEventListener("submit", (event) => void submitFirstPlan(event));
elements.onboardingSkipPlan.addEventListener("click", () => void finishFirstPlanOnboarding(0));
elements.onboardingTaskForm.addEventListener("submit", (event) => void submitFirstTask(event));
elements.onboardingFinishWithPlan.addEventListener("click", () =>
  void finishFirstPlanOnboarding(firstPlanState.planId),
);
elements.onboardingRetryStart.addEventListener("click", retryFirstTaskStart);
elements.onboardingFinishSetup.addEventListener("click", () =>
  void finishFirstPlanOnboarding(firstPlanState.planId),
);
elements.activityMore.addEventListener("click", openMemoryHistory);
elements.appVersion.addEventListener("click", (event) => {
  openAboutUpdates(event.currentTarget);
});
elements.updatesClose.addEventListener("click", closeAboutUpdates);
document.querySelectorAll("[data-close-updates]").forEach((element) => {
  element.addEventListener("click", closeAboutUpdates);
});
elements.updatesAutomatic.addEventListener("change", (event) => {
  void setAutomaticUpdateChecks(event.currentTarget.checked);
});
elements.updatesPrimary.addEventListener("click", () => {
  void runUpdateAction(elements.updatesPrimary.dataset.action);
});
elements.updatesCancel.addEventListener("click", async () => {
  updateCancelRequested = true;
  try {
    renderUpdateState(await api().CancelUpdateOperation());
  } catch {
    updateCancelRequested = false;
    await refreshUpdateState();
    showError(new Error("The update operation could not be canceled."));
  }
  if (!updateActionBusy) updateCancelRequested = false;
});
elements.updatesReleasePage.addEventListener("click", openUpdateReleasePage);
elements.aboutProject.addEventListener("click", () => {
  openProjectURL(projectRepositoryURL);
});
elements.aboutLicenseLink.addEventListener("click", () => {
  openProjectURL(projectLicenseURL);
});
elements.aboutHelp.addEventListener("click", () => openHelpDestination("help-center"));
elements.aboutReport.addEventListener("click", () => openHelpDestination("report-issue"));

elements.settingsOpen.addEventListener("click", (event) => {
  openSettings(event.currentTarget);
});
elements.settingsClose.addEventListener("click", closeSettings);
document.querySelectorAll("[data-close-settings]").forEach((element) => {
  element.addEventListener("click", closeSettings);
});
elements.settingsSectionList.addEventListener("click", (event) => {
  const tab = event.target.closest('[role="tab"]');
  if (tab) selectSettingsSection(tab.id.replace("settings-tab-", ""));
});
elements.settingsSectionList.addEventListener("keydown", (event) => {
  const next = nextSettingsSectionIndex(
    event.key,
    settingsSectionIndex(settingsSection),
    settingsSections.length,
  );
  if (next < 0) return;
  event.preventDefault();
  selectSettingsSection(settingsSections[next].id, true);
});
elements.settingsStartupRestore.addEventListener("change", (event) => {
  void savePreferences({
    startup: { restoreLastProject: event.currentTarget.checked },
  });
});
elements.settingsTheme.addEventListener("change", (event) => {
  void savePreferences({ appearance: { theme: event.currentTarget.value } });
});
elements.settingsDensity.addEventListener("change", (event) => {
  void savePreferences({ appearance: { density: event.currentTarget.value } });
});
elements.settingsReducedMotion.addEventListener("change", (event) => {
  void savePreferences({ appearance: { reducedMotion: event.currentTarget.value } });
});
elements.settingsTerminalProfile.addEventListener("change", (event) => {
  void savePreferences({
    terminal: { defaultProfileId: event.currentTarget.value || null },
  });
});
elements.settingsTerminalFontFamily.addEventListener("change", (event) => {
  void savePreferences({ terminal: { fontFamily: event.currentTarget.value } });
});
elements.settingsTerminalFontSize.addEventListener("change", (event) => {
  void savePreferences({ terminal: { fontSize: Number(event.currentTarget.value) } });
});
elements.settingsTerminalUnicode.addEventListener("change", (event) => {
  void savePreferences({ terminal: { unicodeMode: event.currentTarget.value } });
});
elements.settingsTerminalScrollback.addEventListener("change", (event) => {
  void savePreferences({ terminal: { scrollback: Number(event.currentTarget.value) } });
});
elements.settingsTerminalRenderer.addEventListener("change", (event) => {
  void savePreferences({ terminal: { renderer: event.currentTarget.value } });
});
elements.settingsUpdatesAutomatic.addEventListener("change", (event) => {
  void setAutomaticUpdateChecks(event.currentTarget.checked);
});
elements.settingsResetWindowLayout.addEventListener("click", (event) => {
  void resetWindowLayout(event.currentTarget);
});
elements.settingsResetApplicationState.addEventListener("click", (event) => {
  void resetApplicationState(event.currentTarget);
});
elements.settingsOpenUpdates.addEventListener("click", () => {
  const invoker = elements.settingsOpen;
  closeSettings();
  openAboutUpdates(invoker);
});
elements.settingsReset.addEventListener("click", () => void resetPreferences());
elements.planLaunchAgent.addEventListener("click", (event) => {
  if (!board?.planId) return;
  void openAgentLaunchPicker(
    { planId: Number(board.planId) },
    event.currentTarget,
  );
});
elements.confirmCancel.addEventListener("click", () => finishWorkspaceConfirmation(false));
elements.confirmSubmit.addEventListener("click", () => finishWorkspaceConfirmation(true));

function currentBoardPlan() {
  if (!board?.planId) return null;
  return { id: Number(board.planId), title: board.planTitle || `Plan #${board.planId}` };
}
elements.planTitle.addEventListener("contextmenu", (event) => {
  const plan = currentBoardPlan();
  if (!plan) return;
  event.preventDefault();
  openPlanContextMenu(plan, elements.planTitle, elements.planTitle, {
    x: event.clientX,
    y: event.clientY,
  });
});
elements.planTitleMenu.addEventListener("click", () => {
  const plan = currentBoardPlan();
  if (!plan) return;
  const rect = elements.planTitleMenu.getBoundingClientRect();
  openPlanContextMenu(plan, elements.planTitle, elements.planTitleMenu, {
    x: rect.left,
    y: rect.bottom + 4,
  });
});
elements.planDialogProject.addEventListener("input", syncPlanTransferState);
elements.planDialogProject.addEventListener("change", syncPlanTransferState);
elements.planDialogTitle.addEventListener("input", syncPlanTransferState);
elements.planDialogForm.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  event.preventDefault();
  closePlanDialog();
});
elements.planDialogForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (!planDialogPlan || !planDialogMode) return;
  const plan = planDialogPlan;
  const mode = planDialogMode;
  elements.planDialogError.hidden = true;
  elements.planDialogSubmit.disabled = true;
  try {
    const ticket = workspaceController.capture();
    if (mode === "delete") {
      await api().DeletePlanV1(ticket.generation, Number(plan.id), true);
    } else if (mode === "move") {
      await api().MovePlanV1(
        ticket.generation,
        Number(plan.id),
        planDialogTransferState.targetPath,
        planDialogTransferState.title.trim(),
      );
    } else {
      await api().CopyPlanV1(
        ticket.generation,
        Number(plan.id),
        planDialogTransferState.targetPath,
        planDialogTransferState.title.trim(),
      );
    }
    closePlanDialog();
    await loadSnapshot(0);
  } catch (error) {
    setPlanDialogError(error);
    elements.planDialogSubmit.disabled = mode === "delete"
      ? false
      : transferSubmitDisabled(planDialogTransferState);
  }
});
document.querySelectorAll("[data-close-plan-dialog]").forEach((element) => {
  element.addEventListener("click", closePlanDialog);
});

elements.addForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const title = elements.taskTitle.value.trim();
  if (!title || !board?.planId) return;
  const ticket = workspaceController.capture();
  await runMutation(
    async (generation) => {
      const result = await api().AddTaskV2(generation, Number(board.planId), title);
      if (workspaceController.accepts(ticket, Number(result.generation))) {
        elements.taskTitle.value = "";
      }
      return result;
    },
    "Adding task…",
    "Could not add task",
  );
  if (workspaceController.accepts(ticket, ticket.generation)) {
    elements.taskTitle.focus();
  }
});

elements.dialogForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (!editingTask) return;
  const task = editingTask;
  if (dialogMode === "rename") {
    const title = elements.dialogInput.value.trim();
    if (!title) return;
    closeDialog();
    await runMutation(
      (generation) => api().RenameTaskV2(generation, Number(task.id), title),
      `Renaming task #${task.id}…`,
      `Could not rename task #${task.id}`,
    );
  } else {
    const note = elements.dialogNote.value.trim();
    if (!note) return;
    closeDialog();
    await runMutation(
      (generation) => api().AddTaskNoteV2(generation, Number(task.id), note),
      `Recording memory for task #${task.id}…`,
      `Could not record memory for task #${task.id}`,
    );
  }
});

document.querySelectorAll("[data-close-modal]").forEach((element) => {
  element.addEventListener("click", closeDialog);
});
document.querySelectorAll("[data-close-memory-modal]").forEach((element) => {
  element.addEventListener("click", closeMemoryHistory);
});
elements.memoryDialogClose.addEventListener("click", closeMemoryHistory);
document.querySelectorAll("[data-close-drawer]").forEach((element) => {
  element.addEventListener("click", closeTaskDetail);
});
elements.drawerClose.addEventListener("click", closeTaskDetail);
elements.drawerStatusSelect.addEventListener("change", (event) => {
  if (!detailTask) return;
  void moveTask(
    detailTask.id,
    elements.drawerStatusSelect.value,
    event.currentTarget,
  );
});
elements.drawerRename.addEventListener("click", () => {
  if (detailTask) openRename(detailTask);
});
elements.drawerMemory.addEventListener("click", () => {
  if (detailTask) openMemory(detailTask);
});
elements.drawerLaunchAgent.addEventListener("click", (event) => {
  if (!detailTask || !board?.planId) return;
  void openAgentLaunchPicker(
    { planId: Number(board.planId), task: detailTask },
    event.currentTarget,
  );
});
elements.agentLaunchForm.addEventListener("submit", (event) => {
  event.preventDefault();
  void submitAgentLaunch();
});
elements.agentLaunchCancel.addEventListener("click", () => closeAgentLaunchPicker());
document.querySelectorAll("[data-close-agent-launch]").forEach((element) => {
  element.addEventListener("click", () => closeAgentLaunchPicker());
});
elements.terminalLinkContext.addEventListener("click", (event) => {
  openTerminalAssociationEditor(event.currentTarget);
});
elements.terminalHelp.addEventListener("click", () => {
  openHelpDestination("terminals");
});
elements.terminalAssociationForm.addEventListener("submit", (event) => {
  event.preventDefault();
  void submitTerminalAssociation(false);
});
elements.terminalAssociationDetach.addEventListener("click", () => {
  void submitTerminalAssociation(true);
});
elements.terminalAssociationCancel.addEventListener("click", () =>
  closeTerminalAssociationEditor()
);
document.querySelectorAll("[data-close-terminal-association]").forEach((element) => {
  element.addEventListener("click", () => closeTerminalAssociationEditor());
});
elements.terminalWriteback.addEventListener("click", (event) => {
  openTerminalWriteback(event.currentTarget);
});
elements.terminalWritebackForm.addEventListener("submit", (event) => {
  event.preventDefault();
  void previewTerminalWriteback();
});
elements.terminalWritebackKind.addEventListener("change", invalidateTerminalWritebackPreview);
elements.terminalWritebackContent.addEventListener("input", invalidateTerminalWritebackPreview);
elements.terminalWritebackSummaryConfirm.addEventListener("change", () => {
  const preview = terminalWritebackRequest?.preview;
  elements.terminalWritebackSave.disabled = !preview ||
    (preview.replacesSummary && !elements.terminalWritebackSummaryConfirm.checked);
});
elements.terminalWritebackSave.addEventListener("click", () => {
  void commitTerminalWriteback();
});
elements.terminalWritebackCancel.addEventListener("click", () => closeTerminalWriteback());
document.querySelectorAll("[data-close-terminal-writeback]").forEach((element) => {
  element.addEventListener("click", () => closeTerminalWriteback());
});
elements.taskTransitionForm.addEventListener("submit", (event) => {
  event.preventDefault();
  void confirmTaskTransition();
});
elements.taskTransitionCancel.addEventListener("click", () => closeTaskTransition());
document.querySelectorAll("[data-close-task-transition]").forEach((element) => {
  element.addEventListener("click", () => closeTaskTransition());
});

document.addEventListener("keydown", (event) => {
  trapModalFocus(event);
  if (event.key === "Escape") {
    if (event.defaultPrevented || closeActiveApplicationOverlay(event)) return;
  }
  const command = commandShortcut({
    key: event.key,
    composing: event.isComposing,
    meta: event.metaKey,
    ctrl: event.ctrlKey,
    alt: event.altKey,
    shift: event.shiftKey,
    repeat: event.repeat,
    prevented: event.defaultPrevented,
  });
  if (command === "palette") {
    // ⌘K works globally, even while typing in an input.
    event.preventDefault();
    if (elements.palette.hidden) openPalette();
    else closePalette();
    return;
  }
  if (command === "settings") {
    // ⌘, opens the application dialog, including with no project open.
    event.preventDefault();
    if (elements.settingsModal.hidden) openSettings(document.activeElement);
    else closeSettings();
    return;
  }
  if (command && !boardShortcutIsBlocked(event)) {
    event.preventDefault();
    if (command === "board") setView("board", true);
    if (command === "overview") setView("overview", true);
    if (command === "addTask") {
      setView("board");
      elements.taskTitle.focus();
    }
  }
  const shortcut = shortcutIntent({
    key: event.key,
    composing: event.isComposing,
    meta: event.metaKey,
    ctrl: event.ctrlKey,
    alt: event.altKey,
    shift: event.shiftKey,
    repeat: event.repeat,
    prevented: event.defaultPrevented,
  });
  if (shortcut === "refresh" && !boardShortcutIsBlocked(event)) {
    event.preventDefault();
    void loadSnapshot();
  }
  if (shortcut === "addTask" && !boardShortcutIsBlocked(event)) {
    event.preventDefault();
    elements.taskTitle.focus();
  }
});

if ("ResizeObserver" in window) {
  new ResizeObserver(() => requestAnimationFrame(fitRecentMemory)).observe(
    elements.activity,
  );
}

window.addEventListener("beforeunload", () => {
  sidebarDragCleanup?.();
  layoutStateScheduler.flush();
  refreshLoop.dispose();
  runtimeRefreshes.cancel();
  disposeTerminalDock();
  nativeEventDisposers.splice(0).forEach((dispose) => dispose());
});

async function start() {
  let startupError = null;
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      api();
      // The stored record is the authority, so it lands before the terminal
      // dock or the first paint-sensitive surface reads its cache.
      await loadPreferences();
      await loadLayoutState();
      const startup = await resolveFirstRunStartupState(
        () => api().GetWorkspaceState(),
        () => api().GetPendingInitializationV1(),
      );
      const state = startup.state;
      workspaceController.publish({
        status: state.status,
        generation: Number(state.generation || 0),
      });
      renderWorkspaceState(state, false);
      const restored = state.status === "welcome" && startup.pending
        ? hydratePendingInitialization(startup.pending)
        : false;
      if (state.status === "welcome" && !restored) {
        requestAnimationFrame(() => elements.stateInitialize.focus());
      }
      registerNativeProjectActions();
      refreshLoop.start();
      if (state.status === "open") {
        await loadSnapshot(restoredPlanId(state.project?.root));
      }
      return;
    } catch (error) {
      startupError = error;
      await new Promise((resolve) => window.setTimeout(resolve, 100));
    }
  }
  workspaceController.publish({ status: "error", generation: 0 });
  renderWorkspaceState(
    {
      status: "error",
      generation: 0,
      error: `Could not load the desktop startup state: ${messageFrom(startupError)}`,
    },
    true,
  );
}

// ------------------------------------------------------ terminal window mode

async function waitForBridge() {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      return api();
    } catch {
      await new Promise((resolve) => window.setTimeout(resolve, 100));
    }
  }
  throw new Error("The desktop runtime is not ready");
}

/**
 * A terminal window renders its tab's split tree and nothing else: it asks
 * which tab it owns, re-hydrates the shape through the same controller and
 * split view the main window uses, claims one renderer lease per pane, and
 * attaches. Closing the window returns the whole tab to the main window, so
 * the pop-in control is just a window close reachable from the keyboard.
 * Only focus and split resizes may change the shape here; a resize is pushed
 * back into the assignment so pop-in returns the tab as last seen.
 */
async function startTerminalWindow(label) {
  const section = document.getElementById("terminal-window");
  const status = document.getElementById("terminal-window-status");
  const gap = document.getElementById("terminal-window-gap");
  const gapDetail = document.getElementById("terminal-window-gap-detail");
  const host = document.getElementById("terminal-window-host");
  section.hidden = false;
  gapDetail.textContent = terminalGapNotice;
  const showGap = () => {
    gap.hidden = false;
  };

  try {
    await waitForBridge();
    const assignment = await api().GetTerminalWindowTab(label);
    const sessions = assignment?.sessions;
    if (!sessions || sessions.length === 0) {
      status.textContent = "This window no longer shows a terminal. Close it.";
      return;
    }
    const workspaceState = await api().GetWorkspaceState();
    const generation = Number(workspaceState?.generation || 0);

    const controller = new WorkspaceTabController(
      createCryptoIdFactory(),
      {
        version: 1,
        activeTabId: assignment.shape?.id ?? "",
        tabs: [assignment.shape],
      },
      {
        // The tab's structure belongs to the main window; this window may
        // focus panes and resize splits, nothing else.
        allowAction: (action) =>
          action.type === "resize-split" || action.type === "focus-pane",
      },
    );
    const tab = controller.workspace.tabs[0];
    if (!tab) {
      status.textContent = "This window no longer shows a terminal. Close it.";
      return;
    }
    const heading = document.getElementById("terminal-window-heading");
    if (tab.title) {
      heading.textContent = tab.title;
      document.title = `Terminal — ${tab.title}`;
    }

    // Sessions were recorded in pane order when the tab moved; the traversal
    // order survives normalization because the tree structure does.
    // The same stored Settings overrides the main window applies: the shared
    // profile record plus this origin's localStorage, so a font or scrollback
    // choice means the same thing in every window (§4).
    const overrides = readTerminalPreferenceOverrides(localStorage);
    const profiles = await api().GetTerminalProfiles().catch(() => []);
    const settingsForProfile = (profileId) => {
      const profile = profiles.find((candidate) => candidate.id === profileId);
      return normalizeTerminalProfileSettings({
        ...(profile ?? {}),
        fontFamily: overrides.fontFamily || profile?.fontFamily,
        scrollback: overrides.scrollback || profile?.scrollback,
      });
    };

    const paneOrder = paneIds(tab.root);
    const panes = new Map();
    for (const [index, paneId] of paneOrder.entries()) {
      const sessionId = sessions[index];
      if (!sessionId) continue;
      const paneHost = document.createElement("div");
      paneHost.className = "terminal-window-pane";
      const profileId = findTerminalPane(tab.root, paneId)?.profileId ?? "";
      const settings = settingsForProfile(profileId);
      const fontSize = readTerminalProfileFontSize(localStorage, profileId, settings.fontSize);
      const terminal = new Terminal({
        allowProposedApi: true,
        cursorBlink: true,
        rescaleOverlappingGlyphs: true,
        ...terminalRendererOptions(settings, fontSize),
      });
      const fit = new FitAddon();
      terminal.loadAddon(fit);
      const search = new SearchAddon();
      terminal.loadAddon(search);
      terminal.open(paneHost);
      terminal.textarea?.setAttribute(
        "aria-label",
        paneOrder.length === 1 ? "Terminal session" : `Terminal pane ${index + 1}`,
      );
      panes.set(paneId, {
        sessionId,
        terminal,
        fit,
        search,
        host: paneHost,
        profileId,
        fontSize,
        baseFontSize: settings.fontSize,
        state: "connecting",
        sequence: 0,
        attempts: 0,
        reclaiming: false,
        ended: false,
        client: null,
      });
    }

    // One status line for the window: the least-connected pane speaks for it,
    // and a shell that ended says so in its own scrollback.
    const renderStatus = () => {
      const states = [...panes.values()].map((pane) => pane.state);
      const aggregate = ["error", "closed", "connecting"].find((candidate) =>
        states.includes(candidate),
      ) ?? "open";
      status.textContent = terminalWindowStatusLabel(aggregate);
    };

    const fitPane = (pane) => {
      pane.fit.fit();
      void api()
        .ResizeTerminalV2(generation, pane.sessionId, pane.terminal.rows, pane.terminal.cols)
        .catch(() => {});
    };

    const splitView = new WorkspaceSplitView({
      container: host,
      controller,
      hostForPane: (paneId) => panes.get(paneId)?.host ?? null,
      // Panes are closed where the tab lives — the chrome is hidden here.
      closePane: () => {},
      fitPanes: (paneIdList) => {
        for (const paneId of paneIdList) {
          const pane = panes.get(paneId);
          if (pane) requestAnimationFrame(() => fitPane(pane));
        }
      },
    });

    // A resized split is pushed back into the assignment, debounced, so the
    // tab pops back in with the geometry the user last saw in this window.
    let shapePush = null;
    controller.subscribe((workspace, previous) => {
      splitView.refresh(workspace);
      const current = workspace.tabs[0];
      const before = previous.tabs[0];
      if (current && current.activePaneId !== before?.activePaneId) {
        panes.get(current.activePaneId)?.terminal.focus();
      }
      if (!current || current.root === before?.root) return;
      window.clearTimeout(shapePush ?? undefined);
      shapePush = window.setTimeout(() => {
        void api()
          .SetTerminalWindowTab(label, sessions, current)
          .catch(() => {});
      }, 300);
    });

    // ------------------------------------------------- per-session surfaces
    // The same search, paste guard, and zoom the dock offers (§4); project
    // chrome — writeback, diagnostics, the association editor — stays in the
    // window that owns the tab.
    const activePane = () => panes.get(controller.workspace.tabs[0]?.activePaneId ?? "");
    const searchBar = document.getElementById("terminal-window-search");
    const searchInput = document.getElementById("terminal-window-search-input");
    const searchResults = document.getElementById("terminal-window-search-results");
    const searchClose = document.getElementById("terminal-window-search-close");
    const searchOptions = (incremental) => ({
      incremental,
      decorations: {
        matchBackground: "#26483e",
        matchBorder: "#3dd6a3",
        matchOverviewRuler: "#3dd6a3",
        activeMatchBackground: "#7a5f1f",
        activeMatchBorder: "#ffd75f",
        activeMatchColorOverviewRuler: "#ffd75f",
      },
    });
    const runSearch = (incremental, backwards = false) => {
      const pane = activePane();
      if (!pane) return;
      const query = searchInput.value;
      if (!query) {
        pane.search.clearDecorations();
        searchResults.textContent = "";
        return;
      }
      const found = backwards
        ? pane.search.findPrevious(query, searchOptions(false))
        : pane.search.findNext(query, searchOptions(incremental));
      if (!found) searchResults.textContent = "No results";
    };
    const openSearch = () => {
      searchBar.hidden = false;
      searchInput.focus();
      searchInput.select();
    };
    const closeSearch = () => {
      activePane()?.search.clearDecorations();
      searchBar.hidden = true;
      searchResults.textContent = "";
      activePane()?.terminal.focus();
    };
    searchInput.addEventListener("input", () => runSearch(true));
    searchInput.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeSearch();
      } else if (event.key === "Enter") {
        event.preventDefault();
        runSearch(false, event.shiftKey);
      }
    });
    searchClose.addEventListener("click", closeSearch);

    const zoomPane = (pane, nextSize) => {
      pane.fontSize = clampTerminalFontSize(nextSize);
      pane.terminal.options.fontSize = pane.fontSize;
      if (pane.profileId) {
        writeTerminalProfileFontSize(localStorage, pane.profileId, pane.fontSize);
      }
      requestAnimationFrame(() => fitPane(pane));
    };

    for (const pane of panes.values()) {
      pane.search.onDidChangeResults((result) => {
        if (pane !== activePane()) return;
        searchResults.textContent = terminalSearchResultLabel(
          result,
          searchInput.value !== "",
        );
      });
      pane.terminal.attachCustomKeyEventHandler((event) => {
        const action = terminalShortcutAction(
          event,
          /Mac|iPhone|iPad/.test(navigator.platform) ? "mac" : "linux",
          pane.terminal.hasSelection(),
        );
        if (!action) return true;
        if (event.type !== "keydown" || event.repeat) return false;
        event.preventDefault();
        switch (action) {
          case "search":
            openSearch();
            break;
          case "zoom-in":
            zoomPane(pane, pane.fontSize + 1);
            break;
          case "zoom-out":
            zoomPane(pane, pane.fontSize - 1);
            break;
          case "zoom-reset":
            zoomPane(pane, pane.baseFontSize);
            break;
          case "copy":
            void navigator.clipboard
              ?.writeText(pane.terminal.getSelection())
              .catch(() => {});
            break;
          case "select-all":
            pane.terminal.selectAll();
            break;
          case "clear":
            pane.terminal.clear();
            break;
          default:
            // Paste arrives through the DOM paste event below, where the
            // clipboard's own payload feeds the guard.
            return true;
        }
        return false;
      });
      // The dock's paste guard, fed by the event's own clipboard payload: a
      // multi-line paste outside the alternate screen asks first.
      pane.terminal.textarea?.addEventListener("paste", (event) => {
        event.preventDefault();
        event.stopPropagation();
        const request = prepareClipboardPaste(
          event.clipboardData?.getData("text") ?? "",
          pane.terminal.buffer.active.type === "alternate",
        );
        void commitClipboardPaste(
          request,
          (pending) => Promise.resolve(window.confirm(
            `Paste ${pending.lineCount} lines into the terminal?`,
          )),
          (text) => pane.terminal.paste(text),
        );
      });

      // One client per attach: the stream ticket is single-use and the
      // client's write generation is what stops input from a released
      // renderer reaching a re-claimed PTY, so a re-attach builds a new one
      // instead of reopening it.
      const attach = (url, from) => {
        pane.sequence = Number(from || 0);
        const next = new TerminalStreamClient({
          createWebSocket: (streamUrl) => new WebSocket(streamUrl),
          // The rendered byte count is the sequence: a re-claim resumes
          // exactly where the renderer stopped drawing, not where the socket
          // stopped.
          writeOutput: (output, done) => pane.terminal.write(output, () => {
            pane.sequence += output.byteLength;
            done();
          }),
          onStateChange: (state) => {
            if (pane.client !== next) return;
            pane.state = state;
            renderStatus();
            // Only a stream that opened earns a fresh re-claim budget.
            if (state === "open") pane.attempts = 0;
            if (state === "closed" || state === "error") scheduleReclaim();
          },
          onGap: showGap,
        });
        pane.client = next;
        next.connect(url);
      };

      // A stream that ended without the shell ending is recoverable: claim
      // the lease back from the last rendered sequence, bounded so it cannot
      // spin.
      const scheduleReclaim = () => {
        if (pane.reclaiming) return;
        pane.reclaiming = true;
        void reclaimStream({
          recoverable: () => !pane.ended,
          sequence: () => pane.sequence,
          wait: (delay) => new Promise((resolve) => window.setTimeout(resolve, delay)),
          claim: (fromSequence) => api().ClaimTerminalStream(pane.sessionId, fromSequence),
          attach: (claim) => {
            if (claim.gap) showGap();
            attach(claim.url, claim.fromSequence);
          },
          reclaiming: () => {
            pane.attempts += 1;
            status.textContent = reclaimingStreamNotice;
          },
          exhausted: () => {
            status.textContent = streamReclaimFailedNotice;
          },
        }, pane.attempts).finally(() => {
          pane.reclaiming = false;
        });
      };

      pane.terminal.onData((data) => {
        for (const chunk of splitTerminalInput(terminalTextToBytes(data))) {
          pane.client?.sendInput(chunk);
        }
      });
      pane.terminal.onBinary((data) => {
        for (const chunk of splitTerminalInput(binaryStringToBytes(data))) {
          pane.client?.sendInput(chunk);
        }
      });

      const claim = await api().ClaimTerminalStream(pane.sessionId, 0);
      if (claim.gap) showGap();
      attach(claim.url, claim.fromSequence);
    }

    window.runtime?.EventsOnMultiple?.("terminal:exit", (payload) => {
      for (const pane of panes.values()) {
        if (payload?.sessionId !== pane.sessionId) continue;
        pane.ended = true;
        pane.state = "closed";
        status.textContent = payload.error || `Exited (${payload.exitCode})`;
      }
    }, -1);

    const fitAll = () => {
      for (const pane of panes.values()) fitPane(pane);
    };
    window.addEventListener("resize", () => requestAnimationFrame(fitAll));
    // A theme picked in the main window reaches this one through the shared
    // stored record; the OS preference path is already followed by initTheme.
    window.addEventListener("storage", (event) => {
      if (event.key !== THEME_STORAGE_KEY && event.key !== null) return;
      document.documentElement.dataset.theme = resolveTheme(
        event.key === null ? null : event.newValue,
        matchMedia("(prefers-color-scheme: light)").matches,
      );
    });
    fitAll();
    renderStatus();
    panes.get(tab.activePaneId)?.terminal.focus();
  } catch (error) {
    status.textContent = messageFrom(error);
  }
}

const terminalWindow = terminalWindowLabel(window.location.hash);
if (terminalWindow) void startTerminalWindow(terminalWindow);
else void start();
