import { mountTerminalDock } from "./terminal/pane";
import {
  RefreshGate,
  RefreshLoop,
  WorkspaceController,
} from "./workspace/controller";
import {
  confirmationCopy,
  focusCycleIndex,
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
  medium: "#5fafff",
  high: "var(--doing)",
  critical: "var(--blocked)",
};

const elements = {
  workspace: document.querySelector("#workspace"),
  stateScreen: document.querySelector("#workspace-state-screen"),
  stateEyebrow: document.querySelector("#workspace-state-eyebrow"),
  stateHeading: document.querySelector("#workspace-state-heading"),
  stateDetail: document.querySelector("#workspace-state-detail"),
  stateOpen: document.querySelector("#state-open-project-button"),
  recents: document.querySelector("#recent-project-list"),
  board: document.querySelector("#board"),
  projectName: document.querySelector("#project-name"),
  planTitle: document.querySelector("#plan-title"),
  planPicker: document.querySelector("#plan-picker"),
  planSelect: document.querySelector("#plan-select"),
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
  refresh: document.querySelector("#refresh-button"),
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
  toast: document.querySelector("#toast"),
};

const workspaceController = new WorkspaceController();
const refreshGate = new RefreshGate();
const nativeEventDisposers = [];
const refreshLoop = new RefreshLoop(() => {
  void loadSnapshot(board?.planId || 0, true);
}, 15_000);

let workspaceState = { status: "welcome", generation: 0 };
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
let snapshotSequence = 0;
let activeSnapshotRequest = null;
let queuedSnapshotPlanId = 0;

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
  dragZone.setAttribute("aria-label", `Task #${task.id}: ${task.title}. Drag to change status.`);
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
  dragZone.addEventListener("dblclick", () => openRename(task));
  dragZone.addEventListener("dragstart", (event) => {
    draggedTask = task;
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", String(task.id));
    requestAnimationFrame(() => card.classList.add("dragging"));
  });
  dragZone.addEventListener("dragend", () => {
    draggedTask = null;
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

function columnElement(column) {
  const lane = document.createElement("section");
  lane.className = "column";
  lane.dataset.status = column.status;
  lane.style.setProperty("--lane-color", laneColors[column.status]);
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
  header.append(heading, count);
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

function renderBoard() {
  elements.projectName.textContent = board.projectName;
  elements.planTitle.textContent = board.planTitle || "No active plan";
  const selected = String(board.planId);
  elements.planSelect.replaceChildren();
  board.plans.forEach((plan) => {
    const option = document.createElement("option");
    option.value = plan.id;
    option.textContent = `${plan.isActive ? "Active · " : ""}#${plan.id} ${plan.title}`;
    option.selected = String(plan.id) === selected;
    elements.planSelect.append(option);
  });
  elements.planSelect.disabled = board.plans.length === 0;
  const total = board.stats.planTasks;
  const done = board.stats.planTasksDone;
  const percentage = total ? Math.round((done / total) * 100) : 0;
  elements.planProgress.style.width = `${percentage}%`;
  elements.planProgressLabel.textContent = `${done}/${total} done`;
  elements.taskTitle.disabled = board.planId === 0;
  elements.addForm.querySelector("button").disabled = board.planId === 0;
  elements.board.replaceChildren();
  board.columns.forEach((column) => elements.board.append(columnElement(column)));
  renderMemory();
}

function renderIntelligence() {
  const project = snapshot.project;
  const tracking = snapshot.tracking;
  elements.projectRoot.textContent = project.root;
  const storage = project.storage;
  elements.storageStatus.textContent = storage.exists
    ? `P-TRACK format v${storage.formatVersion} · ${compactBytes(storage.sizeBytes)} · writer ${storage.lastWriteVersion || "unknown"}`
    : storage.error || "P-TRACK storage unavailable";
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
  elements.refresh.disabled = true;
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
    if (workspaceController.state.status === "open") elements.refresh.disabled = false;
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
  elements.dialogEyebrow.textContent = "P-TRACK memory";
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

function renderWorkspaceState(state, focus = false) {
  workspaceState = state;
  const open = state.status === "open";
  elements.workspace.hidden = !open;
  elements.stateScreen.hidden = open;
  elements.planPicker.hidden = !open;
  elements.refresh.hidden = !open;
  elements.switchProject.hidden = !open;
  elements.closeProject.hidden = !open;
  elements.openProject.hidden = open;
  elements.workspace.removeAttribute("aria-busy");
  elements.workspace.inert = false;
  elements.refresh.disabled = false;
  elements.switchProject.disabled = false;
  elements.closeProject.disabled = false;

  if (open) {
    elements.projectName.textContent = state.project?.name || "Project workspace";
    void ensureTerminalDock(state.generation);
    if (focus) requestAnimationFrame(() => elements.projectName.focus());
    return;
  }

  snapshotSequence += 1;
  activeSnapshotRequest = null;
  disposeTerminalDock();
  board = null;
  snapshot = null;
  elements.projectName.textContent = "Project workspace";
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
  }
  if (state.status === "open" && !keepInert) void loadSnapshot(0);
  return true;
}

function beginWorkspaceTransition() {
  const transition = workspaceController.beginTransition();
  if (workspaceState.status === "open") {
    elements.workspace.inert = true;
    elements.workspace.setAttribute("aria-busy", "true");
    elements.refresh.disabled = true;
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
    ResizeTerminal(sessionID, rows, columns) {
      return api().ResizeTerminalV2(generation, sessionID, rows, columns);
    },
    CloseTerminal(sessionID, force) {
      return api().CloseTerminalV2(generation, sessionID, force);
    },
  };
}

async function ensureTerminalDock(generation) {
  if (terminalHandle && terminalGeneration === generation) return;
  disposeTerminalDock();
  terminalGeneration = generation;
  try {
    const handle = mountTerminalDock({
      backend: generationTerminalBackend(generation),
      workspaceGeneration: generation,
      showError,
    });
    terminalHandle = handle;
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
  const modal = [elements.confirmModal, elements.modal, elements.memoryModal].find(
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
  );
}

elements.refresh.addEventListener("click", () => void loadSnapshot());
elements.planSelect.addEventListener("change", () =>
  void loadSnapshot(elements.planSelect.value),
);
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

document.addEventListener("keydown", (event) => {
  trapModalFocus(event);
  if (event.key === "Escape" && !elements.confirmModal.hidden) {
    event.preventDefault();
    finishWorkspaceConfirmation(false);
    return;
  }
  if (event.key === "Escape" && !elements.modal.hidden) closeDialog();
  if (event.key === "Escape" && !elements.memoryModal.hidden) closeMemoryHistory();
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
    document.querySelector(".memory-rail"),
  );
}

window.addEventListener("beforeunload", () => {
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
