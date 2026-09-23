import type { AppContext } from "./app-context";
import { agentContextText, type AgentTaskDetail } from "./copy-context";
import { element, emptyMemory, svgElement } from "./dom";
import { shortRelativeTime } from "./format";
import {
  currentPlanCloseoutLabel,
  filterPlans,
  pinnedPlanSelection,
  planIsViewOnly,
  planViewingLabel,
  splitCurrentPlan,
} from "./plan-list";
import { bindPlanMotion } from "./plan-motion";
import {
  boardGridColumns,
  collapsedLaneStatuses,
  linkedTaskRuntimePresentation,
} from "./presentation";
import type { BoardColumn, BoardPlan, BoardTask } from "./snapshot-types";
import { laneColors } from "./task-status";

export type ContextScope = "plan" | "task" | "project";

/** A task handed to "Copy context", with whatever detail the drawer loaded. */
export type ContextTask = BoardTask & { detail?: AgentTaskDetail };

export interface MenuPosition {
  x: number;
  y: number;
}

export function contextChip(count: number, singular: string, extraClass = ""): HTMLSpanElement {
  const chip = document.createElement("span");
  chip.className = `context-chip ${extraClass}`.trim();
  chip.textContent = `${count} ${count === 1 ? singular : `${singular}s`}`;
  return chip;
}

// Accent check chip marking a completed plan in the sidebar.
export function planDoneTick(): HTMLSpanElement {
  const svg = svgElement("svg", { viewBox: "0 0 12 12", "aria-hidden": "true" });
  svg.append(svgElement("path", { d: "M2 6.5 4.8 9 10 3.5" }));
  const tick = document.createElement("span");
  tick.className = "sidebar-plan-tick";
  tick.setAttribute("role", "img");
  tick.setAttribute("aria-label", "Done");
  tick.title = "Done";
  tick.append(svg);
  return tick;
}

// Status dots beside a sidebar plan name. Each one is named for screen
// readers and carries the same words as its tooltip. "On hold" parks the
// plan without changing its status; it never means blocked.
export function planFlagElements(plan: BoardPlan): HTMLSpanElement[] {
  const flags: HTMLSpanElement[] = [];
  const flag = (kind: string, label: string) => {
    const dot = document.createElement("span");
    dot.className = `sidebar-plan-flag ${kind}`;
    dot.setAttribute("role", "img");
    dot.setAttribute("aria-label", label);
    dot.title = label;
    flags.push(dot);
  };
  if (plan.holdReason) flag("hold", `On hold: ${plan.holdReason}`);
  if (plan.claimedBy) flag("claim", `Claimed by ${plan.claimedBy}`);
  if (plan.depsOpen?.length) {
    flag("deps", `Waiting on ${plan.depsOpen.map((id) => `#${id}`).join(", ")}`);
  }
  return flags;
}

/**
 * The badges a card carries beside its title — linked runtime, hold, open
 * dependencies, latest note, context counts — each appended to `dragZone`,
 * whose accessible name gains the states a screen reader must hear.
 */
export function appendCardBadges(dragZone: HTMLElement, task: BoardTask): void {
  const linkedRuntime = linkedTaskRuntimePresentation(task.linkedRuntime);
  if (linkedRuntime) {
    const linked = document.createElement("span");
    linked.className = "card-linked-runtime";
    linked.dataset.state = linkedRuntime.state;
    linked.textContent = linkedRuntime.compact;
    linked.title = linkedRuntime.detail;
    linked.setAttribute("aria-label", `Linked runtime: ${linkedRuntime.detail}`);
    dragZone.append(linked);
  }

  // Hold is orthogonal to status: the card keeps its lane and gains a badge.
  // Drag and drop stay enabled — a held task can still change status.
  if (task.holdReason) {
    const hold = document.createElement("span");
    hold.className = "card-hold";
    hold.textContent = "⏸ On hold";
    // Parked, not blocked: the status and the lane are unchanged.
    hold.title = `On hold: ${task.holdReason}`;
    dragZone.append(hold);
    dragZone.setAttribute(
      "aria-label",
      `${dragZone.getAttribute("aria-label")}, on hold`,
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
      `${dragZone.getAttribute("aria-label")}, waiting on dependencies`,
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
}

function laneHeading(column: BoardColumn): HTMLHeadingElement {
  const heading = document.createElement("h3");
  heading.className = "column-title";
  const dot = document.createElement("span");
  dot.className = "column-dot";
  dot.setAttribute("aria-hidden", "true");
  heading.append(dot, document.createTextNode(column.title));
  return heading;
}

function laneCount(column: BoardColumn): HTMLSpanElement {
  const count = document.createElement("span");
  count.className = "column-count";
  count.textContent = String(column.tasks.length);
  return count;
}

function activationKey(event: KeyboardEvent): boolean {
  return event.key === "Enter" || event.key === " ";
}

export function createBoardView(ctx: AppContext) {
  const { api, setStatus, showError, workspaceController } = ctx;
  const elements = {
    addForm: element("#add-form", HTMLFormElement),
    board: element("#board", HTMLElement),
    contextCopyStatus: element("#context-copy-status", HTMLSpanElement),
    planAdd: element("#plan-add", HTMLButtonElement),
    planCopyContext: element("#plan-copy-context", HTMLButtonElement),
    planEyebrow: element("#plan-eyebrow", HTMLParagraphElement),
    planFilterClear: element("#plan-filter-clear", HTMLButtonElement),
    planFilterSummary: element("#plan-filter-summary", HTMLParagraphElement),
    planFilterToggle: element("#plan-filter-toggle", HTMLButtonElement),
    planFilters: element("#plan-filters", HTMLDivElement),
    planLaunchAgent: element("#plan-launch-agent", HTMLButtonElement),
    planList: element("#sidebar-plan-list", HTMLDivElement),
    planProgress: element("#plan-progress", HTMLSpanElement),
    planProgressLabel: element("#plan-progress-label", HTMLSpanElement),
    planSearch: element("#plan-search", HTMLInputElement),
    planStatusFilter: element("#plan-status-filter", HTMLSelectElement),
    planTitle: element("#plan-title", HTMLHeadingElement),
    planTitleMenu: element("#plan-title-menu", HTMLButtonElement),
    planTotal: element("#plan-total", HTMLSpanElement),
    projectName: element("#project-name", HTMLHeadingElement),
    sidebarCurrent: element("#sidebar-current", HTMLElement),
    sidebarCurrentSlot: element("#sidebar-current-slot", HTMLDivElement),
    taskTitle: element("#task-title", HTMLInputElement),
  };
  const addTaskButton = element("button", HTMLButtonElement, elements.addForm);

  let draggedTask: BoardTask | null = null;
  let drawerOpenTimer = 0;
  let addTaskPending = false;
  let dragJustEndedAt = 0;

  // Same gear as the board header's plan-actions trigger, cloned so the icon
  // path lives in index.html exactly once.
  function gearIcon(): Node {
    return element("svg", SVGSVGElement, elements.planTitleMenu).cloneNode(true);
  }

  function cardMenuButton(task: BoardTask): HTMLButtonElement {
    // Same gear trigger as the plan rows, tucked at the meta line's end so it
    // never collides with the Drag hint; revealed on hover alongside it.
    const cardMenu = document.createElement("button");
    cardMenu.type = "button";
    cardMenu.className = "card-menu";
    cardMenu.append(gearIcon());
    cardMenu.setAttribute("aria-label", `Task #${task.id} actions`);
    cardMenu.title = `Task #${task.id} actions`;
    cardMenu.setAttribute("aria-haspopup", "menu");
    cardMenu.addEventListener("click", (event) => {
      event.stopPropagation();
      const rect = cardMenu.getBoundingClientRect();
      ctx.planDialogs.openTaskContextMenu(task, { x: rect.right, y: rect.bottom + 4 }, cardMenu);
    });
    return cardMenu;
  }

  function cancelPendingDrawerOpen(): void {
    window.clearTimeout(drawerOpenTimer);
    drawerOpenTimer = 0;
  }

  function bindCardInteractions(card: HTMLElement, dragZone: HTMLElement, task: BoardTask): void {
    // Anywhere on the card except its own controls opens the task drawer.
    card.addEventListener("click", (event) => {
      if (event.target instanceof Element && event.target.closest("button, a, input, select, textarea")) {
        return;
      }
      // A click that ends a drag, or the first click of a double-click rename,
      // must not open the drawer.
      if (Date.now() - dragJustEndedAt < 300) return;
      window.clearTimeout(drawerOpenTimer);
      drawerOpenTimer = window.setTimeout(() => {
        drawerOpenTimer = 0;
        ctx.drawer.openTaskDetail(task);
      }, 240);
    });
    dragZone.addEventListener("keydown", (event) => {
      if (event.target !== dragZone) return;
      if (!activationKey(event)) return;
      event.preventDefault();
      cancelPendingDrawerOpen();
      ctx.drawer.openTaskDetail(task);
    });
    dragZone.addEventListener("dblclick", () => {
      cancelPendingDrawerOpen();
      ctx.planDialogs.openRename(task);
    });
    dragZone.addEventListener("dragstart", (event) => {
      draggedTask = task;
      if (event.dataTransfer) {
        event.dataTransfer.effectAllowed = "move";
        event.dataTransfer.setData("text/plain", String(task.id));
      }
      requestAnimationFrame(() => card.classList.add("dragging"));
    });
    dragZone.addEventListener("dragend", () => {
      draggedTask = null;
      dragJustEndedAt = Date.now();
      card.classList.remove("dragging");
      document.querySelectorAll(".drag-over").forEach((node) => node.classList.remove("drag-over"));
    });
    // Right-click opens the same task menu as the gear trigger.
    card.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      ctx.planDialogs.openTaskContextMenu(task, { x: event.clientX, y: event.clientY });
    });
  }

  function cardElement(task: BoardTask): HTMLElement {
    const card = document.createElement("article");
    card.className = "card";
    card.dataset.taskId = String(task.id);
    card.dataset.status = task.status;

    const dragZone = document.createElement("div");
    dragZone.className = "card-drag-zone";
    dragZone.draggable = true;
    dragZone.tabIndex = 0;
    // The label names the task; the visible card carries the rest. Body text
    // and runtime details stay out of the label or screen readers announce a
    // full paragraph before every card.
    dragZone.setAttribute(
      "aria-label",
      `Task #${task.id}, ${task.status}: ${task.title}`,
    );
    const meta = document.createElement("div");
    meta.className = "card-meta";
    const identity = document.createElement("span");
    identity.textContent = `#${task.id} · ${shortRelativeTime(task.updatedAt)}`;
    meta.append(identity, cardMenuButton(task));
    const title = document.createElement("p");
    title.className = "card-title";
    title.textContent = task.title;
    dragZone.append(meta, title);
    appendCardBadges(dragZone, task);
    bindCardInteractions(card, dragZone, task);

    // The lane already shows the status, so the card carries no status
    // control of its own: drag it, use the drawer's status field, or pick
    // "Move to …" from the ⋯ / right-click menu.
    card.append(dragZone);
    return card;
  }

  function setLaneFolded(column: BoardColumn, folded: boolean): void {
    if (folded) {
      ctx.state.expandedLanes.delete(column.status);
      ctx.state.foldedLanes.add(column.status);
    } else {
      ctx.state.foldedLanes.delete(column.status);
      ctx.state.expandedLanes.add(column.status);
    }
    renderBoard();
    ctx.layout.recordProjectLayout();
  }

  // Slim rail for an empty lane: rotated title + count, click to expand.
  function collapsedLane(lane: HTMLElement, column: BoardColumn): void {
    lane.setAttribute("role", "button");
    lane.tabIndex = 0;
    lane.setAttribute("aria-expanded", "false");
    lane.setAttribute("aria-label", `${column.title} lane is collapsed. Activate to expand.`);
    lane.title = `${column.title} · ${column.tasks.length} — click to expand`;
    const rail = document.createElement("div");
    rail.className = "column-rail";
    rail.append(laneHeading(column), laneCount(column));
    lane.append(rail);
    lane.addEventListener("click", (event) => {
      if (Date.now() - dragJustEndedAt < 300) return;
      event.preventDefault();
      setLaneFolded(column, false);
    });
    lane.addEventListener("keydown", (event) => {
      if (!activationKey(event)) return;
      event.preventDefault();
      setLaneFolded(column, false);
    });
  }

  function expandedLane(lane: HTMLElement, column: BoardColumn): void {
    const header = document.createElement("header");
    header.className = "column-header";
    const fold = document.createElement("button");
    fold.type = "button";
    fold.className = "column-fold";
    fold.textContent = "⌄";
    fold.title = `Collapse ${column.title} lane`;
    fold.setAttribute("aria-label", `Collapse ${column.title} lane`);
    fold.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      setLaneFolded(column, true);
    });
    header.append(laneHeading(column), laneCount(column), fold);
    const cards = document.createElement("div");
    cards.className = "cards";
    if (column.tasks.length === 0) {
      const empty = document.createElement("div");
      empty.className = "empty";
      empty.textContent = ctx.state.board?.planId ? "Drop a task here" : "No active plan";
      cards.append(empty);
    } else {
      column.tasks.forEach((task) => cards.append(cardElement(task)));
    }
    lane.append(header, cards);
  }

  function bindLaneDrop(lane: HTMLElement, column: BoardColumn): void {
    lane.addEventListener("dragover", (event) => {
      if (!draggedTask || draggedTask.status === column.status) return;
      event.preventDefault();
      if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
      lane.classList.add("drag-over");
    });
    lane.addEventListener("dragleave", (event) => {
      if (!(event.relatedTarget instanceof Node) || !lane.contains(event.relatedTarget)) {
        lane.classList.remove("drag-over");
      }
    });
    lane.addEventListener("drop", (event) => {
      event.preventDefault();
      lane.classList.remove("drag-over");
      if (draggedTask && draggedTask.status !== column.status) {
        const taskId = draggedTask.id;
        const invoker = document.querySelector(
          `.card[data-task-id="${taskId}"] .card-drag-zone`,
        );
        void ctx.snapshot.moveTask(taskId, column.status, invoker);
      }
    });
  }

  function columnElement(column: BoardColumn, collapsed = false): HTMLElement {
    const lane = document.createElement("section");
    lane.className = collapsed ? "column column-collapsed" : "column";
    lane.dataset.status = column.status;
    lane.style.setProperty("--lane-color", laneColors[column.status]);
    if (collapsed) collapsedLane(lane, column);
    else expandedLane(lane, column);
    bindLaneDrop(lane, column);
    return lane;
  }

  function taskDragActive(): boolean {
    return draggedTask !== null;
  }

  // Choosing a plan makes it the project's current plan — the same pointer
  // `ptrack plan use` moves — and only then shows its board. A done or
  // archived plan cannot be current, so choosing one only views it, and a
  // plan that is already current needs no call. A refused change (a plan
  // claimed by someone else, one that vanished, a stale workspace) leaves the
  // board on the plan it showed.
  async function activatePlan(planId: number): Promise<void> {
    const plan = ctx.state.board?.plans.find((candidate) => Number(candidate.id) === Number(planId));
    if (!plan || plan.isActive || planIsViewOnly(plan)) {
      await ctx.snapshot.loadSnapshot(planId);
      return;
    }
    const ticket = workspaceController.capture();
    try {
      const reply = await api().SetActivePlanV1(ticket.generation, Number(planId));
      if (!workspaceController.accepts(ticket, Number(reply.generation))) return;
    } catch (error) {
      // A task the palette or an issue asked to open stays unopened with the
      // plan it lives in.
      ctx.drawer.clearPendingTaskDetail();
      showError(error);
      setStatus(`Could not make plan #${planId} the current plan`);
      return;
    }
    await ctx.snapshot.loadSnapshot(planId);
  }

  function selectPlan(planId: number): void {
    if (ctx.state.firstPlanState.phase !== "idle") return;
    void activatePlan(planId);
  }

  // Selecting and opening the plan menu from a plan row or the current-plan
  // card. Keys bubbling up from the nested gear button (Enter/Space there
  // should trigger it) never also select the row.
  function bindPlanSelection(target: HTMLElement, title: HTMLElement, plan: BoardPlan): void {
    target.addEventListener("click", () => selectPlan(plan.id));
    target.addEventListener("keydown", (event) => {
      if (event.target !== target) return;
      if (!activationKey(event)) return;
      event.preventDefault();
      selectPlan(plan.id);
    });
    target.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      ctx.planDialogs.openPlanContextMenu(plan, title, target, { x: event.clientX, y: event.clientY });
    });
  }

  function planMenuButton(plan: BoardPlan, title: HTMLElement, withTooltip: boolean): HTMLButtonElement {
    const menuButton = document.createElement("button");
    menuButton.type = "button";
    menuButton.className = "sidebar-plan-menu";
    menuButton.append(gearIcon());
    menuButton.setAttribute("aria-label", `Plan #${plan.id} actions`);
    if (withTooltip) menuButton.title = `Plan #${plan.id} actions`;
    menuButton.setAttribute("aria-haspopup", "menu");
    menuButton.addEventListener("click", (event) => {
      event.stopPropagation();
      const rect = menuButton.getBoundingClientRect();
      ctx.planDialogs.openPlanContextMenu(plan, title, menuButton, { x: rect.left, y: rect.bottom + 4 });
    });
    return menuButton;
  }

  function planRow(plan: BoardPlan, selectedPlanId: number): HTMLElement {
    // Not a native <button>: it hosts the nested "⋯" plan-actions button
    // below, and interactive content can't nest inside a real button.
    const item = document.createElement("div");
    item.setAttribute("role", "button");
    item.tabIndex = 0;
    item.className = "sidebar-plan";
    const doneTasks = Number(plan.tasksDone || 0);
    const totalTasks = Number(plan.tasksTotal || 0);
    const settled = plan.status === "done" || plan.status === "archived";
    if (String(plan.id) === String(selectedPlanId)) {
      item.classList.add("active");
      item.setAttribute("aria-current", "true");
    }
    if (settled) item.classList.add("settled");
    item.title = `#${plan.id} ${plan.title}`;
    if (totalTasks > 0) item.title = `${item.title} · ${doneTasks}/${totalTasks} done`;
    const title = document.createElement("span");
    title.className = "sidebar-plan-title";
    title.textContent = `#${plan.id} ${plan.title}`;
    // The row truncates long names; the full name rides on the title too.
    title.title = `#${plan.id} ${plan.title}`;
    item.append(title);
    bindPlanMotion(item, title);
    if (plan.status === "done") item.append(planDoneTick());
    const flags = planFlagElements(plan);
    item.append(...flags);
    flags.forEach((flag) => {
      item.title = `${item.title} · ${flag.title}`;
    });
    bindPlanSelection(item, title, plan);
    item.append(planMenuButton(plan, title, true));
    return item;
  }

  function renderPlanList(): void {
    const board = ctx.state.board;
    const plans = board
      ? filterPlans(board.plans, elements.planSearch.value, elements.planStatusFilter.value)
      : [];
    const filtered = Boolean(board) &&
      (elements.planSearch.value.trim() !== "" || elements.planStatusFilter.value !== "all");
    // The project's current plan is pinned above the list as its own card, so
    // the scrollable rows carry everyone else. Pinning also keeps the expanded
    // card out of the grid track sizing that painted it over the next row.
    // The card survives filtering on purpose: it is the project's active
    // context, and the filters only reshape the browsing list below it.
    // A view-only plan open on the board stays in the rows, highlighted.
    const pinned = board ? pinnedPlanSelection(board.plans, board.planId) : 0;
    const { current } = board ? splitCurrentPlan(board.plans, pinned) : { current: undefined };
    const { rest } = splitCurrentPlan(plans, pinned);
    renderCurrentPlan(current);
    elements.planList.replaceChildren();
    if (!board) return;
    elements.planTotal.textContent = filtered ? `${plans.length}/${board.plans.length}` : String(board.plans.length);
    elements.planFilterToggle.dataset.active = String(filtered);
    const total = Number(board.stats.plans || board.plans.length);
    elements.planFilterSummary.textContent = `${plans.length} of ${board.plans.length} loaded plans${total > board.plans.length ? ` (${total} in project)` : ""}.`;
    if (!board.plans.length) {
      elements.planList.append(emptyMemory("No plans yet. Create a plan with +."));
    } else if (filtered && !plans.length) {
      elements.planList.append(emptyMemory("No plans match these filters."));
    }
    rest.forEach((plan) => elements.planList.append(planRow(plan, board.planId)));
  }

  function currentPlanProgress(card: HTMLElement, doneTasks: number, totalTasks: number): void {
    if (totalTasks > 0) {
      const meter = document.createElement("span");
      meter.className = "sidebar-current-meter";
      meter.setAttribute("aria-hidden", "true");
      const fill = document.createElement("span");
      fill.className = "sidebar-current-meter-fill";
      fill.style.width = `${Math.round((doneTasks / totalTasks) * 100)}%`;
      meter.append(fill);
      card.append(meter);
    }
    const stats = document.createElement("span");
    stats.className = "sidebar-current-stats";
    if (totalTasks > 0) {
      const pct = document.createElement("span");
      pct.className = "sidebar-current-stats-pct";
      pct.textContent = `${Math.round((doneTasks / totalTasks) * 100)}%`;
      stats.append(
        document.createTextNode(`${doneTasks}/${totalTasks} done · `),
        pct,
        document.createTextNode(` · ${totalTasks - doneTasks} left`),
      );
    } else {
      stats.textContent = "No tasks yet";
    }
    card.append(stats);
  }

  // A finished plan that is still active asks to be closed right here,
  // instead of staying pinned as "current" with nothing left to do.
  function currentPlanCloseout(card: HTMLElement, plan: BoardPlan): void {
    const closeout = currentPlanCloseoutLabel(plan);
    if (!closeout) return;
    const close = document.createElement("button");
    close.type = "button";
    close.className = "sidebar-current-closeout";
    close.textContent = closeout;
    close.addEventListener("click", (event) => {
      event.stopPropagation();
      ctx.planDialogs.openPlanDoneDialog(plan);
    });
    card.append(close);
  }

  // The project's single current plan, pinned above the plan rows in a void
  // card. Rendered in plain block flow — never as a grid row — so its height
  // always follows its content. A done plan that is still the current plan
  // keeps the meter: a full aurora bar reads as "complete, wrap it up", and
  // the ✓ tick rides inline before the title.
  function renderCurrentPlan(plan: BoardPlan | undefined): void {
    elements.sidebarCurrentSlot.replaceChildren();
    if (!plan) {
      elements.sidebarCurrent.hidden = true;
      return;
    }
    elements.sidebarCurrent.hidden = false;
    const doneTasks = Number(plan.tasksDone || 0);
    const totalTasks = Number(plan.tasksTotal || 0);
    const card = document.createElement("div");
    card.setAttribute("role", "button");
    card.tabIndex = 0;
    card.className = "sidebar-current-card";
    card.title = `#${plan.id} ${plan.title} · current plan`;
    if (totalTasks > 0) card.title = `${card.title} · ${doneTasks}/${totalTasks} done`;
    for (const flag of planFlagElements(plan)) card.title = `${card.title} · ${flag.title}`;
    const row = document.createElement("span");
    row.className = "sidebar-current-title-row";
    // The tick leads the title so it can't collide with the gear pinned at
    // the card's top-right corner.
    if (plan.status === "done") row.append(planDoneTick());
    const title = document.createElement("span");
    title.className = "sidebar-current-title";
    title.textContent = `#${plan.id} ${plan.title}`;
    title.title = `#${plan.id} ${plan.title}`;
    row.append(title, ...planFlagElements(plan));
    card.append(row);
    currentPlanProgress(card, doneTasks, totalTasks);
    currentPlanCloseout(card, plan);
    bindPlanSelection(card, title, plan);
    card.append(planMenuButton(plan, title, false));
    elements.sidebarCurrentSlot.append(card);
  }

  function renderBoard(): void {
    const board = ctx.state.board;
    if (!board) return;
    elements.projectName.textContent = board.projectName;
    elements.planTitle.textContent = board.planTitle || "No active plan";
    // One plan concept: the plan on the board is the current plan, the same
    // one the sidebar pins and the one Add task writes to.
    // A done or archived plan open for reading says so, instead of claiming
    // to be the plan new work goes to.
    const shown = board.plans.find((plan) => Number(plan.id) === Number(board.planId));
    elements.planEyebrow.hidden = board.planId === 0;
    elements.planEyebrow.textContent = shown && planIsViewOnly(shown)
      ? planViewingLabel(shown)
      : "Current plan";
    renderPlanList();
    const total = board.stats.planTasks;
    const done = board.stats.planTasksDone;
    const percentage = total ? Math.round((done / total) * 100) : 0;
    elements.planProgress.style.width = `${percentage}%`;
    elements.planProgressLabel.textContent = `${done}/${total} done`;
    elements.taskTitle.disabled = board.planId === 0;
    addTaskButton.disabled = board.planId === 0 || addTaskPending;
    elements.planLaunchAgent.disabled = board.planId === 0;
    elements.planCopyContext.disabled = board.planId === 0;
    const collapsed = new Set(
      collapsedLaneStatuses(
        board.columns.map((column) => ({
          status: column.status,
          taskCount: column.tasks.length,
        })),
        ctx.state.expandedLanes,
        ctx.state.foldedLanes,
      ),
    );
    elements.board.style.gridTemplateColumns = boardGridColumns(
      board.columns.map((column) => column.status),
      collapsed,
    );
    elements.board.replaceChildren();
    board.columns.forEach((column) =>
      elements.board.append(columnElement(column, collapsed.has(column.status))),
    );
    ctx.overview.renderMemory();
  }

  function clearPlanFilters(): void {
    elements.planSearch.value = "";
    elements.planStatusFilter.value = "all";
    elements.planFilterToggle.dataset.active = "false";
    elements.planFilterSummary.textContent = "";
    renderPlanList();
  }

  function currentBoardPlan(): BoardPlan | null {
    const board = ctx.state.board;
    if (!board?.planId) return null;
    return board.plans?.find((plan) => Number(plan.id) === Number(board.planId)) || {
      id: Number(board.planId),
      title: board.planTitle || `Plan #${board.planId}`,
      status: "active",
      tasksTotal: Number(board.stats?.planTasks || 0),
      tasksDone: Number(board.stats?.planTasksDone || 0),
    };
  }

  function contextPlan(
    scope: ContextScope,
    task: ContextTask | null,
    planOverride: BoardPlan | undefined,
  ): BoardPlan | undefined {
    if (planOverride) return planOverride;
    if (scope === "project") return undefined;
    if (task?.planId) {
      return ctx.state.board?.plans.find((candidate) => Number(candidate.id) === Number(task.planId));
    }
    return currentBoardPlan() ?? undefined;
  }

  function markCopied(invoker: HTMLElement): void {
    invoker.dataset.copied = "true";
    window.setTimeout(() => { delete invoker.dataset.copied; }, 1800);
    if (!invoker.querySelector("svg")) {
      const label = invoker.dataset.copyLabel || invoker.textContent || "";
      invoker.dataset.copyLabel = label;
      invoker.textContent = "Copied";
      window.setTimeout(() => { invoker.textContent = label; }, 1800);
    }
  }

  async function copyAgentContext(
    scope: ContextScope,
    task: ContextTask | null,
    invoker: HTMLElement | null,
    planOverride?: BoardPlan,
  ): Promise<void> {
    try {
      const plan = contextPlan(scope, task, planOverride);
      if (scope !== "project" && !plan) throw new Error("Refresh the plan before copying its context.");
      const text = agentContextText({
        project: {
          name: ctx.state.board?.projectName || ctx.state.workspaceState.project?.name || "Project",
          root: ctx.state.workspaceState.project?.root || "",
          goal: ctx.state.board?.goal,
        },
        plan,
        task: scope === "task" && task ? task : undefined,
      });
      if ((await window.runtime?.ClipboardSetText?.(text)) !== true) {
        throw new Error("Clipboard unavailable. Try copying again.");
      }
      const subject = scope === "task" ? `Task #${task?.id}` : scope === "plan" ? `Plan #${plan?.id}` : "Project";
      setStatus(`${subject} context copied.`);
      elements.contextCopyStatus.textContent = `${scope === "task" ? "Task" : scope === "plan" ? "Plan" : "Project"} context copied to clipboard.`;
      if (invoker) markCopied(invoker);
    } catch (error) {
      showError(error);
    }
  }

  // While AddTaskV2 is pending the field is read-only (it keeps focus, so the
  // user's place survives) and the button is disabled; the in-flight key
  // refuses a second Enter. The title clears only once the task was added.
  function setAddTaskPending(pending: boolean): void {
    addTaskPending = pending;
    if (pending) elements.addForm.setAttribute("aria-busy", "true");
    else elements.addForm.removeAttribute("aria-busy");
    elements.taskTitle.readOnly = pending;
    addTaskButton.disabled = pending || !ctx.state.board?.planId;
  }

  async function submitNewTask(): Promise<void> {
    if (addTaskPending) return;
    const title = elements.taskTitle.value.trim();
    const board = ctx.state.board;
    if (!title || !board?.planId) return;
    const ticket = workspaceController.capture();
    const planId = Number(board.planId);
    setAddTaskPending(true);
    let added = false;
    try {
      added = await ctx.snapshot.runMutation(
        (generation) => api().AddTaskV2(generation, planId, title),
        "Adding task…",
        "Could not add task",
        "add-task",
      );
    } finally {
      setAddTaskPending(false);
    }
    if (!workspaceController.accepts(ticket, ticket.generation)) return;
    if (added && elements.taskTitle.value.trim() === title) elements.taskTitle.value = "";
    elements.taskTitle.focus();
  }

  function openCurrentPlanMenu(invoker: HTMLElement, position: MenuPosition): void {
    const plan = currentBoardPlan();
    if (!plan) return;
    ctx.planDialogs.openPlanContextMenu(plan, elements.planTitle, invoker, position);
  }

  function setPlanFiltersOpen(open: boolean): void {
    elements.planFilters.hidden = !open;
    elements.planFilterToggle.setAttribute("aria-expanded", String(open));
  }

  function bind(): void {
    elements.planLaunchAgent.addEventListener("click", () => {
      if (!ctx.state.board?.planId) return;
      void ctx.agentLaunch.openAgentLaunchPicker(
        { planId: Number(ctx.state.board.planId) },
        elements.planLaunchAgent,
      );
    });
    elements.planTitle.addEventListener("contextmenu", (event) => {
      if (!currentBoardPlan()) return;
      event.preventDefault();
      openCurrentPlanMenu(elements.planTitle, { x: event.clientX, y: event.clientY });
    });
    elements.planTitleMenu.addEventListener("click", () => {
      const rect = elements.planTitleMenu.getBoundingClientRect();
      openCurrentPlanMenu(elements.planTitleMenu, { x: rect.left, y: rect.bottom + 4 });
    });
    elements.planCopyContext.addEventListener("click", () =>
      void copyAgentContext("plan", null, elements.planCopyContext),
    );
    elements.planAdd.addEventListener("click", () => ctx.planDialogs.openNewPlanDialog());
    elements.planFilterToggle.addEventListener("click", () => {
      setPlanFiltersOpen(Boolean(elements.planFilters.hidden));
      if (!elements.planFilters.hidden) elements.planSearch.focus();
    });
    elements.planSearch.addEventListener("input", renderPlanList);
    elements.planStatusFilter.addEventListener("change", renderPlanList);
    elements.planFilterClear.addEventListener("click", () => {
      clearPlanFilters();
      elements.planSearch.focus();
    });
    elements.planFilters.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      setPlanFiltersOpen(false);
      elements.planFilterToggle.focus();
    });
    elements.addForm.addEventListener("submit", (event) => {
      event.preventDefault();
      void submitNewTask();
    });
  }

  return {
    bind,
    taskDragActive,
    selectPlan,
    renderBoard,
    clearPlanFilters,
    currentBoardPlan,
    copyAgentContext,
  };
}

export type BoardView = ReturnType<typeof createBoardView>;
