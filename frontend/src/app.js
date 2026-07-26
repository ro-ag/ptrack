import { mountTerminalDock } from "./terminal/pane";

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
  board: document.querySelector("#board"),
  projectName: document.querySelector("#project-name"),
  planTitle: document.querySelector("#plan-title"),
  planSelect: document.querySelector("#plan-select"),
  planProgress: document.querySelector("#plan-progress"),
  planProgressLabel: document.querySelector("#plan-progress-label"),
  goal: document.querySelector("#goal"),
  summary: document.querySelector("#summary"),
  stats: document.querySelector("#project-stats"),
  issues: document.querySelector("#issue-list"),
  issueTotal: document.querySelector("#issue-total"),
  activity: document.querySelector("#activity-list"),
  activityMore: document.querySelector("#activity-more"),
  memoryModal: document.querySelector("#memory-modal"),
  memoryDialogList: document.querySelector("#memory-dialog-list"),
  memoryDialogClose: document.querySelector("#memory-dialog-close"),
  status: document.querySelector("#status"),
  refresh: document.querySelector("#refresh-button"),
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
  toast: document.querySelector("#toast"),
};

let board = null;
let draggedTask = null;
let editingTask = null;
let dialogMode = "rename";
let toastTimer = null;
let loading = false;
let memoryModalReturnFocus = null;

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
  const days = Math.round(hours / 24);
  return `${days}d ago`;
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
    elements.activity.append(
      emptyMemory("Decisions and linked commits will appear here as the project evolves."),
    );
    elements.memoryDialogList.append(
      emptyMemory("Decisions and linked commits will appear here as the project evolves."),
    );
    elements.activityMore.hidden = true;
  } else {
    board.activity.forEach((activity) => {
      elements.activity.append(activityElement(activity));
      elements.memoryDialogList.append(activityElement(activity, true));
    });
    requestAnimationFrame(fitRecentMemory);
  }
}

function emptyMemory(message) {
  const empty = document.createElement("div");
  empty.className = "memory-empty";
  empty.textContent = message;
  return empty;
}

function contextChip(count, singular, extraClass = "") {
  const chip = document.createElement("span");
  chip.className = `context-chip ${extraClass}`.trim();
  chip.textContent = `${count} ${count === 1 ? singular : `${singular}s`}`;
  return chip;
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
  statusSelect.addEventListener("change", () => moveTask(task.id, statusSelect.value));
  actions.append(
    statusSelect,
    actionButton("Edit", "Rename task", () => openRename(task)),
    actionButton("Memory", "Record a memory note", () => openMemory(task)),
  );

  card.append(dragZone, actions);
  return card;
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
    empty.textContent = "Drop a task here";
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
      moveTask(draggedTask.id, column.status);
    }
  });
  return lane;
}

function render() {
  elements.projectName.textContent = board.projectName;
  elements.planTitle.textContent = board.planTitle;

  const selected = String(board.planId);
  elements.planSelect.replaceChildren();
  board.plans.forEach((plan) => {
    const option = document.createElement("option");
    option.value = plan.id;
    option.textContent = `${plan.isActive ? "Active · " : ""}#${plan.id} ${plan.title}`;
    option.selected = String(plan.id) === selected;
    elements.planSelect.append(option);
  });

  const total = board.stats.planTasks;
  const done = board.stats.planTasksDone;
  const percentage = total ? Math.round((done / total) * 100) : 0;
  elements.planProgress.style.width = `${percentage}%`;
  elements.planProgressLabel.textContent = `${done}/${total} done`;

  elements.board.replaceChildren();
  board.columns.forEach((column) => elements.board.append(columnElement(column)));
  renderMemory();
}

async function loadBoard(planId = board?.planId || 0, quiet = false) {
  if (loading) return;
  if (
    quiet &&
    (!elements.modal.hidden ||
      !elements.memoryModal.hidden ||
      draggedTask ||
      elements.taskTitle.value.trim().length > 0)
  ) {
    return;
  }
  loading = true;
  elements.refresh.disabled = true;
  if (!quiet) setStatus("Refreshing board…");
  try {
    board = await api().GetBoard(Number(planId));
    render();
    const count = board.stats.planTasks;
    const now = new Date().toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
    setStatus(`${count} task${count === 1 ? "" : "s"} · synced ${now}`);
  } catch (error) {
    showError(error);
    setStatus("Refresh failed");
  } finally {
    loading = false;
    elements.refresh.disabled = false;
  }
}

async function moveTask(taskId, status) {
  setStatus(`Moving task #${taskId}…`);
  try {
    await api().MoveTask(Number(taskId), status);
    await loadBoard(board.planId);
  } catch (error) {
    showError(error);
    setStatus(`Could not move task #${taskId}`);
    await loadBoard(board.planId, true);
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

elements.refresh.addEventListener("click", () => loadBoard());
elements.planSelect.addEventListener("change", () => loadBoard(elements.planSelect.value));
elements.activityMore.addEventListener("click", openMemoryHistory);
elements.addForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const title = elements.taskTitle.value.trim();
  if (!title || !board) return;
  setStatus("Adding task…");
  try {
    await api().AddTask(Number(board.planId), title);
    elements.taskTitle.value = "";
    await loadBoard(board.planId);
    elements.taskTitle.focus();
  } catch (error) {
    showError(error);
    setStatus("Could not add task");
  }
});

elements.dialogForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (!editingTask) return;
  try {
    if (dialogMode === "rename") {
      const title = elements.dialogInput.value.trim();
      if (!title) return;
      await api().RenameTask(Number(editingTask.id), title);
    } else {
      const note = elements.dialogNote.value.trim();
      if (!note) return;
      await api().AddTaskNote(Number(editingTask.id), note);
    }
    closeDialog();
    await loadBoard(board.planId);
  } catch (error) {
    showError(error);
  }
});

document.querySelectorAll("[data-close-modal]").forEach((element) => {
  element.addEventListener("click", closeDialog);
});
document.querySelectorAll("[data-close-memory-modal]").forEach((element) => {
  element.addEventListener("click", closeMemoryHistory);
});
elements.memoryDialogClose.addEventListener("click", closeMemoryHistory);

function boardShortcutIsBlocked(event) {
  const active = document.activeElement;
  const terminalInteractionVisible = Boolean(
    document.querySelector(
      "#terminal-paste-modal:not([hidden]), #terminal-context-menu:not([hidden])",
    ),
  );
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
  return interactive || terminalFocused || terminalInteractionVisible;
}

document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !elements.modal.hidden) closeDialog();
  if (event.key === "Escape" && !elements.memoryModal.hidden) closeMemoryHistory();
  if (
    event.key.toLowerCase() === "r" &&
    !event.metaKey &&
    !event.ctrlKey &&
    !event.altKey &&
    !event.shiftKey &&
    !event.repeat &&
    !event.defaultPrevented &&
    elements.modal.hidden &&
    elements.memoryModal.hidden &&
    !boardShortcutIsBlocked(event)
  ) {
    event.preventDefault();
    loadBoard();
  }
  if (
    event.key === "/" &&
    !event.metaKey &&
    !event.ctrlKey &&
    !event.altKey &&
    !event.shiftKey &&
    !event.repeat &&
    !event.defaultPrevented &&
    elements.modal.hidden &&
    elements.memoryModal.hidden &&
    !boardShortcutIsBlocked(event)
  ) {
    event.preventDefault();
    elements.taskTitle.focus();
  }
});

if ("ResizeObserver" in window) {
  new ResizeObserver(() => requestAnimationFrame(fitRecentMemory)).observe(
    document.querySelector(".memory-rail"),
  );
}

async function start() {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      api();
      await loadBoard();
      try {
        await mountTerminalDock({ backend: api(), showError });
      } catch (error) {
        showError(error);
      }
      window.setInterval(() => loadBoard(board?.planId || 0, true), 15000);
      return;
    } catch {
      await new Promise((resolve) => window.setTimeout(resolve, 100));
    }
  }
  showError("Could not connect to the Wails backend");
  setStatus("Backend unavailable");
}

start();
