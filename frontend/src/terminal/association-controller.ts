import type { AppContext } from "../workspace/app-context";
import { element } from "../workspace/dom";
import { messageFrom } from "../workspace/format";
import type { AssociationPointerV1 } from "../workspace/model";
import type { Board } from "../workspace/snapshot-types";
import type { ActiveTerminalAssociation } from "./association-editor";
import { linkedAssociationPointer } from "./linked-launch";
import type { TerminalDockHandle } from "./pane";

/** One choice in the association editor: the current plan or one of its tasks. */
export interface AssociationTarget {
  value: string;
  label: string;
  association: AssociationPointerV1;
}

interface AssociationRequest {
  active: ActiveTerminalAssociation;
  generation: number;
  handle: TerminalDockHandle;
  sequence: number;
  targets: AssociationTarget[];
}

/** What a terminal can be linked to: the board's plan and every task on it. */
export function terminalAssociationTargets(board: Board | null): AssociationTarget[] {
  if (!board?.planId) return [];
  const planId = Number(board.planId);
  const targets: AssociationTarget[] = [{
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

export function createAssociationController(ctx: AppContext) {
  const { setStatus, showError, workspaceController } = ctx;
  const elements = {
    terminalAssociationCancel: element("#terminal-association-cancel", HTMLButtonElement),
    terminalAssociationDetach: element("#terminal-association-detach", HTMLButtonElement),
    terminalAssociationDetail: element("#terminal-association-detail", HTMLParagraphElement),
    terminalAssociationForm: element("#terminal-association-form", HTMLFormElement),
    terminalAssociationHeading: element("#terminal-association-heading", HTMLHeadingElement),
    terminalAssociationMessage: element("#terminal-association-message", HTMLParagraphElement),
    terminalAssociationModal: element("#terminal-association-modal", HTMLDivElement),
    terminalAssociationSubmit: element("#terminal-association-submit", HTMLButtonElement),
    terminalAssociationTarget: element("#terminal-association-target", HTMLSelectElement),
    terminalLinkContext: element("#terminal-link-context", HTMLButtonElement),
  };

  let terminalAssociationRequest: AssociationRequest | null = null;
  let terminalAssociationReturnFocus: HTMLElement | null = null;
  let terminalAssociationSequence = 0;
  let terminalAssociationBusy = false;

  function openTerminalAssociationEditor(invoker: Element | null = document.activeElement): void {
    if (workspaceController.state.status !== "open" || !ctx.state.terminalHandle) return;
    const active = ctx.state.terminalHandle.associationState();
    if (!active || active.generation !== workspaceController.state.generation) {
      showError(new Error("A live single-pane terminal tab is required"));
      return;
    }
    closeTerminalAssociationEditor(false, true);
    const sequence = ++terminalAssociationSequence;
    const targets = terminalAssociationTargets(ctx.state.board);
    terminalAssociationReturnFocus = invoker instanceof HTMLElement ? invoker : null;
    terminalAssociationRequest = {
      active,
      generation: active.generation,
      handle: ctx.state.terminalHandle,
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

  function closeTerminalAssociationEditor(restoreFocus = true, force = false): void {
    if (terminalAssociationBusy && !force) return;
    terminalAssociationSequence += 1;
    terminalAssociationBusy = false;
    terminalAssociationRequest = null;
    ctx.snapshot.hideApplicationOverlay(elements.terminalAssociationModal);
    elements.terminalAssociationTarget.disabled = true;
    elements.terminalAssociationCancel.disabled = false;
    elements.terminalAssociationDetach.disabled = true;
    elements.terminalAssociationSubmit.disabled = true;
    if (restoreFocus) terminalAssociationReturnFocus?.focus?.();
    terminalAssociationReturnFocus = null;
  }

  async function submitTerminalAssociation(detach = false): Promise<void> {
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
          ctx.state.terminalHandle === request.handle,
      );
      if (
        terminalAssociationSequence !== request.sequence ||
        workspaceController.state.status !== "open" ||
        workspaceController.state.generation !== request.generation ||
        ctx.state.terminalHandle !== request.handle ||
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

  function bind(): void {
    elements.terminalLinkContext.addEventListener("click", () => {
      openTerminalAssociationEditor(elements.terminalLinkContext);
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
    document.querySelectorAll("[data-close-terminal-association]").forEach((closer) => {
      closer.addEventListener("click", () => closeTerminalAssociationEditor());
    });
  }

  return {
    bind,
    closeTerminalAssociationEditor,
  };
}

export type AssociationController = ReturnType<typeof createAssociationController>;
