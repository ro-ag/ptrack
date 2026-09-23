import type { AppContext } from "../workspace/app-context";
import { element } from "../workspace/dom";
import { messageFrom } from "../workspace/format";
import type { ActiveTerminalAssociation } from "./association-editor";
import type { TerminalDockHandle } from "./pane";
import {
  isTerminalWritebackKind,
  stableTerminalWritebackRequestID,
  terminalWritebackContentPolicy,
  type TerminalWritebackPreview,
} from "./writeback";

interface WritebackRequest {
  active: ActiveTerminalAssociation;
  generation: number;
  handle: TerminalDockHandle;
  sequence: number;
  preview: TerminalWritebackPreview | null;
  requestID: string | null;
}

/** Where a write-back lands, named from the terminal's live association. */
export function terminalWritebackAssociationLabel(active: ActiveTerminalAssociation): string {
  if (active.pointer?.taskId) return `Task #${active.pointer.taskId}`;
  if (active.pointer?.planId) return `Plan #${active.pointer.planId}`;
  return active.pointer ? "Project" : "Detached terminal";
}

export function createWritebackController(ctx: AppContext) {
  const { setStatus, showError, workspaceController } = ctx;
  const elements = {
    terminalWriteback: element("#terminal-writeback", HTMLButtonElement),
    terminalWritebackCancel: element("#terminal-writeback-cancel", HTMLButtonElement),
    terminalWritebackContent: element("#terminal-writeback-content", HTMLTextAreaElement),
    terminalWritebackForm: element("#terminal-writeback-form", HTMLFormElement),
    terminalWritebackKind: element("#terminal-writeback-kind", HTMLSelectElement),
    terminalWritebackMessage: element("#terminal-writeback-message", HTMLParagraphElement),
    terminalWritebackModal: element("#terminal-writeback-modal", HTMLDivElement),
    terminalWritebackPreview: element("#terminal-writeback-preview", HTMLElement),
    terminalWritebackPreviewButton: element("#terminal-writeback-preview-button", HTMLButtonElement),
    terminalWritebackPreviewContent: element("#terminal-writeback-preview-content", HTMLParagraphElement),
    terminalWritebackPreviewTarget: element("#terminal-writeback-preview-target", HTMLElement),
    terminalWritebackSave: element("#terminal-writeback-save", HTMLButtonElement),
    terminalWritebackSummaryConfirm: element("#terminal-writeback-summary-confirm", HTMLInputElement),
    terminalWritebackSummaryWarning: element("#terminal-writeback-summary-warning", HTMLLabelElement),
    terminalWritebackTarget: element("#terminal-writeback-target", HTMLParagraphElement),
  };

  let terminalWritebackRequest: WritebackRequest | null = null;
  let terminalWritebackReturnFocus: HTMLElement | null = null;
  let terminalWritebackSequence = 0;
  let terminalWritebackBusy = false;

  function invalidateTerminalWritebackPreview(): void {
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

  function openTerminalWriteback(invoker: Element | null = document.activeElement): void {
    if (workspaceController.state.status !== "open" || !ctx.state.terminalHandle) return;
    const active = ctx.state.terminalHandle.associationState();
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
      handle: ctx.state.terminalHandle,
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

  function closeTerminalWriteback(restoreFocus = true, force = false): void {
    if (terminalWritebackBusy && !force) return;
    terminalWritebackSequence += 1;
    terminalWritebackBusy = false;
    terminalWritebackRequest = null;
    ctx.snapshot.hideApplicationOverlay(elements.terminalWritebackModal);
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

  function terminalWritebackRequestIsCurrent(request: WritebackRequest): boolean {
    return terminalWritebackSequence === request.sequence &&
      !elements.terminalWritebackModal.hidden &&
      workspaceController.state.status === "open" &&
      workspaceController.state.generation === request.generation &&
      ctx.state.terminalHandle === request.handle;
  }

  async function previewTerminalWriteback(): Promise<void> {
    const request = terminalWritebackRequest;
    if (!request || terminalWritebackBusy) return;
    const kind = elements.terminalWritebackKind.value;
    if (!isTerminalWritebackKind(kind)) {
      elements.terminalWritebackMessage.textContent = "Choose what kind of memory to write.";
      return;
    }
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

  async function commitTerminalWriteback(): Promise<void> {
    const request = terminalWritebackRequest;
    const preview = request?.preview;
    const requestID = request?.requestID;
    if (!request || terminalWritebackBusy || !preview || !requestID) return;
    const policy = terminalWritebackContentPolicy(elements.terminalWritebackContent.value);
    if (!policy.valid || policy.normalized !== preview.content ||
      elements.terminalWritebackKind.value !== preview.kind) {
      invalidateTerminalWritebackPreview();
      return;
    }
    const confirmSummary = preview.replacesSummary &&
      elements.terminalWritebackSummaryConfirm.checked;
    if (preview.replacesSummary && !confirmSummary) {
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
        requestID,
        preview.kind,
        preview.content,
        confirmSummary,
        () => terminalWritebackRequestIsCurrent(request),
      );
      if (!terminalWritebackRequestIsCurrent(request)) return;
      terminalWritebackBusy = false;
      closeTerminalWriteback(true);
      setStatus(`${result.kind} written to ${result.destination}.`);
      await ctx.snapshot.loadSnapshot(ctx.state.board?.planId || 0);
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
        preview.replacesSummary && !elements.terminalWritebackSummaryConfirm.checked;
      showError(error);
    }
  }

  function bind(): void {
    elements.terminalWriteback.addEventListener("click", () => {
      openTerminalWriteback(elements.terminalWriteback);
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
    document.querySelectorAll("[data-close-terminal-writeback]").forEach((closer) => {
      closer.addEventListener("click", () => closeTerminalWriteback());
    });
  }

  return {
    bind,
    closeTerminalWriteback,
  };
}

export type WritebackController = ReturnType<typeof createWritebackController>;
