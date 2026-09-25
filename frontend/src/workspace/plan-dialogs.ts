import type { AppContext } from "./app-context";
import type { MenuPosition } from "./board-view";
import { element } from "./dom";
import { validateOnboardingTitle } from "./first-plan";
import { messageFrom } from "./format";
import {
  clampMenuPosition,
  completionPromptMode,
  deleteConfirmationText,
  isTextEntryElement,
  planMenuItems,
  planReadyForCompletion,
  transferSubmitDisabled,
  type CompletionPromptContext,
  type PlanLifecycleAction,
  type TransferDialogState,
} from "./plan-lifecycle";
import type {
  BoardPlan,
  BoardTask,
  GenerationReply,
  PlanCompletionResponse,
} from "./snapshot-types";
import { statusTitles, statuses } from "./task-status";

/** What the plan dialog is currently asking. */
type PlanDialogMode = "create" | "done" | "checkpoint" | "hold" | "delete" | "move" | "copy";

/** The plan dialog modes that submit a lifecycle change for an existing plan. */
type PlanLifecycleMode = Exclude<PlanDialogMode, "create" | "checkpoint">;

type LifecycleReply = GenerationReply & Partial<Pick<PlanCompletionResponse, "checkpoint">>;

export interface ContextMenuEntry {
  label: string;
  destructive?: boolean;
  onSelect(): void;
}

function transferMode(action: PlanLifecycleAction): "move" | "copy" | null {
  return action === "move" || action === "copy" ? action : null;
}

function focusedHTMLElement(): HTMLElement | null {
  const active = document.activeElement;
  return active instanceof HTMLElement ? active : null;
}

export function createPlanDialogs(ctx: AppContext) {
  const { api, setStatus, showError, workspaceController } = ctx;
  const elements = {
    dialogEyebrow: element("#dialog-eyebrow", HTMLParagraphElement),
    dialogForm: element("#dialog-form", HTMLFormElement),
    dialogHeading: element("#dialog-heading", HTMLHeadingElement),
    dialogHelp: element("#dialog-help", HTMLParagraphElement),
    dialogInput: element("#dialog-input", HTMLInputElement),
    dialogLabel: element("#dialog-label", HTMLLabelElement),
    dialogNote: element("#dialog-note", HTMLTextAreaElement),
    dialogSubmit: element("#dialog-submit", HTMLButtonElement),
    modal: element("#modal", HTMLDivElement),
    planDialog: element("#plan-dialog", HTMLDivElement),
    planDialogBody: element("#plan-dialog-body", HTMLParagraphElement),
    planDialogCancel: element("#plan-dialog-cancel", HTMLButtonElement),
    planDialogError: element("#plan-dialog-error", HTMLParagraphElement),
    planDialogEyebrow: element("#plan-dialog-eyebrow", HTMLParagraphElement),
    planDialogForm: element("#plan-dialog-form", HTMLFormElement),
    planDialogHeading: element("#plan-dialog-heading", HTMLHeadingElement),
    planDialogProject: element("#plan-dialog-project", HTMLSelectElement),
    planDialogProjectLabel: element("#plan-dialog-project-label", HTMLLabelElement),
    planDialogSubmit: element("#plan-dialog-submit", HTMLButtonElement),
    planDialogTitle: element("#plan-dialog-title", HTMLInputElement),
    planDialogTitleLabel: element("#plan-dialog-title-label", HTMLLabelElement),
    taskTitle: element("#task-title", HTMLInputElement),
  };

  // ------------------------------------------------ rename / note dialog
  let editingTask: BoardTask | null = null;
  let dialogMode: "rename" | "memory" = "rename";
  let dialogReturnFocus: HTMLElement | null = null;

  // ---------------------------------------------------------- context menu
  let contextMenu: HTMLElement | null = null;
  let contextMenuDispose: (() => void) | null = null;
  let contextMenuReturnFocus: HTMLElement | null = null;
  let planRenameActive = false;

  // ----------------------------------------------------------- plan dialog
  let planDialogMode: PlanDialogMode | null = null;
  let planDialogDeleteRevision = "";
  let planCreateSequence = 0;
  let planDialogPlan: BoardPlan | null = null;
  let planDialogTransferState: TransferDialogState | null = null;
  let planDialogReturnFocus: HTMLElement | null = null;
  // Bumped whenever the plan dialog opens or closes, so a pending submit can
  // tell that the dialog it started from is gone or was replaced.
  let planDialogToken = 0;
  const promptedCompletedPlans = new Set<string>();
  let planCloseoutBanner: { element: HTMLElement; key: string } | null = null;

  function openRename(task: BoardTask): void {
    dialogMode = "rename";
    editingTask = task;
    dialogReturnFocus = focusedHTMLElement();
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

  function openMemory(task: BoardTask): void {
    dialogMode = "memory";
    editingTask = task;
    dialogReturnFocus = focusedHTMLElement();
    elements.dialogEyebrow.textContent = "Note";
    elements.dialogHeading.textContent = `Add note to task #${task.id}`;
    elements.dialogLabel.textContent = "Decision or observation";
    elements.dialogLabel.htmlFor = "dialog-note";
    elements.dialogInput.hidden = true;
    elements.dialogNote.value = "";
    elements.dialogNote.hidden = false;
    elements.dialogHelp.textContent =
      "Capture a decision, constraint, or durable observation—not a narration of routine work.";
    elements.dialogSubmit.textContent = "Add note";
    elements.modal.hidden = false;
    requestAnimationFrame(() => elements.dialogNote.focus());
  }

  function closeDialog(): void {
    if (elements.modal.hidden) return;
    editingTask = null;
    ctx.snapshot.hideApplicationOverlay(elements.modal);
    dialogReturnFocus?.focus?.();
    dialogReturnFocus = null;
  }

  async function submitTaskDialog(): Promise<void> {
    if (!editingTask) return;
    const task = editingTask;
    if (dialogMode === "rename") {
      const title = elements.dialogInput.value.trim();
      if (!title) return;
      closeDialog();
      await ctx.snapshot.runMutation(
        (generation) => api().RenameTaskV2(generation, Number(task.id), title),
        `Renaming task #${task.id}…`,
        `Could not rename task #${task.id}`,
      );
      return;
    }
    const note = elements.dialogNote.value.trim();
    if (!note) return;
    closeDialog();
    await ctx.snapshot.runMutation(
      (generation) => api().AddTaskNoteV2(generation, Number(task.id), note),
      `Adding note to task #${task.id}…`,
      `Could not add note to task #${task.id}`,
    );
  }

  // ------------------------------------------------------ plan lifecycle

  function closeContextMenu(): void {
    if (!contextMenu) return;
    const menu = contextMenu;
    contextMenu = null;
    contextMenuDispose?.();
    contextMenuDispose = null;
    menu.remove();
    contextMenuReturnFocus?.focus?.();
    contextMenuReturnFocus = null;
  }

  // A plan title being renamed in place or an open context menu owns the
  // pointer; a quiet refresh would tear either out from under it.
  function inlinePlanEditActive(): boolean {
    return planRenameActive || contextMenu !== null;
  }

  function contextMenuButton(entry: ContextMenuEntry): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.setAttribute("role", "menuitem");
    button.textContent = entry.label;
    if (entry.destructive) button.classList.add("context-menu-destructive");
    button.addEventListener("click", () => {
      closeContextMenu();
      entry.onSelect();
    });
    return button;
  }

  // Outside click, Escape, and focus leaving the menu all close it.
  function bindContextMenuDismissal(menu: HTMLElement): void {
    const onOutsideClick = (event: MouseEvent) => {
      if (!(event.target instanceof Node) || !menu.contains(event.target)) closeContextMenu();
    };
    const onKeydown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      closeContextMenu();
    };
    const onFocusOut = (event: FocusEvent) => {
      // WKWebView blurs without relatedTarget on menu mousedown; wait for click.
      // The document listener handles outside clicks.
      if (!event.relatedTarget) return;
      if (event.relatedTarget instanceof Node && menu.contains(event.relatedTarget)) return;
      closeContextMenu();
    };
    // Deferred so the click/contextmenu event that opened the menu doesn't
    // also register as the outside click that closes it.
    window.setTimeout(() => document.addEventListener("click", onOutsideClick), 0);
    document.addEventListener("keydown", onKeydown);
    menu.addEventListener("focusout", onFocusOut);
    contextMenuDispose = () => {
      document.removeEventListener("click", onOutsideClick);
      document.removeEventListener("keydown", onKeydown);
      menu.removeEventListener("focusout", onFocusOut);
    };
  }

  // Shared context-menu plumbing for plans and tasks: positions a list of
  // { label, destructive?, onSelect } entries, clamps to the viewport, and
  // cleans up on outside click, Escape, or focus loss.
  function openContextMenu(
    entries: readonly ContextMenuEntry[],
    position: MenuPosition,
    invoker: Element | null,
  ): void {
    closeContextMenu();
    const menu = document.createElement("div");
    menu.className = "context-menu";
    menu.setAttribute("role", "menu");
    menu.style.visibility = "hidden";
    entries.forEach((entry) => menu.append(contextMenuButton(entry)));
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
    contextMenu = menu;
    contextMenuReturnFocus = invoker instanceof HTMLElement ? invoker : null;
    requestAnimationFrame(() => {
      menu.querySelector("button")?.focus();
    });
    bindContextMenuDismissal(menu);
  }

  function runPlanMenuAction(action: PlanLifecycleAction, plan: BoardPlan, titleElement: HTMLElement): void {
    if (action === "copy-context") void ctx.board.copyAgentContext("plan", null, null, plan);
    else if (action === "rename") beginPlanRename(titleElement, plan);
    else if (action === "done") openPlanDoneDialog(plan);
    else if (action === "hold") openPlanHoldDialog(plan);
    else if (action === "resume") void resumePlan(plan);
    else if (action === "reopen") void reopenPlan(plan);
    else if (action === "delete") void openPlanDeleteDialog(plan);
    else {
      const mode = transferMode(action);
      if (mode) void openPlanTransferDialog(plan, mode);
    }
  }

  function openPlanContextMenu(
    plan: BoardPlan,
    titleElement: HTMLElement,
    invoker: Element | null,
    position: MenuPosition,
  ): void {
    openContextMenu(
      planMenuItems(plan).map((item) => ({
        label: item.label,
        destructive: item.destructive,
        onSelect: () => runPlanMenuAction(item.action, plan, titleElement),
      })),
      position,
      invoker,
    );
  }

  function openTaskContextMenu(
    task: BoardTask,
    position: MenuPosition,
    invoker: Element | null = null,
  ): void {
    const dragZone = () => document.querySelector(`.card[data-task-id="${task.id}"] .card-drag-zone`);
    // Keyboard path for a status change: one "Move to …" entry per other lane.
    const moves = statuses
      .filter((status) => status !== task.status)
      .map((status) => ({
        label: `Move to ${statusTitles[status]}`,
        onSelect: () => void ctx.snapshot.moveTask(task.id, status, dragZone()),
      }));
    openContextMenu(
      [
        { label: "Open details", onSelect: () => ctx.drawer.openTaskDetail(task) },
        ...moves,
        {
          label: "Launch agent",
          onSelect: () => void ctx.agentLaunch.openAgentLaunchPicker(
            { planId: Number(ctx.state.board?.planId), task },
            document.activeElement,
          ),
        },
        { label: "Copy context", onSelect: () => void ctx.board.copyAgentContext("task", task, null) },
        { label: "Edit", onSelect: () => openRename(task) },
        { label: "Add note", onSelect: () => openMemory(task) },
      ],
      position,
      invoker,
    );
  }

  function beginPlanRename(titleElement: HTMLElement, plan: BoardPlan): void {
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
        await ctx.snapshot.loadSnapshot(ctx.state.board?.planId || 0);
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

  function planDialogBusy(): boolean {
    return elements.planDialogForm.getAttribute("aria-busy") === "true";
  }

  // A submit in flight owns the dialog: Escape, Cancel and the backdrop all go
  // through closePlanDialog, which refuses while the form is busy.
  function setPlanDialogBusy(busy: boolean): void {
    if (busy) elements.planDialogForm.setAttribute("aria-busy", "true");
    else elements.planDialogForm.removeAttribute("aria-busy");
    elements.planDialogTitle.readOnly = busy;
    elements.planDialogProject.disabled = busy;
    elements.planDialogCancel.disabled = busy;
    if (busy) elements.planDialogSubmit.disabled = true;
  }

  // Closes the dialog even mid-submit: the workspace it belonged to is gone.
  function abandonPlanDialog(): void {
    if (elements.planDialog.hidden) return;
    planCreateSequence += 1;
    setPlanDialogBusy(false);
    closePlanDialog();
  }

  function resetPlanDialogChrome(): void {
    elements.planDialogError.hidden = true;
    elements.planDialogError.textContent = "";
    elements.planDialogBody.classList.remove("plan-dialog-checkpoint");
    elements.planDialogCancel.hidden = false;
    elements.planDialogCancel.textContent = "Cancel";
    elements.planDialogSubmit.classList.remove("dialog-danger");
  }

  function closePlanDialog(): void {
    if (planDialogBusy()) return;
    if (elements.planDialog.hidden) return;
    planDialogToken += 1;
    ctx.snapshot.hideApplicationOverlay(elements.planDialog);
    planDialogReturnFocus?.focus?.();
    planDialogReturnFocus = null;
    planDialogMode = null;
    planDialogPlan = null;
    planDialogTransferState = null;
    planDialogDeleteRevision = "";
    elements.planDialogBody.classList.remove("plan-dialog-checkpoint");
    elements.planDialogError.hidden = true;
    elements.planDialogError.textContent = "";
    elements.planDialogCancel.hidden = false;
    elements.planDialogCancel.textContent = "Cancel";
    elements.planDialogSubmit.classList.remove("dialog-danger");
  }

  function setPlanDialogError(error: unknown): void {
    elements.planDialogError.textContent = messageFrom(error);
    elements.planDialogError.hidden = false;
  }

  function openPlanDialogShell(): void {
    planDialogToken += 1;
    hidePlanCloseoutBanner();
    planDialogReturnFocus = focusedHTMLElement();
    resetPlanDialogChrome();
    elements.planDialog.hidden = false;
  }

  function hidePlanDialogFields(): void {
    elements.planDialogProjectLabel.hidden = true;
    elements.planDialogProject.hidden = true;
    elements.planDialogTitleLabel.hidden = true;
    elements.planDialogTitle.hidden = true;
  }

  function showPlanDialogTitleField(label: string): void {
    elements.planDialogTitleLabel.textContent = label;
    elements.planDialogTitleLabel.hidden = false;
    elements.planDialogTitle.value = "";
    elements.planDialogTitle.hidden = false;
  }

  function completionPromptKey(plan: BoardPlan): string {
    return `${workspaceController.state.generation}:${Number(plan.id)}`;
  }

  // The prompt fires from refreshes, window focus and watcher events, so it is
  // always automatic. It opens the dialog only when nobody is typing; otherwise
  // it leaves a non-modal banner, because a modal would catch the next Enter
  // meant for a shell or a field.
  function maybePromptForPlanCompletion(): void {
    const plan = ctx.board.currentBoardPlan();
    if (!plan) {
      hidePlanCloseoutBanner();
      return;
    }
    const key = completionPromptKey(plan);
    if (planCloseoutBanner && planCloseoutBanner.key !== key) hidePlanCloseoutBanner();
    if (!planReadyForCompletion(plan)) {
      promptedCompletedPlans.delete(key);
      hidePlanCloseoutBanner();
      return;
    }
    if (promptedCompletedPlans.has(key) || ctx.snapshot.snapshotDialogIsOpen()) return;
    promptedCompletedPlans.add(key);
    if (completionPromptMode(completionPromptContext()) === "banner") {
      showPlanCloseoutBanner(plan, key);
    } else {
      openPlanDoneDialog(plan, true);
    }
  }

  function completionPromptContext(): CompletionPromptContext {
    const active = document.activeElement;
    return {
      automatic: true,
      windowFocused: document.hasFocus(),
      focusInTerminal: Boolean(active?.closest?.("#terminal-dock")),
      focusInTextEntry: isTextEntryElement(active instanceof HTMLElement ? active : null),
    };
  }

  function showPlanCloseoutBanner(plan: BoardPlan, key: string): void {
    hidePlanCloseoutBanner();
    const banner = document.createElement("div");
    banner.className = "plan-closeout-banner";
    banner.setAttribute("role", "status");
    banner.setAttribute("aria-live", "polite");
    const message = document.createElement("span");
    message.textContent = `Every task in “${plan.title}” is done. `;
    const review = document.createElement("button");
    review.type = "button";
    review.className = "button-secondary";
    review.textContent = "Review plan closeout";
    review.addEventListener("click", () => {
      // The banner sits outside the inert app shell; another dialog wins.
      if (ctx.snapshot.snapshotDialogIsOpen()) return;
      hidePlanCloseoutBanner();
      const current = ctx.board.currentBoardPlan();
      if (current && completionPromptKey(current) === key && planReadyForCompletion(current)) {
        openPlanDoneDialog(current, true);
      }
    });
    const dismiss = document.createElement("button");
    dismiss.type = "button";
    dismiss.className = "button-secondary";
    dismiss.textContent = "Dismiss";
    dismiss.setAttribute("aria-label", "Dismiss plan closeout reminder");
    dismiss.addEventListener("click", hidePlanCloseoutBanner);
    banner.append(message, review, " ", dismiss);
    (document.querySelector("#notice-stack") ?? document.body).prepend(banner);
    planCloseoutBanner = { element: banner, key };
  }

  function hidePlanCloseoutBanner(): void {
    planCloseoutBanner?.element.remove();
    planCloseoutBanner = null;
  }

  function openNewPlanDialog(): void {
    if (workspaceController.state.status !== "open" || ctx.state.firstPlanState.phase !== "idle") return;
    planDialogMode = "create";
    planDialogPlan = null;
    planDialogTransferState = null;
    openPlanDialogShell();
    hidePlanDialogFields();
    elements.planDialogEyebrow.textContent = "Plans";
    elements.planDialogHeading.textContent = "Create a new plan";
    elements.planDialogBody.textContent = "Give this plan a title. You can add tasks on its board next.";
    showPlanDialogTitleField("Plan title");
    elements.planDialogSubmit.textContent = "Create plan";
    elements.planDialogSubmit.disabled = true;
    requestAnimationFrame(() => elements.planDialogTitle.focus());
  }

  async function submitNewPlan(): Promise<void> {
    if (planDialogBusy()) return;
    const validation = validateOnboardingTitle(elements.planDialogTitle.value, "plan");
    if (validation.error) {
      setPlanDialogError(validation.error);
      elements.planDialogTitle.focus();
      return;
    }
    const ticket = workspaceController.capture();
    const sequence = ++planCreateSequence;
    setPlanDialogBusy(true);
    elements.planDialogError.hidden = true;
    try {
      const response = await api().AddPlanV1(ticket.generation, validation.value);
      if (sequence !== planCreateSequence || !workspaceController.accepts(ticket, Number(response.generation))) return;
      setPlanDialogBusy(false);
      closePlanDialog();
      ctx.board.clearPlanFilters();
      ctx.shell.setView("board");
      await ctx.snapshot.loadSnapshot(Number(response.plan.id));
      elements.taskTitle.focus();
    } catch (error) {
      if (sequence === planCreateSequence && workspaceController.accepts(ticket, ticket.generation)) setPlanDialogError(error);
    } finally {
      if (sequence === planCreateSequence) {
        setPlanDialogBusy(false);
        if (planDialogMode === "create") syncPlanDialogState();
      }
    }
  }

  function openPlanDoneDialog(plan: BoardPlan, automatic = false): void {
    if (workspaceController.state.status !== "open") return;
    if (planReadyForCompletion(plan)) {
      promptedCompletedPlans.add(completionPromptKey(plan));
    }
    const remaining = Math.max(0, Number(plan.tasksTotal) - Number(plan.tasksDone));
    planDialogMode = "done";
    planDialogPlan = plan;
    planDialogTransferState = null;
    openPlanDialogShell();
    elements.planDialogEyebrow.textContent = "Complete plan";
    elements.planDialogHeading.textContent = `Mark “${plan.title}” done?`;
    elements.planDialogBody.textContent = remaining === 0
      ? `All ${plan.tasksTotal} task${Number(plan.tasksTotal) === 1 ? " is" : "s are"} done. Close the plan and review the project checkpoint before continuing.`
      : `${remaining} open task${remaining === 1 ? " remains" : "s remain"}. Finish every task before closing this plan.`;
    hidePlanDialogFields();
    elements.planDialogCancel.textContent = automatic ? "Not now" : "Cancel";
    elements.planDialogSubmit.textContent = "Mark plan done";
    elements.planDialogSubmit.disabled = remaining !== 0;
    // Never the submit button: closing a plan takes a deliberate click or Tab,
    // not a stray Enter.
    requestAnimationFrame(() => elements.planDialogCancel.focus());
  }

  function openPlanHoldDialog(plan: BoardPlan): void {
    if (workspaceController.state.status !== "open") return;
    planDialogMode = "hold";
    planDialogPlan = plan;
    planDialogTransferState = null;
    openPlanDialogShell();
    elements.planDialogEyebrow.textContent = "Put plan on hold";
    elements.planDialogHeading.textContent = `Pause “${plan.title}”`;
    elements.planDialogBody.textContent =
      "Record why work is paused. The plan remains visible but will not prompt for completion until resumed.";
    elements.planDialogProjectLabel.hidden = true;
    elements.planDialogProject.hidden = true;
    showPlanDialogTitleField("Hold reason");
    elements.planDialogSubmit.textContent = "Put on hold";
    elements.planDialogSubmit.disabled = true;
    requestAnimationFrame(() => elements.planDialogTitle.focus());
  }

  function openPlanCheckpointDialog(plan: BoardPlan, checkpoint: PlanCompletionResponse["checkpoint"]): void {
    planDialogMode = "checkpoint";
    planDialogPlan = null;
    planDialogTransferState = null;
    openPlanDialogShell();
    elements.planDialogEyebrow.textContent = "Plan complete";
    elements.planDialogHeading.textContent = `“${plan.title}” is done`;
    elements.planDialogBody.textContent = checkpoint.markdown;
    elements.planDialogBody.classList.add("plan-dialog-checkpoint");
    hidePlanDialogFields();
    elements.planDialogCancel.hidden = true;
    elements.planDialogSubmit.textContent = "Close checkpoint";
    elements.planDialogSubmit.disabled = false;
    requestAnimationFrame(() => elements.planDialogSubmit.focus());
  }

  // Resume and reopen are one call each, with the plan refreshed after.
  async function changePlanState(
    plan: BoardPlan,
    request: (generation: number, planId: number) => Promise<GenerationReply>,
    progress: string,
    done: string,
    failed: string,
  ): Promise<void> {
    if (workspaceController.state.status !== "open") return;
    const ticket = workspaceController.capture();
    setStatus(progress);
    try {
      const response = await request(ticket.generation, Number(plan.id));
      if (!workspaceController.accepts(ticket, Number(response.generation))) return;
      await ctx.snapshot.loadSnapshot(Number(plan.id));
      setStatus(done);
    } catch (error) {
      showError(error);
      setStatus(failed);
    }
  }

  function resumePlan(plan: BoardPlan): Promise<void> {
    return changePlanState(
      plan,
      (generation, planId) => api().ResumePlanV1(generation, planId),
      `Resuming plan #${plan.id}…`,
      `Plan #${plan.id} resumed.`,
      `Could not resume plan #${plan.id}`,
    );
  }

  function reopenPlan(plan: BoardPlan): Promise<void> {
    return changePlanState(
      plan,
      (generation, planId) => api().ReopenPlanV1(generation, planId),
      `Reopening plan #${plan.id}…`,
      `Plan #${plan.id} reopened.`,
      `Could not reopen plan #${plan.id}`,
    );
  }

  async function openPlanDeleteDialog(plan: BoardPlan): Promise<void> {
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
    // The confirmation is bound to exactly what this preview counted: the
    // runtime refuses the delete if the plan changed since.
    planDialogDeleteRevision = String(response.previewRevision || "");
    openPlanDialogShell();
    elements.planDialogEyebrow.textContent = "Delete plan";
    elements.planDialogHeading.textContent = `Delete “${response.summary.title}”?`;
    elements.planDialogBody.textContent = deleteConfirmationText(response.summary);
    hidePlanDialogFields();
    elements.planDialogSubmit.textContent = "Delete plan";
    elements.planDialogSubmit.classList.add("dialog-danger");
    elements.planDialogSubmit.disabled = false;
    requestAnimationFrame(() => elements.planDialogCancel.focus());
  }

  function syncPlanDialogState(): void {
    if (planDialogMode === "create") {
      elements.planDialogSubmit.disabled = elements.planDialogForm.getAttribute("aria-busy") === "true" || elements.planDialogTitle.value.trim() === "";
      return;
    }
    if (planDialogMode === "hold") {
      elements.planDialogSubmit.disabled = elements.planDialogTitle.value.trim() === "";
      return;
    }
    if (planDialogTransferState) {
      planDialogTransferState.targetPath = elements.planDialogProject.value;
      planDialogTransferState.title = elements.planDialogTitle.value;
      elements.planDialogSubmit.disabled = transferSubmitDisabled(planDialogTransferState);
    }
  }

  function renderTransferProjects(projects: TransferDialogState["projects"]): void {
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
  }

  async function openPlanTransferDialog(plan: BoardPlan, mode: "move" | "copy"): Promise<void> {
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
    renderTransferProjects(projects);
    elements.planDialogTitle.value = "";
    elements.planDialogProjectLabel.hidden = false;
    elements.planDialogProject.hidden = false;
    elements.planDialogTitleLabel.textContent = "New title (optional)";
    elements.planDialogTitleLabel.hidden = false;
    elements.planDialogTitle.hidden = false;
    elements.planDialogSubmit.textContent = "OK";
    syncPlanDialogState();
    requestAnimationFrame(() => elements.planDialogProject.focus());
  }

  function planLifecycleRequest(
    mode: PlanLifecycleMode,
    generation: number,
    plan: BoardPlan,
    transfer: TransferDialogState | null,
    holdReason: string,
    deleteRevision: string,
  ): Promise<LifecycleReply> {
    const planId = Number(plan.id);
    if (mode === "done") return api().CompletePlanV1(generation, planId);
    if (mode === "hold") return api().HoldPlanV1(generation, planId, holdReason);
    if (mode === "delete") return api().DeletePlanV1(generation, planId, true, deleteRevision);
    const title = transfer?.title.trim() ?? "";
    const targetPath = transfer?.targetPath ?? "";
    return mode === "move"
      ? api().MovePlanV1(generation, planId, targetPath, title)
      : api().CopyPlanV1(generation, planId, targetPath, title);
  }

  function lifecycleMode(mode: PlanDialogMode | null): PlanLifecycleMode | null {
    return mode === null || mode === "create" || mode === "checkpoint" ? null : mode;
  }

  // Every lifecycle mode captures what it submits before the request, keeps the
  // dialog busy until it answers, and afterwards touches the dialog only if it
  // is still the one that submitted (the token). An error with no dialog left
  // to hold it goes to the toast instead of vanishing.
  async function submitPlanLifecycle(): Promise<void> {
    const mode = lifecycleMode(planDialogMode);
    if (planDialogBusy() || !planDialogPlan || !mode) return;
    const plan = planDialogPlan;
    const transfer = planDialogTransferState ? { ...planDialogTransferState } : null;
    const holdReason = elements.planDialogTitle.value.trim();
    const deleteRevision = planDialogDeleteRevision;
    if ((mode === "move" || mode === "copy") && (!transfer || transferSubmitDisabled(transfer))) return;
    if (mode === "hold" && !holdReason) return;
    const token = planDialogToken;
    const dialogCurrent = () => token === planDialogToken && !elements.planDialog.hidden;
    const ticket = workspaceController.capture();
    elements.planDialogError.hidden = true;
    setPlanDialogBusy(true);
    let response: LifecycleReply;
    try {
      response = await planLifecycleRequest(
        mode, ticket.generation, plan, transfer, holdReason, deleteRevision,
      );
    } catch (error) {
      if (dialogCurrent()) {
        setPlanDialogBusy(false);
        setPlanDialogError(error);
        elements.planDialogSubmit.disabled = mode === "hold"
          ? elements.planDialogTitle.value.trim() === ""
          : transfer ? transferSubmitDisabled(transfer) : false;
      } else if (workspaceController.isCurrent(ticket)) {
        showError(error);
      }
      return;
    }
    if (dialogCurrent()) {
      setPlanDialogBusy(false);
      closePlanDialog();
    }
    if (!workspaceController.accepts(ticket, Number(response?.generation))) return;
    if (mode === "done" && response.checkpoint) {
      const nextPlan = response.checkpoint.openPlans
        ?.find((candidate) => Number(candidate.id) !== Number(plan.id));
      if (elements.planDialog.hidden) openPlanCheckpointDialog(plan, response.checkpoint);
      await ctx.snapshot.loadSnapshot(nextPlan ? Number(nextPlan.id) : null);
      return;
    }
    await ctx.snapshot.loadSnapshot(mode === "hold" ? Number(plan.id) : 0);
  }

  function bind(): void {
    elements.planDialogProject.addEventListener("input", syncPlanDialogState);
    elements.planDialogProject.addEventListener("change", syncPlanDialogState);
    elements.planDialogTitle.addEventListener("input", syncPlanDialogState);
    elements.planDialogForm.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      closePlanDialog();
    });
    elements.planDialogForm.addEventListener("submit", async (event) => {
      event.preventDefault();
      if (planDialogMode === "checkpoint") {
        closePlanDialog();
        return;
      }
      if (planDialogMode === "create") {
        await submitNewPlan();
        return;
      }
      await submitPlanLifecycle();
    });
    document.querySelectorAll("[data-close-plan-dialog]").forEach((closer) => {
      closer.addEventListener("click", closePlanDialog);
    });
    elements.dialogForm.addEventListener("submit", (event) => {
      event.preventDefault();
      void submitTaskDialog();
    });
    document.querySelectorAll("[data-close-modal]").forEach((closer) => {
      closer.addEventListener("click", closeDialog);
    });
  }

  return {
    bind,
    openRename,
    openMemory,
    closeDialog,
    inlinePlanEditActive,
    openPlanContextMenu,
    openTaskContextMenu,
    abandonPlanDialog,
    maybePromptForPlanCompletion,
    hidePlanCloseoutBanner,
    openNewPlanDialog,
    openPlanDoneDialog,
  };
}

export type PlanDialogs = ReturnType<typeof createPlanDialogs>;
