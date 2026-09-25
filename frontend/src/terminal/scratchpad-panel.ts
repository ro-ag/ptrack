import { terminalControlIcon } from "./control-icon";
import {
  addSnippet,
  clampScratchpadWidth,
  maximumScratchpadWidth,
  orderedSnippets,
  readScratchpadOpen,
  readScratchpadWidth,
  removeSnippet,
  ScratchpadSaver,
  scratchpadNotices,
  scratchpadSplitterWidth,
  snippetPreview,
  togglePinned,
  writeScratchpadOpen,
  writeScratchpadWidth,
  type Scratchpad,
  type ScratchpadSaverBackend,
  type ScratchpadSnippet,
} from "./scratchpad";
import { looksLikeSecret, secretCaptureNotice } from "./secrets";

// Shared scratchpad-panel DOM; hosts provide surface-specific behavior.

export interface ScratchpadPanelElements {
  toggle: HTMLButtonElement;
  panel: HTMLElement;
  splitter: HTMLElement;
  state: HTMLElement;
  close: HTMLButtonElement;
  text: HTMLTextAreaElement;
  add: HTMLButtonElement;
  list: HTMLElement;
  empty: HTMLElement;
  /** The row the panel sits in: it carries the overlay gutter. */
  body: HTMLElement;
}

export interface ScratchpadPanelHost {
  /** The open workspace's generation; zero disables scratchpad traffic. */
  generation: number;
  backend: ScratchpadSaverBackend;
  storage: Pick<Storage, "getItem" | "setItem">;
  /** The body row's width, or 0 while it cannot be measured. */
  bodyWidth(): number;
  /** Whether the active pane has a selection the strip could take. */
  hasSelection(): boolean;
  /** The active pane's selection, or null without a live pane. */
  selection(): string | null;
  /** Whether a snippet could be pasted into the active pane right now. */
  pasteReady(): boolean;
  /** Pastes a snippet through the surface's own paste guard. */
  paste(text: string): Promise<void>;
  clipboardAvailable(): boolean;
  /** Puts text on the system clipboard; rejects when that fails. */
  setClipboardText(text: string): Promise<void>;
  /** The panel opened or closed: the surface refits and re-renders. */
  openChanged(open: boolean): void;
  /** The panel changed width: the surface refits its panes. */
  resized(): void;
  reportError(error: unknown): void;
  setTimeout(callback: () => void, delay: number): unknown;
  clearTimeout(handle: unknown): void;
}

export class ScratchpadPanel {
  readonly #elements: ScratchpadPanelElements;
  readonly #host: ScratchpadPanelHost;
  readonly #saver: ScratchpadSaver;
  readonly #disposers: Array<() => void> = [];
  #open: boolean;
  #width: number;
  #dragCleanup: (() => void) | null = null;
  #disposed = false;

  constructor(elements: ScratchpadPanelElements, host: ScratchpadPanelHost) {
    this.#elements = elements;
    this.#host = host;
    this.#width = readScratchpadWidth(host.storage);
    this.#open = readScratchpadOpen(host.storage);
    this.#saver = new ScratchpadSaver({
      generation: host.generation,
      backend: host.backend,
      clock: {
        setTimeout: (callback, delay) => host.setTimeout(callback, delay),
        clearTimeout: (handle) => host.clearTimeout(handle),
      },
      setText: (text) => host.setClipboardText(text),
      status: (text) => this.#setStatus(text),
      applyRecord: (record, replaceLocalText) =>
        this.#applyRecord(record, replaceLocalText),
      reportError: (error) => {
        if (!this.#disposed) host.reportError(error);
      },
    });
  }

  get open(): boolean {
    return this.#open;
  }

  get saver(): ScratchpadSaver {
    return this.#saver;
  }

  /** Binds the controls and applies the stored open state and width. */
  mount(): void {
    const { toggle, close, text, add, splitter } = this.#elements;
    this.#listen(toggle, "click", () => this.setOpen(!this.#open));
    this.#listen(close, "click", () => {
      this.setOpen(false);
      toggle.focus();
    });
    this.#listen(text, "input", () => this.#saver.markText(text.value));
    // Flush on blur before a project switch can fence the write.
    this.#listen(text, "blur", () => this.flush());
    this.#listen(add, "click", () => {
      const selection = this.#host.selection();
      if (selection !== null) this.capture(selection);
    });
    this.#listen(splitter, "pointerdown", (event) =>
      this.#beginResize(event as PointerEvent),
    );
    this.#listen(splitter, "keydown", (event) =>
      this.#resizeFromKeyboard(event as KeyboardEvent),
    );
    this.setOpen(this.#open, false);
  }

  setOpen(open: boolean, persist = true): void {
    const { panel, splitter, toggle } = this.#elements;
    this.#open = open;
    panel.hidden = !open;
    splitter.hidden = !open;
    toggle.setAttribute("aria-pressed", String(open));
    const label = open ? "Hide scratchpad" : "Show scratchpad";
    toggle.setAttribute("aria-label", label);
    toggle.title = label;
    if (persist) writeScratchpadOpen(this.#host.storage, open);
    this.#applyWidth();
    if (!open) this.#dragCleanup?.();
    this.#host.openChanged(open);
    if (!open) {
      this.#saver.flush();
      return;
    }
    this.renderSelection();
    void this.#saver.ensureLoaded();
  }

  /** Writes any pending edit now. */
  flush(): void {
    this.#saver.flush();
  }

  flushPending(): Promise<void> {
    return this.#saver.flushPending();
  }

  /**
   * Refreshes another surface's write unless this panel has an edit.
   */
  refresh(revision: number): Promise<boolean> {
    return this.#saver.refresh(revision);
  }

  /**
   * Captures an explicit pane selection after credential screening.
   */
  capture(text: string): void {
    if (this.#disposed) return;
    if (looksLikeSecret(text)) {
      this.#setStatus(secretCaptureNotice);
      return;
    }
    // Load the stored revision before capture to avoid a revision-zero conflict.
    if (!this.#saver.loaded && this.#saver.enabled) {
      void this.#saver.ensureLoaded().then(() => {
        if (this.#disposed) return;
        if (this.#saver.loaded) this.#applyCaptured(text);
        else this.#setStatus(scratchpadNotices.unavailable);
      });
      return;
    }
    this.#applyCaptured(text);
  }

  /** Enables the strip's controls for the active pane's current state. */
  renderSelection(): void {
    const { add, list } = this.#elements;
    add.disabled = !this.#host.hasSelection();
    const clipboard = this.#host.clipboardAvailable();
    const pasteReady = clipboard && this.#host.pasteReady();
    for (const button of list.querySelectorAll<HTMLButtonElement>(
      '[data-scratchpad-action="copy"]',
    )) {
      button.disabled = !clipboard;
      // The reason travels on the accessible name too, not only the tooltip.
      labelAction(button, clipboard ? "Copy snippet" : "Copy needs the native clipboard");
    }
    for (const button of list.querySelectorAll<HTMLButtonElement>(
      '[data-scratchpad-action="paste"]',
    )) {
      button.disabled = !pasteReady;
      labelAction(
        button,
        !clipboard
          ? "Paste needs the native clipboard"
          : pasteReady
            ? "Paste snippet into the active pane"
            : "Paste needs a running terminal pane",
      );
    }
  }

  /** Re-applies the width once the body can be measured again. */
  applyWidth(): void {
    this.#applyWidth();
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#saver.dispose();
    this.#dragCleanup?.();
    this.#disposed = true;
    for (const dispose of this.#disposers.splice(0)) dispose();
  }

  #listen(target: EventTarget, type: string, listener: (event: Event) => void): void {
    target.addEventListener(type, listener);
    this.#disposers.push(() => target.removeEventListener(type, listener));
  }

  #setStatus(text: string): void {
    this.#elements.state.textContent = text;
  }

  /**
   * Installs a record the saver read from the store. Returns the local text
   * that must win when the user is typing into the note right now.
   */
  #applyRecord(record: Scratchpad, replaceLocalText: boolean): string | null {
    // A disposed surface has no DOM to update; its final write keeps the record.
    if (this.#disposed) return null;
    this.#renderSnippets();
    const { text } = this.#elements;
    const editing = document.activeElement === text;
    if (!replaceLocalText && editing && text.value !== record.text) {
      return text.value;
    }
    if (text.value !== record.text) {
      // Preserve the focused caret across a reload when possible.
      const start = text.selectionStart ?? 0;
      const end = text.selectionEnd ?? 0;
      text.value = record.text;
      if (editing && typeof text.setSelectionRange === "function") {
        const length = record.text.length;
        text.setSelectionRange(Math.min(start, length), Math.min(end, length));
      }
    }
    return null;
  }

  #applyCaptured(text: string): void {
    const result = addSnippet(this.#saver.record.snippets, text, Date.now());
    if (!result.ok) {
      if (result.reason === "too-large") {
        this.#setStatus(scratchpadNotices.tooLarge);
      } else if (result.reason === "all-pinned") {
        this.#setStatus(scratchpadNotices.allPinned);
      }
      return;
    }
    this.#commitSnippets(result.snippets);
  }

  #commitSnippets(snippets: ScratchpadSnippet[]): void {
    const focus = this.#focusedSnippetAction();
    this.#saver.applySnippets(snippets);
    this.#renderSnippets();
    this.#restoreSnippetFocus(focus);
  }

  /** The snippet control that holds focus, so a re-render can give it back. */
  #focusedSnippetAction(): { id: string; action: string } | null {
    const active = document.activeElement;
    if (!(active instanceof HTMLElement)) return null;
    const action = active.dataset.scratchpadAction;
    const id = active.closest<HTMLElement>(".terminal-scratchpad-snippet")?.dataset
      .snippetId;
    return action && id ? { id, action } : null;
  }

  #restoreSnippetFocus(focus: { id: string; action: string } | null): void {
    if (!focus) return;
    const { list, add, text } = this.#elements;
    const button = list.querySelector<HTMLButtonElement>(
      `[data-snippet-id="${focus.id}"] [data-scratchpad-action="${focus.action}"]`,
    );
    // Move focus to the strip when its deleted row owned it.
    if (button && !button.disabled) button.focus();
    else if (!add.disabled) add.focus();
    else text.focus();
  }

  #renderSnippets(): void {
    const snippets = orderedSnippets(this.#saver.record.snippets);
    this.#elements.list.replaceChildren(
      ...snippets.map((snippet) => this.#row(snippet)),
    );
    this.#elements.empty.hidden = snippets.length > 0;
    this.renderSelection();
  }

  #row(snippet: ScratchpadSnippet): HTMLLIElement {
    const row = document.createElement("li");
    row.className = "terminal-scratchpad-snippet";
    row.dataset.pinned = String(snippet.pinned);
    row.dataset.snippetId = String(snippet.id);
    const preview = document.createElement("span");
    preview.className = "terminal-scratchpad-preview";
    const text = snippetPreview(snippet.text);
    preview.textContent = text;
    preview.title = text;
    const copy = actionButton("copy", "Copy snippet");
    copy.append(terminalControlIcon("duplicate"));
    copy.addEventListener("click", () => void this.#copySnippet(snippet.text));
    const paste = actionButton("paste", "Paste snippet into the active pane");
    paste.textContent = "Paste";
    paste.addEventListener("click", () => void this.#host.paste(snippet.text));
    const pinLabel = snippet.pinned ? "Unpin snippet" : "Pin snippet";
    const pin = actionButton("pin", pinLabel);
    pin.textContent = snippet.pinned ? "Unpin" : "Pin";
    pin.addEventListener("click", () =>
      this.#commitSnippets(togglePinned(this.#saver.record.snippets, snippet.id)),
    );
    const remove = actionButton("delete", "Delete snippet");
    remove.append(terminalControlIcon("close"));
    remove.addEventListener("click", () =>
      this.#commitSnippets(removeSnippet(this.#saver.record.snippets, snippet.id)),
    );
    row.append(preview, copy, paste, pin, remove);
    return row;
  }

  async #copySnippet(text: string): Promise<void> {
    try {
      await this.#host.setClipboardText(text);
    } catch (error) {
      if (!this.#disposed) this.#host.reportError(error);
    }
  }

  #applyWidth(): void {
    const { panel, body, splitter } = this.#elements;
    const width = this.#width;
    panel.style.width = `${width}px`;
    // Body-level overlays (terminal search) step aside for an open panel.
    body.style.setProperty(
      "--terminal-scratchpad-gutter",
      this.#open ? `${width + scratchpadSplitterWidth}px` : "0px",
    );
    splitter.setAttribute("aria-valuenow", String(width));
    const maximum = maximumScratchpadWidth(this.#host.bodyWidth());
    if (Number.isFinite(maximum)) {
      splitter.setAttribute("aria-valuemax", String(maximum));
    } else {
      splitter.removeAttribute("aria-valuemax");
    }
  }

  #setWidth(width: number, persist = true): void {
    this.#width = clampScratchpadWidth(width, this.#host.bodyWidth());
    this.#applyWidth();
    if (persist) writeScratchpadWidth(this.#host.storage, this.#width);
    this.#host.resized();
  }

  #beginResize(event: PointerEvent): void {
    if (!this.#open) return;
    event.preventDefault();
    this.#dragCleanup?.();
    const { splitter } = this.#elements;
    const startX = event.clientX;
    const startWidth = this.#width;
    const pointerID = event.pointerId;
    const move = (moveEvent: PointerEvent) => {
      if (moveEvent.pointerId !== pointerID) return;
      // The panel sits right of the splitter: dragging left widens it.
      this.#setWidth(startWidth + startX - moveEvent.clientX, false);
    };
    const cleanup = () => {
      splitter.removeEventListener("pointermove", move);
      splitter.removeEventListener("pointerup", finish);
      splitter.removeEventListener("pointercancel", finish);
      splitter.removeEventListener("lostpointercapture", finish);
      if (splitter.hasPointerCapture(pointerID)) {
        splitter.releasePointerCapture(pointerID);
      }
      if (this.#dragCleanup === cleanup) this.#dragCleanup = null;
    };
    const finish = (finishEvent: PointerEvent) => {
      if (
        finishEvent.type !== "lostpointercapture" &&
        finishEvent.pointerId !== pointerID
      ) {
        return;
      }
      cleanup();
      writeScratchpadWidth(this.#host.storage, this.#width);
      this.#host.resized();
    };
    this.#dragCleanup = cleanup;
    splitter.setPointerCapture(pointerID);
    splitter.addEventListener("pointermove", move);
    splitter.addEventListener("pointerup", finish);
    splitter.addEventListener("pointercancel", finish);
    splitter.addEventListener("lostpointercapture", finish);
  }

  #resizeFromKeyboard(event: KeyboardEvent): void {
    if (!this.#open) return;
    let width = this.#width;
    if (event.key === "ArrowLeft") width += 16;
    else if (event.key === "ArrowRight") width -= 16;
    else return;
    event.preventDefault();
    this.#setWidth(width);
  }
}

function actionButton(action: string, label: string): HTMLButtonElement {
  const button = document.createElement("button");
  button.className = "terminal-scratchpad-action";
  button.type = "button";
  button.dataset.scratchpadAction = action;
  labelAction(button, label);
  return button;
}

function labelAction(button: HTMLButtonElement, label: string): void {
  button.setAttribute("aria-label", label);
  button.title = label;
}
