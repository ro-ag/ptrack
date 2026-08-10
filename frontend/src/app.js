import { mountTerminalDock } from "./terminal/pane";
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
  WorkspaceController,
} from "./workspace/controller";
import {
  collapsedLaneStatuses,
  commandShortcut,
  confirmationCopy,
  focusCycleIndex,
  groupSearchResults,
  heatmapWeeks,
  paletteTarget,
  preserveSectionOnError,
  shortcutIntent,
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
  projectName: document.querySelector("#project-name"),
  planTitle: document.querySelector("#plan-title"),
  planTotal: document.querySelector("#plan-total"),
  planList: document.querySelector("#sidebar-plan-list"),
  planProgress: document.querySelector("#plan-progress"),
  planProgressLabel: document.querySelector("#plan-progress-label"),
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
  runtimeTotal: document.querySelector("#runtime-total"),
  terminalSessions: document.querySelector("#terminal-sessions"),
  agentRuns: document.querySelector("#agent-runs"),
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
  drawerNotes: document.querySelector("#drawer-notes"),
  drawerNotesCount: document.querySelector("#drawer-notes-count"),
  drawerCommits: document.querySelector("#drawer-commits"),
  drawerCommitsCount: document.querySelector("#drawer-commits-count"),
  drawerIssues: document.querySelector("#drawer-issues"),
  drawerIssuesCount: document.querySelector("#drawer-issues-count"),
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
let detailTask = null;
let detailRequest = 0;
let drawerReturnFocus = null;
let drawerOpenTimer = null;
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
  statusSelect.addEventListener("change", () => void moveTask(task.id, statusSelect.value));
  actions.append(
    statusSelect,
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
      void moveTask(draggedTask.id, column.status);
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
        `Note · ${note.target}${note.targetId ? ` #${note.targetId}` : ""}`,
        `${relativeTime(note.occurredAt)} · ${note.body}`,
      ),
    );
  });

  renderGitIntelligence(snapshot.git);
  renderRuntimeIntelligence(snapshot.terminals, snapshot.agentRuns);
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

function renderRuntimeIntelligence(terminals, agents) {
  elements.terminalSessions.replaceChildren();
  elements.agentRuns.replaceChildren();
  const sessions = terminals.sessions || [];
  const runs = agents.runs || [];
  const activeRuns = runs.filter((run) => ["running", "unknown", "stale"].includes(run.state));
  elements.runtimeTotal.textContent = sessions.length + activeRuns.length;
  if (sessions.length === 0) {
    elements.terminalSessions.append(emptyMemory("No terminal sessions."));
  } else {
    sessions.forEach((session) => {
      elements.terminalSessions.append(
        intelligenceItem(
          `Terminal · ${session.profileId}`,
          `${session.state} · PID ${session.pid || "unknown"} · ${relativeTime(session.lastActivityAt)} · ${session.cwd}`,
          session.state === "failed" ? "error" : "",
        ),
      );
    });
  }
  if (runs.length === 0) {
    elements.agentRuns.append(emptyMemory("No registered agent runs."));
  } else {
    runs.forEach((run) => {
      const association = [
        run.planId ? `plan #${run.planId}` : "",
        run.taskId ? `task #${run.taskId}` : "",
        run.terminalId ? `terminal ${run.terminalId.slice(0, 8)}` : "",
      ].filter(Boolean);
      elements.agentRuns.append(
        intelligenceItem(
          `Agent · ${run.profile} · ${run.provider}`,
          `${run.state} · process ${run.processState} · lease ${run.leaseState} · PID ${run.pid || "unknown"} · ${association.join(" · ") || "project"} · ${relativeTime(run.lastActivityAt)}`,
          ["stale", "unknown"].includes(run.state) ? "stale" : run.state === "exited" ? "" : "",
        ),
      );
    });
  }
}

function snapshotDialogIsOpen() {
  return (
    !elements.modal.hidden ||
    !elements.memoryModal.hidden ||
    !elements.confirmModal.hidden ||
    !elements.drawer.hidden ||
    !elements.palette.hidden ||
    Boolean(
      document.querySelector(
        "#terminal-paste-modal:not([hidden]), #terminal-context-menu:not([hidden])",
      ),
    )
  );
}

async function loadSnapshot(planId = board?.planId || 0, quiet = false) {
  if (workspaceController.state.status !== "open") return;
  if (!refreshGate.tryBegin(!quiet)) {
    if (!quiet) queuedSnapshotPlanId = Number(planId);
    return;
  }
  if (
    quiet &&
    (snapshotDialogIsOpen() ||
      draggedTask ||
      elements.taskTitle.value.trim().length > 0)
  ) {
    refreshGate.finish();
    return;
  }

  const ticket = workspaceController.capture();
  const request = ++snapshotSequence;
  activeSnapshotRequest = request;
  if (!quiet) setStatus("Refreshing project snapshot…");
  try {
    const response = await api().GetWorkspaceSnapshot(ticket.generation, Number(planId));
    if (request !== snapshotSequence || !workspaceController.accepts(ticket, response.generation)) {
      return;
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
      queuedSnapshotPlanId = 0;
      queueMicrotask(() => void loadSnapshot(queuedPlan));
    }
  }
}

async function runMutation(operation, progress, failed) {
  if (!board || workspaceController.state.status !== "open") return;
  const ticket = workspaceController.capture();
  setStatus(progress);
  try {
    const result = await operation(ticket.generation);
    if (result?.generation && !workspaceController.accepts(ticket, result.generation)) return;
    await loadSnapshot(board.planId);
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
    }
  }
}

async function moveTask(taskId, status) {
  await runMutation(
    (generation) => api().MoveTaskV2(generation, Number(taskId), status),
    `Moving task #${taskId}…`,
    `Could not move task #${taskId}`,
  );
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
}

function renderDrawerLoading() {
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
  meta.textContent = relativeTime(note.occurredAt);
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
  const modal = [elements.palette, elements.confirmModal, elements.modal, elements.memoryModal, elements.drawer].find(
    (candidate) => !candidate.hidden,
  );
  if (!modal) return;
  const focusable = Array.from(
    modal.querySelectorAll(
      'button:not([disabled]), input:not([disabled]):not([hidden]), textarea:not([disabled]):not([hidden]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
    ),
  ).filter((item) => !item.hidden);
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
elements.drawerStatusSelect.addEventListener("change", () => {
  if (!detailTask) return;
  void moveTask(detailTask.id, elements.drawerStatusSelect.value);
});
elements.drawerRename.addEventListener("click", () => {
  if (detailTask) openRename(detailTask);
});
elements.drawerMemory.addEventListener("click", () => {
  if (detailTask) openMemory(detailTask);
});

document.addEventListener("keydown", (event) => {
  trapModalFocus(event);
  if (event.key === "Escape" && !elements.confirmModal.hidden) {
    event.preventDefault();
    finishWorkspaceConfirmation(false);
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
