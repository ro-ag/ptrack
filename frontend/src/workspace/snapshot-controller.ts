import type { AppContext } from "./app-context";
import {
  ApplicationOverlayCoordinator,
  type ApplicationOverlayChange,
} from "./application-overlay";
import { InFlightOperations, RefreshLoop, RuntimeRefreshCoalescer } from "./controller";
import { messageFrom } from "./format";
import { preserveSectionOnError, runtimeEventIsCurrent } from "./presentation";
import { element } from "./dom";
import type { BoardTask, GenerationReply, WorkspaceSnapshot } from "./snapshot-types";
import { isTaskStatus, statusTitles, type TaskStatus } from "./task-status";
import {
  taskTransitionCanStart,
  taskTransitionConfirmationCopy,
  taskTransitionFocusIntent,
  taskTransitionResponseIsCurrent,
  type TaskTransitionConfirmation,
  type TaskTransitionOrigin,
  type TaskTransitionResult,
} from "./task-transition";

/** One requested status change, fenced to the board it started from. */
interface TaskTransitionRequest {
  sequence: number;
  generation: number;
  planId: number;
  taskId: number;
  fromStatus: TaskStatus;
  toStatus: TaskStatus;
  invoker: HTMLElement | null;
  origin: TaskTransitionOrigin;
  confirmation: TaskTransitionConfirmation | null;
}

/** A plan request: a plan id, 0 for the active plan, or null for "no plan". */
export type SnapshotPlanRequest = number | null;

/** A mutation reports the generation it ran in when it has one. */
export type MutationOperation = (generation: number) => Promise<Partial<GenerationReply> | void>;

export const applicationOverlaySelector =
  "body > .modal, body > [data-terminal-overlay]";

/** The overlay open/close changes a batch of `hidden` mutations describes. */
export function applicationOverlayChanges(
  records: readonly Pick<MutationRecord, "attributeName" | "target" | "oldValue">[] = [],
): ApplicationOverlayChange[] {
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

function dragZoneFor(taskId: number): HTMLElement | null {
  return document.querySelector<HTMLElement>(
    `.card[data-task-id="${taskId}"] .card-drag-zone`,
  );
}

export function createSnapshotController(ctx: AppContext) {
  const { api, nativeEventDisposers, refreshGate, setStatus, showError, workspaceController } = ctx;
  const elements = {
    app: element("#app", HTMLDivElement),
    drawer: element("#task-drawer", HTMLDivElement),
    drawerStatusSelect: element("#drawer-status-select", HTMLSelectElement),
    taskTitle: element("#task-title", HTMLInputElement),
    taskTransitionCancel: element("#task-transition-cancel", HTMLButtonElement),
    taskTransitionDetail: element("#task-transition-detail", HTMLParagraphElement),
    taskTransitionForm: element("#task-transition-form", HTMLFormElement),
    taskTransitionHeading: element("#task-transition-heading", HTMLHeadingElement),
    taskTransitionMessage: element("#task-transition-message", HTMLParagraphElement),
    taskTransitionModal: element("#task-transition-modal", HTMLDivElement),
    taskTransitionSubmit: element("#task-transition-submit", HTMLButtonElement),
    workspace: element("#workspace", HTMLElement),
  };

  // The background poll pauses while the window is hidden and catches up once
  // when it is shown again.
  const refreshLoop = new RefreshLoop(() => {
    void loadSnapshot(ctx.state.board?.planId || 0, true);
  }, 15_000, () => document.hidden);

  const mutationsInFlight = new InFlightOperations();

  const runtimeRefreshes = new RuntimeRefreshCoalescer((generation: number) => {
    if (!runtimeEventIsCurrent(
      generation,
      workspaceController.state.generation,
      workspaceController.state.status === "open",
    )) return;
    void loadSnapshot(ctx.state.board?.planId || 0, true);
  });

  let snapshotSequence = 0;
  let activeSnapshotRequest: number | null = null;
  let queuedSnapshotPlanRequest: SnapshotPlanRequest | undefined;
  let explicitNoPlanSelection = false;
  let taskTransitionRequest: TaskTransitionRequest | null = null;
  let taskTransitionSequence = 0;
  let taskTransitionBusy = false;

  function snapshotDialogIsOpen(): boolean {
    return applicationOverlayCoordinator.isOpen();
  }

  function syncApplicationOverlayState(records: readonly MutationRecord[] = []): void {
    const changes = applicationOverlayChanges(records);
    applicationOverlayCoordinator.reconcile(changes);
    schedulePlanCompletionPrompt(changes);
  }

  function hideApplicationOverlay(overlay: HTMLElement): void {
    overlay.hidden = true;
    const changes = [{ overlay, open: false }];
    applicationOverlayCoordinator.reconcile(changes);
    schedulePlanCompletionPrompt(changes);
  }

  function schedulePlanCompletionPrompt(changes: readonly ApplicationOverlayChange[]): void {
    if (!changes.some((change) => !change.open)) return;
    queueMicrotask(() => {
      if (!snapshotDialogIsOpen()) ctx.planDialogs.maybePromptForPlanCompletion();
    });
  }

  const applicationOverlayCoordinator = new ApplicationOverlayCoordinator(() =>
    document.querySelectorAll<HTMLElement>(applicationOverlaySelector),
    elements.app,
  );

  const applicationOverlayObserver = new MutationObserver((records) =>
    syncApplicationOverlayState(records)
  );

  // A quiet refresh never lands under someone mid-gesture: a dialog, a drag,
  // a half-typed task title, or a plan being renamed in place.
  function quietRefreshBlocked(): boolean {
    return snapshotDialogIsOpen() ||
      ctx.board.taskDragActive() ||
      elements.taskTitle.value.trim().length > 0 ||
      ctx.planDialogs.inlinePlanEditActive();
  }

  function applySnapshot(response: WorkspaceSnapshot, planRequest: SnapshotPlanRequest): void {
    response.git = preserveSectionOnError(ctx.state.snapshot?.git, response.git);
    ctx.state.snapshot = response;
    ctx.state.board = response.tracking.board;
    explicitNoPlanSelection = planRequest === null;
    elements.workspace.dataset.snapshotState = "ready";
    ctx.overview.withOverviewScrollPreserved(() => {
      ctx.board.renderBoard();
      ctx.layout.recordProjectLayout();
      ctx.overview.renderIntelligence();
    });
    ctx.shell.applyView();
    ctx.drawer.openPendingTaskDetail();
    if (ctx.state.view === "issues") void ctx.issues.loadIssues(true);
    ctx.overview.reloadRequestedOverviewPanels();
    // Every snapshot re-reads the stack with a plain read: the backend scans
    // only when HEAD moved since the stored profile.
    void ctx.overview.loadStackProfile();
    const now = new Date(response.capturedAt).toLocaleTimeString([], {
      hour: "numeric",
      minute: "2-digit",
    });
    setStatus(`Snapshot synced ${now}`);
    ctx.planDialogs.maybePromptForPlanCompletion();
  }

  function reportSnapshotFailure(error: unknown): void {
    if (ctx.state.snapshot) {
      elements.workspace.dataset.snapshotState = "stale";
      setStatus(`Snapshot stale · ${messageFrom(error)}`);
    } else {
      setStatus("Snapshot failed");
    }
    showError(error);
  }

  // A read that was refused while this one ran is replayed once, for the
  // workspace generation that asked for it.
  function finishSnapshotRead(request: number): void {
    if (activeSnapshotRequest === request) activeSnapshotRequest = null;
    const rerun = refreshGate.finish();
    if (rerun && workspaceController.state.status === "open") {
      const queuedPlan = queuedSnapshotPlanRequest !== undefined
        ? queuedSnapshotPlanRequest
        : explicitNoPlanSelection
          ? null
          : ctx.state.board?.planId || 0;
      const queuedGeneration = workspaceController.state.generation;
      queuedSnapshotPlanRequest = undefined;
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

  async function loadSnapshot(
    planId: SnapshotPlanRequest = ctx.state.board?.planId || 0,
    quiet = false,
    queueIfBusy = true,
  ): Promise<boolean> {
    if (workspaceController.state.status !== "open") return false;
    const planRequest = planId === null || (explicitNoPlanSelection && Number(planId) === 0)
      ? null
      : Number(planId);
    if (!refreshGate.tryBegin(!quiet && queueIfBusy)) {
      if (!quiet && queueIfBusy) queuedSnapshotPlanRequest = planRequest;
      return false;
    }
    if (quiet && quietRefreshBlocked()) {
      refreshGate.finish();
      return false;
    }

    const ticket = workspaceController.capture();
    const request = ++snapshotSequence;
    activeSnapshotRequest = request;
    if (!quiet) setStatus("Refreshing project snapshot…");
    try {
      const response = await api().GetWorkspaceSnapshot(ticket.generation, planRequest);
      if (request !== snapshotSequence || !workspaceController.accepts(ticket, response.generation)) {
        return true;
      }
      applySnapshot(response, planRequest);
    } catch (error) {
      // A superseded read reports no fresh board.
      if (request !== snapshotSequence || ticket.epoch !== workspaceController.capture().epoch) {
        return false;
      }
      reportSnapshotFailure(error);
    } finally {
      finishSnapshotRead(request);
    }
    return true;
  }

  // A project switch drops the previous project's snapshot and any queued read.
  function discardSnapshotForProjectChange(): void {
    snapshotSequence += 1;
    ctx.state.snapshot = null;
    ctx.state.board = null;
    explicitNoPlanSelection = false;
    queuedSnapshotPlanRequest = undefined;
    refreshGate.cancelQueued();
  }

  function cancelSnapshotsForClose(): void {
    snapshotSequence += 1;
    activeSnapshotRequest = null;
    queuedSnapshotPlanRequest = undefined;
    explicitNoPlanSelection = false;
    refreshGate.cancelQueued();
    runtimeRefreshes.cancel();
  }

  async function loadExactTaskTransitionSnapshot(planId: number, generation: number): Promise<boolean> {
    while (workspaceController.state.status === "open" &&
      workspaceController.state.generation === generation) {
      await refreshGate.whenIdle();
      if (workspaceController.state.status !== "open" ||
        workspaceController.state.generation !== generation ||
        Number(ctx.state.board?.planId) !== Number(planId)) return false;
      if (await loadSnapshot(planId, false, false)) {
        await refreshGate.whenIdle();
        return workspaceController.state.status === "open" &&
          workspaceController.state.generation === generation;
      }
    }
    return false;
  }

  // `key` names the operation for the in-flight guard: a second start of the
  // same key while the first is pending is refused, so a double Enter or double
  // click submits once. It defaults to the progress text, which already names
  // the action and its target. Resolves true only when the mutation succeeded
  // for the workspace that started it.
  async function runMutation(
    operation: MutationOperation,
    progress: string,
    failed: string,
    key = progress,
  ): Promise<boolean> {
    if (!ctx.state.board || workspaceController.state.status !== "open") return false;
    if (!mutationsInFlight.begin(key)) return false;
    try {
      return await runGuardedMutation(operation, progress, failed);
    } finally {
      mutationsInFlight.end(key);
    }
  }

  async function runGuardedMutation(
    operation: MutationOperation,
    progress: string,
    failed: string,
  ): Promise<boolean> {
    const ticket = workspaceController.capture();
    const active = document.activeElement;
    const focusKey = active instanceof HTMLElement ? active.dataset.mutationFocusKey || "" : "";
    setStatus(progress);
    try {
      const result = await operation(ticket.generation);
      if (result?.generation && !workspaceController.accepts(ticket, result.generation)) return false;
      await loadSnapshot(ctx.state.board?.planId ?? 0);
      ctx.agentActivity.restoreMutationFocus(focusKey);
      if (ctx.state.detailTask && !elements.drawer.hidden) {
        // Sync from the fresh snapshot, then reload the full detail.
        const fresh = boardTask(ctx.state.detailTask.id);
        if (fresh) {
          ctx.state.detailTask = fresh;
          ctx.drawer.renderDrawerTask(fresh);
        }
        void ctx.drawer.loadTaskDetail(ctx.state.detailTask);
      }
      return true;
    } catch (error) {
      if (ticket.epoch === workspaceController.capture().epoch) {
        showError(error);
        setStatus(failed);
        await loadSnapshot(ctx.state.board?.planId || 0, true);
        ctx.agentActivity.restoreMutationFocus(focusKey);
      }
      return false;
    }
  }

  function boardTask(taskId: number | string): BoardTask | undefined {
    return ctx.state.board?.columns
      ?.flatMap((column) => column.tasks)
      .find((task) => Number(task.id) === Number(taskId));
  }

  function taskTransitionRequestIsCurrent(request: TaskTransitionRequest): boolean {
    return taskTransitionRequest === request &&
      taskTransitionSequence === request.sequence &&
      workspaceController.state.status === "open" &&
      workspaceController.state.generation === request.generation;
  }

  function restoreTaskTransitionControl(request: TaskTransitionRequest): void {
    if (request.invoker instanceof HTMLSelectElement) {
      request.invoker.value = request.fromStatus;
    }
  }

  function focusTaskTransitionOrigin(request: TaskTransitionRequest): void {
    const intent = taskTransitionFocusIntent(
      request.origin,
      !elements.drawer.hidden,
      Boolean(ctx.state.detailTask && Number(ctx.state.detailTask.id) === request.taskId),
    );
    if (intent === "none") return;
    if (intent === "drawer-select") {
      elements.drawerStatusSelect.focus();
      return;
    }
    if (intent === "card-select") {
      // Cards carry no status select any more; the card itself takes focus.
      dragZoneFor(request.taskId)?.focus();
      return;
    }
    if (request.invoker instanceof HTMLElement && request.invoker.isConnected) {
      request.invoker.focus();
      return;
    }
    dragZoneFor(request.taskId)?.focus();
  }

  function closeTaskTransition(
    restoreState = true,
    restoreFocus = true,
    force = false,
  ): void {
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

  async function refreshTaskTransitionView(request: TaskTransitionRequest): Promise<boolean> {
    const refreshed = await loadExactTaskTransitionSnapshot(
      request.planId,
      request.generation,
    );
    if (!refreshed) return false;
    if (workspaceController.state.status !== "open" ||
      workspaceController.state.generation !== request.generation ||
      Number(ctx.state.board?.planId) !== request.planId) return false;
    const fresh = boardTask(request.taskId);
    if (fresh && ctx.state.detailTask && !elements.drawer.hidden &&
      Number(ctx.state.detailTask.id) === request.taskId) {
      ctx.state.detailTask = fresh;
      ctx.drawer.renderDrawerTask(fresh);
      await ctx.drawer.loadTaskDetail(fresh);
    }
    if (workspaceController.state.status !== "open" ||
      workspaceController.state.generation !== request.generation ||
      Number(ctx.state.board?.planId) !== request.planId) return false;
    focusTaskTransitionOrigin(request);
    return true;
  }

  function openTaskTransitionConfirmation(
    request: TaskTransitionRequest,
    confirmation: TaskTransitionConfirmation,
  ): void {
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

  async function moveTask(
    taskId: number,
    status: string,
    invoker: Element | null = document.activeElement,
  ): Promise<void> {
    if (!ctx.state.board || workspaceController.state.status !== "open") return;
    const task = boardTask(taskId);
    if (!task || task.status === status || !isTaskStatus(status)) return;
    if (!taskTransitionCanStart(Boolean(taskTransitionRequest), taskTransitionBusy)) {
      if (invoker instanceof HTMLSelectElement) invoker.value = task.status;
      setStatus("Finish the current task status change before starting another.");
      return;
    }
    const sequence = ++taskTransitionSequence;
    const request: TaskTransitionRequest = {
      sequence,
      generation: workspaceController.state.generation,
      planId: Number(ctx.state.board.planId),
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
      const result: TaskTransitionResult = await api().MoveTaskV3(
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
      // The response check above guarantees a challenge on every
      // unapplied transition.
      const confirmation = result.confirmation;
      if (!confirmation) throw new Error("Stale task transition response ignored");
      openTaskTransitionConfirmation(request, confirmation);
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

  async function confirmTaskTransition(): Promise<void> {
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

  function bind(): void {
    applicationOverlayObserver.observe(document.body, {
      attributes: true,
      attributeFilter: ["hidden"],
      attributeOldValue: true,
      subtree: true,
    });
    nativeEventDisposers.push(() => applicationOverlayObserver.disconnect());
    syncApplicationOverlayState();
    window.addEventListener("focus", () => {
      if (workspaceController.state.status !== "open") return;
      void loadSnapshot(ctx.state.board?.planId || 0, true);
    });
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden && workspaceController.state.status === "open") refreshLoop.resume();
    });
    elements.taskTransitionForm.addEventListener("submit", (event) => {
      event.preventDefault();
      void confirmTaskTransition();
    });
    elements.taskTransitionCancel.addEventListener("click", () => closeTaskTransition());
    document.querySelectorAll("[data-close-task-transition]").forEach((element) => {
      element.addEventListener("click", () => closeTaskTransition());
    });
  }

  return {
    bind,
    refreshLoop,
    runtimeRefreshes,
    snapshotDialogIsOpen,
    hideApplicationOverlay,
    applicationOverlayCoordinator,
    loadSnapshot,
    discardSnapshotForProjectChange,
    cancelSnapshotsForClose,
    runMutation,
    boardTask,
    closeTaskTransition,
    moveTask,
  };
}

export type SnapshotController = ReturnType<typeof createSnapshotController>;
