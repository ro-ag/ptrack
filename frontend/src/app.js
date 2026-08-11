import { mountTerminalDock } from "./terminal/pane";
import {
  linkedAssociationPointer,
  selectedInstalledAgentProfile,
} from "./terminal/linked-launch";
import {
  stableTerminalWritebackRequestID,
  terminalWritebackContentPolicy,
} from "./terminal/writeback";
import { initTheme } from "./theme";
import {
  canEnableCapability,
  canStartCapabilitySave,
  capabilityResponseIsCurrent,
  capabilityRiskGrants,
  capabilityStateLabel,
  diagnosticLabel,
  gitCapabilityNeedsSSH,
  splitCapabilityList,
} from "./capabilities/presentation";
import {
  clampSidebarWidth,
  defaultSidebarWidth,
  sidebarHiddenStorageKey,
  sidebarMaximumWidth,
  sidebarWidthFromKey,
  sidebarWidthStorageKey,
  storedSidebarWidth,
} from "./workspace/layout";
import {
  RefreshGate,
  RefreshLoop,
  RuntimeRefreshCoalescer,
  WorkspaceController,
} from "./workspace/controller";
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
  focusCycleIndex,
  groupSearchResults,
  handoffPreviewResponseIsCurrent,
  heatmapWeeks,
  linkedTaskRuntimePresentation,
  mutationFocusFallback,
  paletteTarget,
  preserveSectionOnError,
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
  workspace: document.querySelector("#workspace"),
  overviewPage: document.querySelector("#overview-page"),
  settingsPage: document.querySelector("#settings-page"),
  navBoard: document.querySelector("#nav-board"),
  navOverview: document.querySelector("#nav-overview"),
  navSettings: document.querySelector("#nav-settings"),
  stateScreen: document.querySelector("#workspace-state-screen"),
  stateEyebrow: document.querySelector("#workspace-state-eyebrow"),
  stateHeading: document.querySelector("#workspace-state-heading"),
  stateDetail: document.querySelector("#workspace-state-detail"),
  stateOpen: document.querySelector("#state-open-project-button"),
  recents: document.querySelector("#recent-project-list"),
  board: document.querySelector("#board"),
  appVersion: document.querySelector("#app-version"),
  projectName: document.querySelector("#project-name"),
  planTitle: document.querySelector("#plan-title"),
  planTotal: document.querySelector("#plan-total"),
  planList: document.querySelector("#sidebar-plan-list"),
  planProgress: document.querySelector("#plan-progress"),
  planProgressLabel: document.querySelector("#plan-progress-label"),
  planLaunchAgent: document.querySelector("#plan-launch-agent"),
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
  confirmModal: document.querySelector("#workspace-confirm-modal"),
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
  capabilityNew: document.querySelector("#capability-new"),
  capabilityClear: document.querySelector("#capability-clear"),
  capabilityForm: document.querySelector("#capability-form"),
  capabilityEditorTitle: document.querySelector("#capability-editor-title"),
  capabilityID: document.querySelector("#capability-id"),
  capabilityName: document.querySelector("#capability-name"),
  capabilityProfile: document.querySelector("#capability-profile"),
  capabilityKind: document.querySelector("#capability-kind"),
  capabilityDuration: document.querySelector("#capability-duration"),
  capabilityTimeout: document.querySelector("#capability-timeout"),
  capabilityResponseLimit: document.querySelector("#capability-response-limit"),
  capabilityRequestLimit: document.querySelector("#capability-request-limit"),
  capabilityOutputLimit: document.querySelector("#capability-output-limit"),
  capabilityConcurrency: document.querySelector("#capability-concurrency"),
  capabilityRedirects: document.querySelector("#capability-redirects"),
  capabilityAuditRetain: document.querySelector("#capability-audit-retain"),
  capabilityAudit: document.querySelector("#capability-audit"),
  capabilityHTTPFields: document.querySelector("#capability-http-fields"),
  capabilityHTTPURL: document.querySelector("#capability-http-url"),
  capabilityHTTPMethods: document.querySelector("#capability-http-methods"),
  capabilityHTTPPaths: document.querySelector("#capability-http-paths"),
  capabilityGitFields: document.querySelector("#capability-git-fields"),
  capabilityGitName: document.querySelector("#capability-git-name"),
  capabilityGitURL: document.querySelector("#capability-git-url"),
  capabilityGitSSHID: document.querySelector("#capability-git-ssh-id"),
  capabilityGitOperations: document.querySelector("#capability-git-operations"),
  capabilityGitBranches: document.querySelector("#capability-git-branches"),
  capabilityGitRefspecs: document.querySelector("#capability-git-refspecs"),
  capabilityGitTags: document.querySelector("#capability-git-tags"),
  capabilityGitForce: document.querySelector("#capability-git-force"),
  capabilityGitDelete: document.querySelector("#capability-git-delete"),
  capabilitySSHFields: document.querySelector("#capability-ssh-fields"),
  capabilitySSHAlias: document.querySelector("#capability-ssh-alias"),
  capabilitySSHHost: document.querySelector("#capability-ssh-host"),
  capabilitySSHPort: document.querySelector("#capability-ssh-port"),
  capabilitySSHUser: document.querySelector("#capability-ssh-user"),
  capabilitySSHKey: document.querySelector("#capability-ssh-key"),
  capabilitySSHCommands: document.querySelector("#capability-ssh-commands"),
  capabilitySSHGit: document.querySelector("#capability-ssh-git"),
  capabilitySSHUpload: document.querySelector("#capability-ssh-upload"),
  capabilitySSHDownload: document.querySelector("#capability-ssh-download"),
  capabilitySSHShell: document.querySelector("#capability-ssh-shell"),
  capabilitySSHUploadLocal: document.querySelector("#capability-ssh-upload-local"),
  capabilitySSHUploadRemote: document.querySelector("#capability-ssh-upload-remote"),
  capabilitySSHDownloadLocal: document.querySelector("#capability-ssh-download-local"),
  capabilitySSHDownloadRemote: document.querySelector("#capability-ssh-download-remote"),
  capabilitySSHLocalForward: document.querySelector("#capability-ssh-local-forward"),
  capabilitySSHRemoteForward: document.querySelector("#capability-ssh-remote-forward"),
  capabilityPreviewButton: document.querySelector("#capability-preview"),
  capabilityTestButton: document.querySelector("#capability-test"),
  capabilitySaveButton: document.querySelector("#capability-save"),
  capabilityPreviewResult: document.querySelector("#capability-preview-result"),
  capabilityEffectiveScope: document.querySelector("#capability-effective-scope"),
  capabilityRiskSummary: document.querySelector("#capability-risk-summary"),
  capabilityDiagnostic: document.querySelector("#capability-diagnostic"),
  capabilityTotal: document.querySelector("#capability-total"),
  capabilityList: document.querySelector("#capability-list"),
  capabilityAuditList: document.querySelector("#capability-audit-list"),
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
let view = "board";
let snapshot = null;
let board = null;
let draggedTask = null;
let editingTask = null;
let dialogMode = "rename";
let toastTimer = null;
let memoryModalReturnFocus = null;
let confirmReturnFocus = null;
let confirmResolve = null;
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
let paletteItems = [];
let paletteActive = -1;
let paletteTimer = null;
let paletteSequence = 0;
let paletteReturnFocus = null;
let pendingDetailTaskId = 0;
let heatmapRequested = false;
let capabilityViews = [];
let capabilityPreview = null;
let capabilityRequest = 0;
let capabilityFormRevision = 0;
let capabilityPreviewRequest = 0;
let capabilityTestRequest = 0;
let capabilityAuditRequest = 0;
let capabilitySaveInFlight = false;
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

function setSidebarWidth(width, persist = true) {
  sidebarWidth = clampSidebarWidth(width, window.innerWidth);
  const maximum = sidebarMaximumWidth(window.innerWidth);
  elements.app.style.setProperty("--sidebar-width", `${sidebarWidth}px`);
  elements.sidebarResize.setAttribute("aria-valuemax", String(maximum));
  elements.sidebarResize.setAttribute("aria-valuenow", String(sidebarWidth));
  if (persist) writeLayoutPreference(sidebarWidthStorageKey, String(sidebarWidth));
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
  }
}

function beginSidebarResize(event) {
  if (sidebarHidden || event.button !== 0) return;
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
  };
  sidebarDragCleanup = cleanup;
  elements.sidebarResize.setPointerCapture(pointerID);
  elements.sidebarResize.addEventListener("pointermove", move);
  elements.sidebarResize.addEventListener("pointerup", finish);
  elements.sidebarResize.addEventListener("pointercancel", finish);
  elements.sidebarResize.addEventListener("lostpointercapture", finish);
}

function resizeSidebarFromKeyboard(event) {
  if (sidebarHidden) return;
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
    board.summary || "No rolling handoff yet. Agents can update it with ptrack summary set.";
  elements.stats.replaceChildren(
    statElement(board.stats.tasksOpen, "Open tasks"),
    statElement(board.stats.tasksBlocked, "Blocked"),
    statElement(board.stats.notes, "Notes"),
    statElement(board.stats.commits, "Commits"),
    statElement(board.stats.openIssues, "Open issues"),
    statElement(`${board.stats.planTasksDone}/${board.stats.planTasks}`, "Plan done"),
  );
  renderPlanRing(board.stats.planTasksDone, board.stats.planTasks);

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
    `Active plan progress: ${done} of ${total} tasks done`,
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
  // Same selection path the topbar picker used: a snapshot for that plan.
  void loadSnapshot(planId);
}

function renderPlanList() {
  elements.planList.replaceChildren();
  elements.planTotal.textContent = board.plans.length;
  board.plans.forEach((plan) => {
    const item = document.createElement("button");
    item.type = "button";
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
    item.addEventListener("click", () => selectPlan(plan.id));
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
  return (
    !elements.modal.hidden ||
    !elements.memoryModal.hidden ||
    !elements.confirmModal.hidden ||
    !elements.agentLaunchModal.hidden ||
    !elements.terminalAssociationModal.hidden ||
    !elements.terminalWritebackModal.hidden ||
    !elements.taskTransitionModal.hidden ||
    !elements.drawer.hidden ||
    !elements.palette.hidden ||
    Boolean(
      document.querySelector(
        "#terminal-paste-modal:not([hidden]), #terminal-context-menu:not([hidden])",
      ),
    )
  );
}

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
      elements.taskTitle.value.trim().length > 0)
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
    renderIntelligence();
    openPendingTaskDetail();
    if (view === "overview" && heatmapRequested) void loadHeatmap(true);
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
  elements.taskTransitionModal.hidden = true;
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
  editingTask = null;
  elements.modal.hidden = true;
}

function openMemoryHistory() {
  memoryModalReturnFocus = document.activeElement;
  elements.memoryModal.hidden = false;
  requestAnimationFrame(() => elements.memoryDialogClose.focus());
}

function closeMemoryHistory() {
  elements.memoryModal.hidden = true;
  memoryModalReturnFocus?.focus();
  memoryModalReturnFocus = null;
}

const paletteKindLabels = {
  plan: "Plan",
  task: "Task",
  note: "Note",
};

function openPalette() {
  if (workspaceController.state.status !== "open") return;
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
  elements.palette.hidden = true;
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
  elements.drawer.hidden = true;
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
  elements.agentLaunchModal.hidden = true;
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
  elements.terminalAssociationModal.hidden = true;
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
  elements.terminalWritebackModal.hidden = true;
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

function showWorkspaceConfirmation(action, resources) {
  confirmReturnFocus = document.activeElement;
  const copy = confirmationCopy(
    action,
    resources.terminals,
    resources.agentRuns,
    resources.pendingAdmissions || 0,
  );
  elements.confirmHeading.textContent = copy.heading;
  elements.confirmDetail.textContent = copy.detail;
  elements.confirmSubmit.textContent = copy.submit;
  elements.confirmModal.hidden = false;
  requestAnimationFrame(() => elements.confirmCancel.focus());
  return new Promise((resolve) => {
    confirmResolve = resolve;
  });
}

function finishWorkspaceConfirmation(confirmed) {
  if (!confirmResolve) return;
  const resolve = confirmResolve;
  confirmResolve = null;
  elements.confirmModal.hidden = true;
  confirmReturnFocus?.focus();
  confirmReturnFocus = null;
  resolve(confirmed);
}

function renderRecentProjects(projects) {
  elements.recents.replaceChildren();
  const available = projects.filter((project) => project.available);
  if (available.length === 0) {
    elements.recents.append(emptyMemory("No available recent projects."));
    return;
  }
  available.forEach((project) => {
    const item = document.createElement("article");
    item.className = "recent-project";
    const content = document.createElement("div");
    const name = document.createElement("p");
    name.className = "recent-project-name";
    name.textContent = project.name;
    const path = document.createElement("p");
    path.className = "recent-project-path";
    path.textContent = `${project.path} · ${relativeTime(project.lastSeen)}`;
    content.append(name, path);
    const open = document.createElement("button");
    open.type = "button";
    open.className = "button-secondary";
    open.textContent = "Open";
    open.addEventListener("click", () => void requestOpenProject(project.path));
    item.append(content, open);
    elements.recents.append(item);
  });
}

async function loadRecentProjects() {
  try {
    renderRecentProjects(await api().GetRecentProjects());
  } catch (error) {
    elements.recents.replaceChildren(emptyMemory("Recent projects are unavailable."));
    showError(error);
  }
}

function numberValue(element, fallback = 0) {
  const value = Number(element.value);
  return Number.isFinite(value) ? value : fallback;
}

function capabilityDraftFromForm() {
  const kind = elements.capabilityKind.value;
  const draft = {
    id: numberValue(elements.capabilityID),
    model_version: 1,
    name: elements.capabilityName.value.trim(),
    kind,
    agent_profile: elements.capabilityProfile.value.trim(),
    approval_duration_seconds: Math.round(
      numberValue(elements.capabilityDuration, 60) * 60,
    ),
    limits: {
      timeout_seconds: numberValue(elements.capabilityTimeout, 30),
      max_request_bytes: numberValue(elements.capabilityRequestLimit, 1048576),
      max_response_bytes: numberValue(
        elements.capabilityResponseLimit,
        4194304,
      ),
      max_output_bytes: numberValue(elements.capabilityOutputLimit, 1048576),
      max_redirects: numberValue(elements.capabilityRedirects, 3),
      max_concurrent: numberValue(elements.capabilityConcurrency, 1),
    },
    audit: {
      enabled: elements.capabilityAudit.checked,
      retain_last: numberValue(elements.capabilityAuditRetain, 100),
    },
  };
  if (kind === "http") {
    draft.http = {
      base_url: elements.capabilityHTTPURL.value.trim(),
      methods: splitCapabilityList(elements.capabilityHTTPMethods.value),
      path_prefixes: splitCapabilityList(elements.capabilityHTTPPaths.value),
    };
  } else if (kind === "git") {
    draft.git = {
      remote_name: elements.capabilityGitName.value.trim(),
      remote_url: elements.capabilityGitURL.value.trim(),
      operations: splitCapabilityList(elements.capabilityGitOperations.value),
      branches: splitCapabilityList(elements.capabilityGitBranches.value),
      refspecs: splitCapabilityList(elements.capabilityGitRefspecs.value),
      allow_tags: elements.capabilityGitTags.checked,
      allow_force_push: elements.capabilityGitForce.checked,
      allow_delete_refs: elements.capabilityGitDelete.checked,
    };
  } else if (kind === "ssh") {
    draft.ssh = {
      alias: elements.capabilitySSHAlias.value.trim(),
      host: elements.capabilitySSHHost.value.trim(),
      port: numberValue(elements.capabilitySSHPort, 22),
      user: elements.capabilitySSHUser.value.trim(),
      host_key: elements.capabilitySSHKey.value.trim(),
      allow_git: elements.capabilitySSHGit.checked,
      remote_commands: splitCapabilityList(elements.capabilitySSHCommands.value),
      allow_upload: elements.capabilitySSHUpload.checked,
      allow_download: elements.capabilitySSHDownload.checked,
      upload_roots: splitCapabilityList(elements.capabilitySSHUploadLocal.value),
      upload_remote_roots: splitCapabilityList(
        elements.capabilitySSHUploadRemote.value,
      ),
      download_roots: splitCapabilityList(
        elements.capabilitySSHDownloadLocal.value,
      ),
      download_remote_roots: splitCapabilityList(
        elements.capabilitySSHDownloadRemote.value,
      ),
      allow_interactive_shell: elements.capabilitySSHShell.checked,
      local_forward_targets: splitCapabilityList(
        elements.capabilitySSHLocalForward.value,
      ),
      remote_forward_targets: splitCapabilityList(
        elements.capabilitySSHRemoteForward.value,
      ),
    };
  }
  return draft;
}

function syncCapabilityScopeFields() {
  const kind = elements.capabilityKind.value;
  elements.capabilityHTTPFields.hidden = kind !== "http";
  elements.capabilityGitFields.hidden = kind !== "git";
  elements.capabilitySSHFields.hidden = kind !== "ssh";
}

function invalidateCapabilityForm() {
  capabilityFormRevision += 1;
  invalidateCapabilityResponses();
  capabilityPreview = null;
  elements.capabilityPreviewResult.hidden = true;
  elements.capabilityAuditList.hidden = true;
}

function invalidateCapabilityResponses() {
  capabilityPreviewRequest += 1;
  capabilityTestRequest += 1;
  capabilityAuditRequest += 1;
}

function resetCapabilityForm() {
  invalidateCapabilityForm();
  elements.capabilityForm.reset();
  elements.capabilityID.value = "0";
  elements.capabilityEditorTitle.textContent = "New capability";
  elements.capabilityPreviewResult.hidden = true;
  elements.capabilityAuditList.hidden = true;
  capabilityPreview = null;
  syncCapabilityScopeFields();
  elements.capabilityName.focus();
}

function setInputValue(element, value) {
  element.value = value ?? "";
}

function fillCapabilityForm(view) {
  invalidateCapabilityForm();
  const capability = view.capability;
  const limits = capability.limits || {};
  const audit = capability.audit || {};
  setInputValue(elements.capabilityID, capability.id);
  setInputValue(elements.capabilityName, capability.name);
  setInputValue(elements.capabilityProfile, capability.agent_profile);
  setInputValue(elements.capabilityKind, capability.kind);
  setInputValue(
    elements.capabilityDuration,
    Math.max(1, Math.round(capability.approval_duration_seconds / 60)),
  );
  setInputValue(elements.capabilityTimeout, limits.timeout_seconds);
  setInputValue(elements.capabilityRequestLimit, limits.max_request_bytes);
  setInputValue(elements.capabilityResponseLimit, limits.max_response_bytes);
  setInputValue(elements.capabilityOutputLimit, limits.max_output_bytes);
  setInputValue(elements.capabilityRedirects, limits.max_redirects);
  setInputValue(elements.capabilityConcurrency, limits.max_concurrent);
  elements.capabilityAudit.checked = Boolean(audit.enabled);
  setInputValue(elements.capabilityAuditRetain, audit.retain_last);

  const http = capability.http || {};
  setInputValue(elements.capabilityHTTPURL, http.base_url);
  setInputValue(elements.capabilityHTTPMethods, (http.methods || []).join(", "));
  setInputValue(elements.capabilityHTTPPaths, (http.path_prefixes || []).join("\n"));

  const git = capability.git || {};
  setInputValue(elements.capabilityGitName, git.remote_name);
  setInputValue(elements.capabilityGitURL, git.remote_url);
  setInputValue(elements.capabilityGitSSHID, 0);
  setInputValue(elements.capabilityGitOperations, (git.operations || []).join(", "));
  setInputValue(elements.capabilityGitBranches, (git.branches || []).join(", "));
  setInputValue(elements.capabilityGitRefspecs, (git.refspecs || []).join("\n"));
  elements.capabilityGitTags.checked = Boolean(git.allow_tags);
  elements.capabilityGitForce.checked = Boolean(git.allow_force_push);
  elements.capabilityGitDelete.checked = Boolean(git.allow_delete_refs);

  const ssh = capability.ssh || {};
  setInputValue(elements.capabilitySSHAlias, ssh.alias);
  setInputValue(elements.capabilitySSHHost, ssh.host);
  setInputValue(elements.capabilitySSHPort, ssh.port || 22);
  setInputValue(elements.capabilitySSHUser, ssh.user);
  setInputValue(elements.capabilitySSHKey, ssh.host_key);
  setInputValue(elements.capabilitySSHCommands, (ssh.remote_commands || []).join("\n"));
  elements.capabilitySSHGit.checked = Boolean(ssh.allow_git);
  elements.capabilitySSHUpload.checked = Boolean(ssh.allow_upload);
  elements.capabilitySSHDownload.checked = Boolean(ssh.allow_download);
  elements.capabilitySSHShell.checked = Boolean(ssh.allow_interactive_shell);
  setInputValue(elements.capabilitySSHUploadLocal, (ssh.upload_roots || []).join(", "));
  setInputValue(
    elements.capabilitySSHUploadRemote,
    (ssh.upload_remote_roots || []).join(", "),
  );
  setInputValue(
    elements.capabilitySSHDownloadLocal,
    (ssh.download_roots || []).join(", "),
  );
  setInputValue(
    elements.capabilitySSHDownloadRemote,
    (ssh.download_remote_roots || []).join(", "),
  );
  setInputValue(
    elements.capabilitySSHLocalForward,
    (ssh.local_forward_targets || []).join(", "),
  );
  setInputValue(
    elements.capabilitySSHRemoteForward,
    (ssh.remote_forward_targets || []).join(", "),
  );

  elements.capabilityEditorTitle.textContent = `Edit capability #${capability.id}`;
  elements.capabilityAuditList.hidden = true;
  syncCapabilityScopeFields();
  showCapabilityPreview(view);
  elements.capabilityName.focus();
}

function showCapabilityPreview(view, diagnostic = null) {
  capabilityPreview = view;
  elements.capabilityPreviewResult.hidden = false;
  elements.capabilityEffectiveScope.textContent =
    view?.effective_scope || "No effective scope available.";
  const grants = capabilityRiskGrants(view?.capability);
  elements.capabilityRiskSummary.textContent = grants.length
    ? `High-risk grants: ${grants.join(", ")}.`
    : "No write or interactive grants are in this scope.";
  elements.capabilityDiagnostic.textContent = diagnosticLabel(diagnostic);
}

function capabilityEmpty(message) {
  const empty = document.createElement("div");
  empty.className = "capability-empty";
  empty.textContent = message;
  return empty;
}

function capabilityButton(label, action, disabled = false) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "button-secondary";
  button.textContent = label;
  button.disabled = disabled;
  button.addEventListener("click", action);
  return button;
}

async function runCapabilityAction(operation, progress, failed) {
  if (workspaceController.state.status !== "open") return null;
  invalidateCapabilityResponses();
  const ticket = workspaceController.capture();
  setStatus(progress);
  try {
    const result = await operation(ticket.generation);
    if (!workspaceController.accepts(ticket, Number(result?.generation || 0))) {
      return null;
    }
    await loadCapabilities();
    setStatus("Capability settings ready");
    return result;
  } catch (error) {
    if (ticket.epoch === workspaceController.capture().epoch) {
      showError(error);
      setStatus(failed);
    }
    return null;
  }
}

async function enableCapability(view) {
  const digest = view.capability.scope_digest;
  if (!canEnableCapability(view, digest)) return;
  const confirmed = window.confirm(
    `Enable this exact scope for ${view.capability.agent_profile}?\n\n${view.effective_scope}`,
  );
  if (!confirmed) return;
  await runCapabilityAction(
    (generation) =>
      api().EnableCapabilityV2(generation, Number(view.capability.id), digest),
    `Enabling capability #${view.capability.id}…`,
    `Could not enable capability #${view.capability.id}`,
  );
}

async function loadCapabilityAudits(view) {
  if (workspaceController.state.status !== "open") return;
  const request = ++capabilityAuditRequest;
  const ticket = workspaceController.capture();
  try {
    const result = await api().GetCapabilityAuditsV2(
      ticket.generation,
      Number(view.capability.id),
      25,
    );
    if (
      request !== capabilityAuditRequest ||
      !workspaceController.accepts(ticket, Number(result.generation))
    ) return;
    elements.capabilityAuditList.replaceChildren();
    const heading = document.createElement("p");
    heading.className = "section-label";
    heading.textContent = `Recent audit metadata · #${view.capability.id}`;
    elements.capabilityAuditList.append(heading);
    if (!result.audits?.length) {
      elements.capabilityAuditList.append(capabilityEmpty("No audit records."));
    } else {
      result.audits.forEach((audit) => {
        const item = document.createElement("div");
        item.className = "capability-audit-item";
        const outcome = audit.success ? "allowed" : `denied · ${audit.error_class}`;
        item.textContent = `${audit.operation} · ${audit.target} · ${outcome} · ${audit.duration_millis} ms · ${relativeTime(audit.created_at)}`;
        elements.capabilityAuditList.append(item);
      });
    }
    elements.capabilityAuditList.hidden = false;
  } catch (error) {
    if (
      request === capabilityAuditRequest &&
      ticket.epoch === workspaceController.capture().epoch
    ) showError(error);
  }
}

async function testCapability() {
  const draft = capabilityDraftFromForm();
  const sshID = numberValue(elements.capabilityGitSSHID);
  if (gitCapabilityNeedsSSH(draft) && sshID <= 0) {
    setStatus("Select a separate approved SSH capability ID before testing Git over SSH");
    elements.capabilityGitSSHID.focus();
    return;
  }
  if (workspaceController.state.status !== "open") return;
  const request = ++capabilityTestRequest;
  const revision = capabilityFormRevision;
  const ticket = workspaceController.capture();
  setStatus("Testing connection without changing remote state…");
  try {
    const result = await api().TestCapabilityV2(ticket.generation, draft, sshID);
    if (
      !capabilityResponseIsCurrent(
        request, capabilityTestRequest, revision, capabilityFormRevision,
      ) || !workspaceController.accepts(ticket, Number(result.generation))
    ) return;
    showCapabilityPreview(capabilityPreview || { capability: draft }, result.diagnostic);
    setStatus(diagnosticLabel(result.diagnostic));
  } catch (error) {
    if (
      capabilityResponseIsCurrent(
        request, capabilityTestRequest, revision, capabilityFormRevision,
      ) && ticket.epoch === workspaceController.capture().epoch
    ) {
      showError(error);
      setStatus("Connection test failed");
    }
  }
}

function renderCapabilities() {
  elements.capabilityTotal.textContent = String(capabilityViews.length);
  elements.capabilityList.replaceChildren();
  if (!capabilityViews.length) {
    elements.capabilityList.append(
      capabilityEmpty("No capabilities. Broker tools are denied by default."),
    );
    return;
  }
  capabilityViews.forEach((view) => {
    const capability = view.capability;
    const card = document.createElement("article");
    card.className = "capability-card";
    const heading = document.createElement("div");
    heading.className = "capability-card-heading";
    const title = document.createElement("div");
    const name = document.createElement("h3");
    name.textContent = capability.name;
    const metadata = document.createElement("p");
    metadata.className = "intelligence-meta";
    metadata.textContent = `${capability.kind.toUpperCase()} · ${capability.agent_profile} · revision ${capability.revision}`;
    title.append(name, metadata);
    const state = document.createElement("span");
    state.className = "capability-state";
    state.dataset.state = view.state;
    state.textContent = capabilityStateLabel(view.state);
    heading.append(title, state);

    const scope = document.createElement("code");
    scope.className = "capability-card-scope";
    scope.textContent = view.effective_scope || view.error || "Invalid scope";
    const grants = capabilityRiskGrants(capability);
    const risk = document.createElement("p");
    risk.className = grants.length ? "settings-warning" : "intelligence-meta";
    risk.textContent = grants.length
      ? `High-risk grants: ${grants.join(", ")}`
      : "Read-only or connection-only scope";

    const actions = document.createElement("div");
    actions.className = "capability-card-actions";
    actions.append(
      capabilityButton("Edit", () => fillCapabilityForm(view)),
      capabilityButton("Test", () => {
        fillCapabilityForm(view);
        if (gitCapabilityNeedsSSH(capability)) {
          setStatus("Select a separate approved SSH capability ID before testing Git over SSH");
          elements.capabilityGitSSHID.focus();
          return;
        }
        void testCapability();
      }, view.state === "invalid"),
    );
    if (view.state === "enabled") {
      actions.append(
        capabilityButton("Disable", () =>
          void runCapabilityAction(
            (generation) =>
              api().DisableCapabilityV2(generation, Number(capability.id)),
            `Disabling capability #${capability.id}…`,
            `Could not disable capability #${capability.id}`,
          ),
        ),
        capabilityButton("Expire now", () =>
          void runCapabilityAction(
            (generation) =>
              api().ExpireCapabilityV2(generation, Number(capability.id)),
            `Expiring capability #${capability.id}…`,
            `Could not expire capability #${capability.id}`,
          ),
        ),
      );
    } else {
      actions.append(
        capabilityButton(
          "Review and enable",
          () => void enableCapability(view),
          !canEnableCapability(view, capability.scope_digest),
        ),
      );
    }
    actions.append(
      capabilityButton("Audit", () => void loadCapabilityAudits(view)),
      capabilityButton("Remove", async () => {
        if (!window.confirm(`Remove capability “${capability.name}”?`)) return;
        await runCapabilityAction(
          (generation) =>
            api().RemoveCapabilityV2(generation, Number(capability.id)),
          `Removing capability #${capability.id}…`,
          `Could not remove capability #${capability.id}`,
        );
      }),
    );
    card.append(heading, scope, risk, actions);
    elements.capabilityList.append(card);
  });
}

async function loadCapabilities() {
  if (workspaceController.state.status !== "open") return;
  const request = ++capabilityRequest;
  const ticket = workspaceController.capture();
  try {
    const result = await api().GetCapabilitiesV2(ticket.generation);
    if (
      request !== capabilityRequest ||
      !workspaceController.accepts(ticket, Number(result.generation))
    ) return;
    capabilityViews = result.capabilities || [];
    renderCapabilities();
  } catch (error) {
    if (
      request === capabilityRequest &&
      ticket.epoch === workspaceController.capture().epoch
    ) {
      elements.capabilityList.replaceChildren(
        capabilityEmpty("Capabilities could not be loaded."),
      );
      showError(error);
    }
  }
}

async function previewCapability() {
  if (workspaceController.state.status !== "open") return;
  const request = ++capabilityPreviewRequest;
  const revision = capabilityFormRevision;
  const ticket = workspaceController.capture();
  try {
    const result = await api().PreviewCapabilityV2(
      ticket.generation,
      capabilityDraftFromForm(),
    );
    if (
      !capabilityResponseIsCurrent(
        request, capabilityPreviewRequest, revision, capabilityFormRevision,
      ) || !workspaceController.accepts(ticket, Number(result.generation))
    ) return;
    showCapabilityPreview(result.view);
  } catch (error) {
    if (
      capabilityResponseIsCurrent(
        request, capabilityPreviewRequest, revision, capabilityFormRevision,
      ) && ticket.epoch === workspaceController.capture().epoch
    ) showError(error);
  }
}

async function saveCapability() {
  if (!canStartCapabilitySave(capabilitySaveInFlight)) return;
  capabilitySaveInFlight = true;
  elements.capabilitySaveButton.disabled = true;
  const revision = capabilityFormRevision;
  const draft = capabilityDraftFromForm();
  try {
    const result = await runCapabilityAction(
      (generation) => api().SaveCapabilityV2(generation, draft),
      "Saving disabled capability draft…",
      "Could not save capability",
    );
    if (result?.view && revision === capabilityFormRevision) {
      fillCapabilityForm(result.view);
    }
  } finally {
    capabilitySaveInFlight = false;
    elements.capabilitySaveButton.disabled = false;
  }
}

function applyView() {
  const open = workspaceState.status === "open";
  elements.workspace.hidden = !open || view !== "board";
  elements.overviewPage.hidden = !open || view !== "overview";
  elements.settingsPage.hidden = !open || view !== "settings";
  elements.navBoard.classList.toggle("active", view === "board");
  elements.navOverview.classList.toggle("active", view === "overview");
  elements.navSettings.classList.toggle("active", view === "settings");
  if (view === "board") elements.navBoard.setAttribute("aria-current", "page");
  else elements.navBoard.removeAttribute("aria-current");
  if (view === "overview") elements.navOverview.setAttribute("aria-current", "page");
  else elements.navOverview.removeAttribute("aria-current");
  if (view === "settings") elements.navSettings.setAttribute("aria-current", "page");
  else elements.navSettings.removeAttribute("aria-current");
  terminalHandle?.setVisible(open && view === "board");
}

function setView(nextView) {
  view = ["overview", "settings"].includes(nextView) ? nextView : "board";
  applyView();
  if (view === "overview") {
    requestAnimationFrame(fitRecentMemory);
    void loadHeatmap();
  }
  if (view === "settings") void loadCapabilities();
}

function renderWorkspaceState(state, focus = false) {
  const wasOpen = workspaceState.status === "open";
  workspaceState = state;
  if (typeof state.version === "string") {
    const version = appVersionLabel(state.version);
    elements.appVersion.textContent = version;
    elements.appVersion.setAttribute("aria-label", `p-track version ${version}`);
  }
  const open = state.status === "open";
  if (open && !wasOpen) view = "board";
  applyView();
  elements.stateScreen.hidden = open;
  elements.navBoard.disabled = !open;
  elements.navOverview.disabled = !open;
  elements.navSettings.disabled = !open;
  elements.switchProject.hidden = !open;
  elements.closeProject.hidden = !open;
  elements.openProject.hidden = open;
  elements.workspace.removeAttribute("aria-busy");
  elements.workspace.inert = false;
  elements.overviewPage.removeAttribute("aria-busy");
  elements.overviewPage.inert = false;
  elements.settingsPage.removeAttribute("aria-busy");
  elements.settingsPage.inert = false;
  elements.switchProject.disabled = false;
  elements.closeProject.disabled = false;

  if (open) {
    elements.projectName.textContent = state.project?.name || "Project workspace";
    if (!wasOpen) {
      elements.planTotal.textContent = "0";
      elements.planList.replaceChildren(emptyMemory("Loading plans…"));
    }
    void loadRecentProjects();
    void ensureTerminalDock(state.generation, state.project.root);
    if (focus) requestAnimationFrame(() => elements.projectName.focus());
    return;
  }

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
  capabilityRequest += 1;
  capabilityFormRevision += 1;
  capabilityPreviewRequest += 1;
  capabilityTestRequest += 1;
  capabilityAuditRequest += 1;
  capabilityViews = [];
  capabilityPreview = null;
  renderCapabilities();
  elements.capabilityPreviewResult.hidden = true;
  elements.capabilityAuditList.hidden = true;
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
  if (state.status !== "loading") void loadRecentProjects();
  if (focus) {
    requestAnimationFrame(() => {
      if (!elements.stateOpen.hidden) elements.stateOpen.focus();
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
    elements.settingsPage.inert = true;
    elements.settingsPage.setAttribute("aria-busy", "true");
  }
  if (state.status === "open" && !keepInert) void loadSnapshot(0);
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
    elements.settingsPage.inert = true;
    elements.settingsPage.setAttribute("aria-busy", "true");
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

async function chooseProjectDirectory() {
  const path = await api().PickProjectDirectory();
  return typeof path === "string" ? path : "";
}

async function requestOpenProject(selectedPath = "") {
  try {
    const path = selectedPath || (await chooseProjectDirectory());
    if (!path) return;
    let transition = beginWorkspaceTransition();
    let result = await api().OpenProject(path, "");
    if (result.requiresConfirmation) {
      if (!publishBackendState(result.state, transition, false, true)) return;
      const confirmed = await showWorkspaceConfirmation("switch", result.activeResources);
      if (!confirmed) {
        await api().CancelWorkspaceChange(result.confirmationToken);
        renderWorkspaceState(result.state, true);
        return;
      }
      transition = beginWorkspaceTransition();
      result = await api().OpenProject(path, result.confirmationToken);
    }
    if (!publishBackendState(result.state, transition, true)) return;
    if (result.warning) showError(result.warning);
  } catch (error) {
    await recoverWorkspaceState(error);
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
    });
    terminalHandle = handle;
    handle.setVisible(workspaceState.status === "open" && view === "board");
    await handle.ready;
    const current = workspaceController.state;
    if (
      terminalHandle !== handle ||
      current.generation !== generation ||
      !["open", "loading"].includes(current.status)
    ) {
      handle.dispose();
      if (terminalHandle === handle) terminalHandle = null;
      return;
    }
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
  terminalHandle?.dispose();
  terminalHandle = null;
  terminalGeneration = 0;
  terminalProjectRoot = "";
}

function boardShortcutIsBlocked(event) {
  if (event.isComposing || workspaceController.state.status !== "open") return true;
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

function trapModalFocus(event) {
  if (event.key !== "Tab") return;
  const modal = [
    elements.palette,
    elements.confirmModal,
    elements.agentLaunchModal,
    elements.terminalAssociationModal,
    elements.terminalWritebackModal,
    elements.taskTransitionModal,
    elements.modal,
    elements.memoryModal,
    elements.drawer,
  ].find(
    (candidate) => !candidate.hidden,
  );
  if (!modal) return;
  const focusable = Array.from(
    modal.querySelectorAll(
      'button:not([disabled]), input:not([disabled]):not([hidden]), textarea:not([disabled]):not([hidden]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
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

function eventsOn(name, callback) {
  const runtime = window.runtime;
  if (typeof runtime?.EventsOnMultiple !== "function") return () => {};
  return runtime.EventsOnMultiple(name, callback, -1);
}

function registerNativeProjectActions() {
  nativeEventDisposers.push(
    eventsOn("workspace:open-requested", () => void requestOpenProject()),
    eventsOn("workspace:switch-requested", () => void requestOpenProject()),
    eventsOn("workspace:close-requested", () => void requestCloseProject()),
    eventsOn("workspace:capabilities-requested", () => setView("settings")),
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
elements.navSettings.addEventListener("click", () => setView("settings"));

window.addEventListener("focus", () => {
  if (workspaceController.state.status !== "open") return;
  if (view === "settings") void loadCapabilities();
  else void loadSnapshot(board?.planId || 0, true);
});

elements.capabilityKind.addEventListener("change", syncCapabilityScopeFields);
elements.capabilityForm.addEventListener("input", invalidateCapabilityForm);
elements.capabilityNew.addEventListener("click", resetCapabilityForm);
elements.capabilityClear.addEventListener("click", resetCapabilityForm);
elements.capabilityPreviewButton.addEventListener("click", () =>
  void previewCapability(),
);
elements.capabilityTestButton.addEventListener("click", () =>
  void testCapability(),
);
elements.capabilityForm.addEventListener("submit", (event) => {
  event.preventDefault();
  void saveCapability();
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
elements.themeToggle.addEventListener("click", () => themeController.toggle());

elements.openProject.addEventListener("click", () => void requestOpenProject());
elements.switchProject.addEventListener("click", () => void requestOpenProject());
elements.closeProject.addEventListener("click", () => void requestCloseProject());
elements.stateOpen.addEventListener("click", () => void requestOpenProject());
elements.activityMore.addEventListener("click", openMemoryHistory);
elements.planLaunchAgent.addEventListener("click", (event) => {
  if (!board?.planId) return;
  void openAgentLaunchPicker(
    { planId: Number(board.planId) },
    event.currentTarget,
  );
});
elements.confirmCancel.addEventListener("click", () => finishWorkspaceConfirmation(false));
elements.confirmSubmit.addEventListener("click", () => finishWorkspaceConfirmation(true));

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
  if (event.key === "Escape" && !elements.confirmModal.hidden) {
    event.preventDefault();
    finishWorkspaceConfirmation(false);
    return;
  }
  if (event.key === "Escape" && !elements.agentLaunchModal.hidden) {
    event.preventDefault();
    closeAgentLaunchPicker();
    return;
  }
  if (event.key === "Escape" && !elements.terminalAssociationModal.hidden) {
    event.preventDefault();
    closeTerminalAssociationEditor();
    return;
  }
  if (event.key === "Escape" && !elements.terminalWritebackModal.hidden) {
    event.preventDefault();
    closeTerminalWriteback();
    return;
  }
  if (event.key === "Escape" && !elements.taskTransitionModal.hidden) {
    event.preventDefault();
    closeTaskTransition();
    return;
  }
  if (event.key === "Escape" && !elements.modal.hidden) closeDialog();
  if (event.key === "Escape" && !elements.memoryModal.hidden) closeMemoryHistory();
  if (
    event.key === "Escape" &&
    !elements.drawer.hidden &&
    elements.modal.hidden &&
    elements.memoryModal.hidden
  ) {
    closeTaskDetail();
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
  if (command && !boardShortcutIsBlocked(event)) {
    event.preventDefault();
    if (command === "board") setView("board");
    if (command === "overview") setView("overview");
    if (command === "settings") setView("settings");
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
  refreshLoop.dispose();
  runtimeRefreshes.cancel();
  disposeTerminalDock();
  nativeEventDisposers.splice(0).forEach((dispose) => dispose());
});

async function start() {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      api();
      const state = await api().GetWorkspaceState();
      workspaceController.publish({
        status: state.status,
        generation: Number(state.generation || 0),
      });
      renderWorkspaceState(state);
      registerNativeProjectActions();
      refreshLoop.start();
      if (state.status === "open") await loadSnapshot(0);
      return;
    } catch {
      await new Promise((resolve) => window.setTimeout(resolve, 100));
    }
  }
  workspaceController.publish({ status: "error", generation: 0 });
  renderWorkspaceState(
    {
      status: "error",
      generation: 0,
      error: "Could not connect to the Wails backend.",
    },
    true,
  );
}

void start();
