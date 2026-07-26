import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import type { IDisposable } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

import { TerminalStreamClient } from "./client";
import type { StreamState } from "./client";
import {
  binaryStringToBytes,
  commitClipboardPaste,
  prepareClipboardPaste,
  splitTerminalInput,
  terminalShortcutAction,
} from "./paste";
import type {
  ClipboardPasteRequest,
  TerminalPlatform,
  TerminalShortcutAction,
} from "./paste";

type DockState = "closed" | "opening" | "running" | "exited" | "failed";

interface TerminalProfile {
  id: string;
  name: string;
  kind: "shell" | "agent";
}

interface TerminalSession {
  sessionId: string;
  profileId: string;
  cwd: string;
  state: string;
  streamUrl: string;
}

interface TerminalExit {
  sessionId: string;
  exitCode: number;
  state: string;
  error?: string;
}

interface TerminalBackend {
  GetTerminalProfiles(): Promise<TerminalProfile[]>;
  CreateTerminal(
    profileID: string,
    cwd: string,
    rows: number,
    columns: number,
  ): Promise<TerminalSession>;
  ResizeTerminal(sessionID: string, rows: number, columns: number): Promise<void>;
  CloseTerminal(sessionID: string, force: boolean): Promise<void>;
}

interface MountOptions {
  backend: TerminalBackend;
  showError(error: unknown): void;
}

interface PaneResources {
  terminal: Terminal;
  fit: FitAddon;
  client: TerminalStreamClient | null;
  observer: ResizeObserver | null;
  subscriptions: IDisposable[];
  eventDisposers: Array<() => void>;
  animationFrame: number | null;
  resizeTimer: number | null;
  pendingSize: { rows: number; columns: number } | null;
  lastResizeAt: number;
  disposed: boolean;
}

const minimumDockHeight = 180;
const defaultDockHeight = 300;
const resizeIntervalMilliseconds = 100;

function requiredElement<T extends HTMLElement>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (!element) throw new Error(`Missing terminal element ${selector}`);
  return element;
}

function messageFrom(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Terminal operation failed";
}

function eventsOn(name: string, callback: (payload: any) => void): () => void {
  const runtime = (window as any).runtime;
  if (typeof runtime?.EventsOnMultiple !== "function") return () => {};
  return runtime.EventsOnMultiple(name, callback, -1);
}

function openExternalURL(uri: string): void {
  const runtime = (window as any).runtime;
  if (typeof runtime?.BrowserOpenURL === "function") runtime.BrowserOpenURL(uri);
}

function platform(): TerminalPlatform {
  if (/Mac|iPhone|iPad/.test(navigator.platform)) return "mac";
  if (/Win/.test(navigator.platform)) return "windows";
  return "linux";
}

function nativeClipboard(): {
  getText(): Promise<string>;
  setText(text: string): Promise<void>;
} {
  const runtime = (window as any).runtime;
  if (
    typeof runtime?.ClipboardGetText !== "function" ||
    typeof runtime?.ClipboardSetText !== "function"
  ) {
    throw new Error("Native clipboard access is unavailable");
  }
  return {
    getText: () => runtime.ClipboardGetText(),
    setText: async (text) => {
      if ((await runtime.ClipboardSetText(text)) !== true) {
        throw new Error("Native clipboard copy failed");
      }
    },
  };
}

class TerminalDock {
  readonly #backend: TerminalBackend;
  readonly #showError: (error: unknown) => void;
  readonly #dock = requiredElement<HTMLElement>("#terminal-dock");
  readonly #body = requiredElement<HTMLElement>("#terminal-body");
  readonly #host = requiredElement<HTMLElement>("#terminal-host");
  readonly #message = requiredElement<HTMLElement>("#terminal-message");
  readonly #status = requiredElement<HTMLElement>("#terminal-status");
  readonly #title = requiredElement<HTMLElement>("#terminal-title");
  readonly #profile = requiredElement<HTMLSelectElement>("#terminal-profile");
  readonly #open = requiredElement<HTMLButtonElement>("#terminal-open");
  readonly #restart = requiredElement<HTMLButtonElement>("#terminal-restart");
  readonly #close = requiredElement<HTMLButtonElement>("#terminal-close");
  readonly #separator = requiredElement<HTMLElement>("#terminal-resize");
  readonly #pasteModal = requiredElement<HTMLElement>("#terminal-paste-modal");
  readonly #pasteForm = requiredElement<HTMLFormElement>("#terminal-paste-form");
  readonly #pasteBackdrop = requiredElement<HTMLButtonElement>(
    "#terminal-paste-backdrop",
  );
  readonly #pasteCancel = requiredElement<HTMLButtonElement>("#terminal-paste-cancel");
  readonly #pasteConfirm = requiredElement<HTMLButtonElement>(
    "#terminal-paste-confirm",
  );
  readonly #pastePreview = requiredElement<HTMLElement>("#terminal-paste-preview");
  readonly #pasteDetail = requiredElement<HTMLElement>("#terminal-paste-detail");
  readonly #contextMenu = requiredElement<HTMLElement>("#terminal-context-menu");
  readonly #menuCopy = requiredElement<HTMLButtonElement>("#terminal-menu-copy");
  readonly #menuPaste = requiredElement<HTMLButtonElement>("#terminal-menu-paste");
  readonly #menuSelectAll = requiredElement<HTMLButtonElement>(
    "#terminal-menu-select-all",
  );

  #state: DockState = "closed";
  #session: TerminalSession | null = null;
  #resources: PaneResources | null = null;
  #generation = 0;
  #dockHeight = defaultDockHeight;
  #earlyExit = new Map<string, TerminalExit>();
  #closing = false;
  #operationBusy = false;
  #dragCleanup: (() => void) | null = null;
  #pasteResolve: ((confirmed: boolean) => void) | null = null;
  #clipboardWrite: Promise<void> = Promise.resolve();
  #pasteBusy = false;
  #pasteRequest = 0;

  constructor(options: MountOptions) {
    this.#backend = options.backend;
    this.#showError = options.showError;
    this.#open.addEventListener("click", () =>
      void this.#runOperation(() => this.#openTerminal()),
    );
    this.#restart.addEventListener("click", () =>
      void this.#runOperation(() => this.#restartTerminal()),
    );
    this.#close.addEventListener("click", () =>
      void this.#runOperation(() => this.#closeTerminal()),
    );
    this.#separator.addEventListener("pointerdown", (event) => this.#beginDockResize(event));
    this.#separator.addEventListener("keydown", (event) => this.#resizeDockFromKeyboard(event));
    this.#pasteForm.addEventListener("submit", (event) => {
      event.preventDefault();
      this.#finishPasteConfirmation(true);
    });
    this.#pasteBackdrop.addEventListener("click", () =>
      this.#finishPasteConfirmation(false),
    );
    this.#pasteCancel.addEventListener("click", () =>
      this.#finishPasteConfirmation(false),
    );
    this.#menuCopy.addEventListener("click", () => {
      this.#hideContextMenu();
      void this.#copySelection();
    });
    this.#menuPaste.addEventListener("click", () => {
      this.#hideContextMenu();
      const resources = this.#resources;
      if (resources) void this.#requestNativePaste(resources);
    });
    this.#menuSelectAll.addEventListener("click", () => {
      this.#hideContextMenu();
      this.#resources?.terminal.selectAll();
      this.#resources?.terminal.focus();
    });
    this.#contextMenu.addEventListener("keydown", (event) =>
      this.#navigateContextMenu(event),
    );
    window.addEventListener("beforeunload", () => this.#teardownPane());
    this.#setShortcutLabels();
    this.#setDockHeight(defaultDockHeight);
    this.#renderState();
  }

  async initialize(): Promise<void> {
    try {
      const profiles = await this.#backend.GetTerminalProfiles();
      this.#profile.replaceChildren();
      for (const profile of profiles) {
        const option = document.createElement("option");
        option.value = profile.id;
        option.textContent = `${profile.name}${profile.kind === "agent" ? " · agent" : ""}`;
        this.#profile.append(option);
      }
      if (profiles.length === 0) {
        throw new Error("No installed terminal profiles were discovered");
      }
    } catch (error) {
      this.#setState("failed", messageFrom(error));
      this.#showError(error);
    }
  }

  async #openTerminal(): Promise<void> {
    if (this.#state === "opening" || this.#state === "running" || !this.#profile.value) return;
    this.#teardownPane();
    const generation = ++this.#generation;
    this.#session = null;
    this.#closing = false;
    this.#body.hidden = false;
    this.#message.hidden = true;
    this.#setState("opening");
    let createdSession: TerminalSession | null = null;

    try {
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      if (generation !== this.#generation) return;
      const resources = this.#createRenderer(generation);
      this.#resources = resources;
      this.#fit(resources, false);
      this.#registerSessionEvents(resources, generation);

      const session = await this.#backend.CreateTerminal(
        this.#profile.value,
        "",
        resources.terminal.rows,
        resources.terminal.cols,
      );
      createdSession = session;
      if (generation !== this.#generation || resources.disposed) {
        await this.#backend.CloseTerminal(session.sessionId, true).catch(() => {});
        return;
      }
      this.#session = session;
      this.#title.textContent = this.#selectedProfileName();

      resources.client = new TerminalStreamClient({
        createWebSocket: (url) => new WebSocket(url) as any,
        writeOutput: (output, done) => resources.terminal.write(output, done),
        onStateChange: (state) => this.#streamStateChanged(state, generation),
      });
      resources.client.connect(session.streamUrl);

      const earlyExit = this.#earlyExit.get(session.sessionId);
      if (earlyExit) {
        this.#earlyExit.delete(session.sessionId);
        this.#handleExit(earlyExit, generation);
      }
      resources.terminal.focus();
    } catch (error) {
      if (createdSession) {
        await this.#backend.CloseTerminal(createdSession.sessionId, true).catch(() => {});
      }
      if (generation !== this.#generation) return;
      this.#teardownPane();
      this.#setState("failed", messageFrom(error));
      this.#showError(error);
    }
  }

  async #restartTerminal(): Promise<void> {
    this.#teardownPane();
    this.#session = null;
    await this.#openTerminal();
  }

  async #closeTerminal(): Promise<void> {
    if (this.#closing) return;
    this.#closing = true;
    this.#invalidatePaste();
    const sessionID = this.#session?.sessionId;
    this.#status.textContent = "Closing…";
    try {
      if (sessionID) await this.#backend.CloseTerminal(sessionID, false);
      this.#teardownPane();
      this.#session = null;
      this.#setState("closed");
    } catch (error) {
      this.#closing = false;
      this.#renderState();
      this.#showError(error);
    }
  }

  async #runOperation(operation: () => Promise<void>): Promise<void> {
    if (this.#operationBusy) return;
    this.#operationBusy = true;
    this.#renderState();
    try {
      await operation();
    } catch (error) {
      this.#showError(error);
    } finally {
      this.#operationBusy = false;
      this.#renderState();
    }
  }

  #configureTerminalInput(resources: PaneResources): void {
    resources.terminal.attachCustomKeyEventHandler((event) => {
      if (event.isComposing) return true;
      const action = terminalShortcutAction(
        event,
        platform(),
        resources.terminal.hasSelection(),
      );
      if (!action) return true;
      event.preventDefault();
      event.stopPropagation();
      if (event.type === "keydown" && !event.repeat) {
        this.#handleTerminalShortcut(action, resources);
      }
      return false;
    });

    const interceptPaste = (event: Event) => {
      event.preventDefault();
      event.stopPropagation();
      void this.#requestNativePaste(resources);
    };
    const interceptRightMouseDown = (event: MouseEvent) => {
      if (event.button !== 2) return;
      event.preventDefault();
      event.stopPropagation();
    };
    const showContextMenu = (event: MouseEvent) => {
      event.preventDefault();
      event.stopPropagation();
      this.#showContextMenu(event.clientX, event.clientY, resources);
    };
    const dismissOnPointer = (event: PointerEvent) => {
      if (
        !this.#contextMenu.hidden &&
        event.target instanceof Node &&
        !this.#contextMenu.contains(event.target)
      ) {
        this.#hideContextMenu();
      }
    };
    const dismissOnKey = (event: KeyboardEvent) => {
      if (!this.#pasteModal.hidden && event.key === "Tab") {
        this.#trapPasteFocus(event);
        return;
      }
      if (event.key !== "Escape") return;
      if (!this.#pasteModal.hidden) {
        event.preventDefault();
        this.#finishPasteConfirmation(false);
      } else if (!this.#contextMenu.hidden) {
        event.preventDefault();
        this.#hideContextMenu();
        resources.terminal.focus();
      }
    };
    const dismiss = () => this.#hideContextMenu();

    this.#host.addEventListener("paste", interceptPaste, true);
    this.#host.addEventListener("mousedown", interceptRightMouseDown, true);
    this.#host.addEventListener("contextmenu", showContextMenu, true);
    document.addEventListener("pointerdown", dismissOnPointer, true);
    document.addEventListener("keydown", dismissOnKey);
    window.addEventListener("blur", dismiss);
    window.addEventListener("resize", dismiss);
    resources.eventDisposers.push(() => {
      this.#host.removeEventListener("paste", interceptPaste, true);
      this.#host.removeEventListener("mousedown", interceptRightMouseDown, true);
      this.#host.removeEventListener("contextmenu", showContextMenu, true);
      document.removeEventListener("pointerdown", dismissOnPointer, true);
      document.removeEventListener("keydown", dismissOnKey);
      window.removeEventListener("blur", dismiss);
      window.removeEventListener("resize", dismiss);
    });
  }

  #handleTerminalShortcut(
    action: TerminalShortcutAction,
    resources: PaneResources,
  ): void {
    if (action === "copy") {
      void this.#copySelection(resources);
    } else if (action === "paste") {
      void this.#requestNativePaste(resources);
    } else if (action === "select-all") {
      resources.terminal.selectAll();
    } else if (action === "context-menu") {
      const bounds = this.#host.getBoundingClientRect();
      this.#showContextMenu(bounds.left + 24, bounds.top + 24, resources);
    }
  }

  async #copySelection(resources = this.#resources): Promise<void> {
    if (!resources || resources.disposed || !resources.terminal.hasSelection()) return;
    const generation = this.#generation;
    const selection = resources.terminal.getSelection();
    const write = this.#clipboardWrite.then(() =>
      nativeClipboard().setText(selection),
    );
    this.#clipboardWrite = write.catch(() => {});
    try {
      await write;
    } catch (error) {
      if (generation === this.#generation && !resources.disposed) {
        this.#showError(error);
      }
    } finally {
      if (generation === this.#generation && !resources.disposed) {
        resources.terminal.focus();
      }
    }
  }

  async #requestNativePaste(resources: PaneResources): Promise<void> {
    if (resources.disposed || this.#state !== "running" || this.#pasteBusy) return;
    this.#pasteBusy = true;
    const requestID = ++this.#pasteRequest;
    const generation = this.#generation;
    try {
      await this.#clipboardWrite;
      if (!this.#canPaste(resources, generation, requestID)) return;
      const text = await nativeClipboard().getText();
      if (!this.#canPaste(resources, generation, requestID)) return;
      const request = prepareClipboardPaste(
        text,
        resources.terminal.buffer.active.type === "alternate",
      );
      await commitClipboardPaste(
        request,
        (pending) => this.#confirmPaste(pending),
        (pending) => {
          if (this.#canPaste(resources, generation, requestID)) {
            resources.terminal.paste(pending);
          }
        },
      );
      if (
        this.#canPaste(resources, generation, requestID) &&
        this.#pasteModal.hidden
      ) {
        resources.terminal.focus();
      }
    } catch (error) {
      if (this.#canPaste(resources, generation, requestID)) {
        this.#showError(error);
      }
    } finally {
      if (requestID === this.#pasteRequest) this.#pasteBusy = false;
    }
  }

  #canPaste(
    resources: PaneResources,
    generation: number,
    requestID: number,
  ): boolean {
    return (
      requestID === this.#pasteRequest &&
      generation === this.#generation &&
      this.#resources === resources &&
      !resources.disposed &&
      this.#state === "running" &&
      !this.#closing
    );
  }

  #invalidatePaste(): void {
    this.#pasteRequest += 1;
    this.#pasteBusy = false;
    this.#finishPasteConfirmation(false);
  }

  #confirmPaste(request: ClipboardPasteRequest): Promise<boolean> {
    this.#hideContextMenu();
    this.#finishPasteConfirmation(false);
    this.#pastePreview.textContent = request.preview;
    this.#pasteDetail.textContent = `${request.lineCount} lines${
      request.previewTruncated ? " · preview truncated" : ""
    }. Review the text before sending it to the terminal.`;
    this.#pasteModal.hidden = false;
    this.#pasteCancel.focus();
    return new Promise<boolean>((resolve) => {
      this.#pasteResolve = resolve;
    });
  }

  #finishPasteConfirmation(confirmed: boolean): void {
    const resolve = this.#pasteResolve;
    this.#pasteResolve = null;
    this.#pasteModal.hidden = true;
    this.#pastePreview.textContent = "";
    if (resolve) resolve(confirmed);
  }

  #trapPasteFocus(event: KeyboardEvent): void {
    const focusable = [this.#pasteCancel, this.#pastePreview, this.#pasteConfirm];
    const current = focusable.indexOf(document.activeElement as HTMLElement);
    const next = event.shiftKey
      ? current <= 0
        ? focusable.length - 1
        : current - 1
      : current < 0 || current === focusable.length - 1
        ? 0
        : current + 1;
    event.preventDefault();
    focusable[next].focus();
  }

  #showContextMenu(x: number, y: number, resources: PaneResources): void {
    this.#finishPasteConfirmation(false);
    this.#menuCopy.disabled = !resources.terminal.hasSelection();
    this.#menuPaste.disabled = this.#state !== "running";
    this.#contextMenu.hidden = false;
    const width = this.#contextMenu.offsetWidth;
    const height = this.#contextMenu.offsetHeight;
    this.#contextMenu.style.left = `${Math.max(8, Math.min(x, window.innerWidth - width - 8))}px`;
    this.#contextMenu.style.top = `${Math.max(8, Math.min(y, window.innerHeight - height - 8))}px`;
    [this.#menuCopy, this.#menuPaste, this.#menuSelectAll]
      .find((button) => !button.disabled)
      ?.focus();
  }

  #hideContextMenu(): void {
    this.#contextMenu.hidden = true;
  }

  #navigateContextMenu(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      this.#hideContextMenu();
      this.#resources?.terminal.focus();
      return;
    }
    const buttons = [this.#menuCopy, this.#menuPaste, this.#menuSelectAll].filter(
      (button) => !button.disabled,
    );
    const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
    let next = current;
    if (event.key === "ArrowDown") next = (current + 1) % buttons.length;
    else if (event.key === "ArrowUp") next = (current - 1 + buttons.length) % buttons.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = buttons.length - 1;
    else return;
    event.preventDefault();
    buttons[next]?.focus();
  }

  #setShortcutLabels(): void {
    const labels =
      platform() === "mac"
        ? { copy: "⌘C", paste: "⌘V", selectAll: "⌘A" }
        : { copy: "Ctrl+Shift+C", paste: "Ctrl+V", selectAll: "Ctrl+Shift+A" };
    requiredElement<HTMLElement>("#terminal-menu-copy-shortcut").textContent =
      labels.copy;
    requiredElement<HTMLElement>("#terminal-menu-paste-shortcut").textContent =
      labels.paste;
    requiredElement<HTMLElement>("#terminal-menu-select-all-shortcut").textContent =
      labels.selectAll;
  }

  #createRenderer(generation: number): PaneResources {
    this.#host.replaceChildren();
    const terminal = new Terminal({
      cursorBlink: true,
      scrollback: 10_000,
      theme: {
        background: "#090d12",
        foreground: "#e6e9f0",
        cursor: "#3dd6a3",
        selectionBackground: "#31594f",
      },
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.loadAddon(new SearchAddon());
    terminal.loadAddon(
      new WebLinksAddon((event, uri) => {
        const isMac = /Mac|iPhone|iPad/.test(navigator.platform);
        if ((isMac && !event.metaKey) || (!isMac && !event.ctrlKey)) return;
        event.preventDefault();
        openExternalURL(uri);
      }),
    );
    terminal.open(this.#host);

    const resources: PaneResources = {
      terminal,
      fit,
      client: null,
      observer: null,
      subscriptions: [],
      eventDisposers: [],
      animationFrame: null,
      resizeTimer: null,
      pendingSize: null,
      lastResizeAt: 0,
      disposed: false,
    };

    this.#configureTerminalInput(resources);
    resources.subscriptions.push(
      terminal.onData((data) => {
        const bytes = new TextEncoder().encode(data);
        for (const chunk of splitTerminalInput(bytes)) resources.client?.sendInput(chunk);
      }),
      terminal.onBinary((data) => {
        for (const chunk of splitTerminalInput(binaryStringToBytes(data))) {
          resources.client?.sendInput(chunk);
        }
      }),
      terminal.onTitleChange((title) => {
        if (generation === this.#generation && title) this.#title.textContent = title;
      }),
    );

    if ("ResizeObserver" in window) {
      resources.observer = new ResizeObserver(() => {
        if (resources.animationFrame !== null) return;
        resources.animationFrame = requestAnimationFrame(() => {
          resources.animationFrame = null;
          this.#fit(resources, true);
        });
      });
      resources.observer.observe(this.#host);
    }

    try {
      const webgl = new WebglAddon();
      terminal.loadAddon(webgl);
      const contextLoss = webgl.onContextLoss(() => {
        contextLoss.dispose();
        webgl.dispose();
        if (!resources.disposed) terminal.refresh(0, terminal.rows - 1);
      });
      resources.subscriptions.push(contextLoss);
    } catch {
      // Xterm's DOM renderer remains active.
    }
    return resources;
  }

  #registerSessionEvents(resources: PaneResources, generation: number): void {
    resources.eventDisposers.push(
      eventsOn("terminal:exit", (payload: TerminalExit) => {
        if (!payload?.sessionId) return;
        if (this.#session?.sessionId === payload.sessionId) {
          this.#handleExit(payload, generation);
        } else {
          this.#earlyExit.set(payload.sessionId, payload);
        }
      }),
    );
  }

  #handleExit(result: TerminalExit, generation: number): void {
    if (generation !== this.#generation) return;
    const detail = result.error
      ? result.error
      : `Process exited with code ${result.exitCode}`;
    this.#setState(result.state === "failed" ? "failed" : "exited", detail);
  }

  #streamStateChanged(state: StreamState, generation: number): void {
    if (generation !== this.#generation || this.#closing) return;
    if (state === "open" && this.#state === "opening") {
      this.#setState("running");
    } else if (state === "error") {
      this.#setState("failed", "Terminal stream failed");
    } else if (state === "closed" && (this.#state === "running" || this.#state === "opening")) {
      this.#setState("exited", "Terminal stream closed");
    }
  }

  #fit(resources: PaneResources, notifyBackend: boolean): void {
    if (resources.disposed || this.#body.hidden || this.#host.clientWidth === 0) return;
    const buffer = resources.terminal.buffer.active;
    const wasAtBottom = buffer.viewportY === buffer.baseY;
    const viewportLine = buffer.viewportY;
    try {
      resources.fit.fit();
      if (!wasAtBottom) {
        resources.terminal.scrollToLine(Math.min(viewportLine, resources.terminal.buffer.active.baseY));
      }
      if (notifyBackend && this.#session) {
        this.#scheduleBackendResize(resources);
      }
    } catch {
      // A later observer callback retries after layout becomes measurable.
    }
  }

  #scheduleBackendResize(resources: PaneResources): void {
    resources.pendingSize = {
      rows: resources.terminal.rows,
      columns: resources.terminal.cols,
    };
    if (resources.resizeTimer !== null) return;
    const elapsed = performance.now() - resources.lastResizeAt;
    const delay = Math.max(0, resizeIntervalMilliseconds - elapsed);
    resources.resizeTimer = window.setTimeout(() => {
      resources.resizeTimer = null;
      const size = resources.pendingSize;
      const sessionID = this.#session?.sessionId;
      resources.pendingSize = null;
      if (!size || !sessionID || resources.disposed) return;
      resources.lastResizeAt = performance.now();
      void this.#backend
        .ResizeTerminal(sessionID, size.rows, size.columns)
        .catch((error) => this.#showError(error));
      if (resources.pendingSize) this.#scheduleBackendResize(resources);
    }, delay);
  }

  #teardownPane(): void {
    this.#dragCleanup?.();
    this.#generation += 1;
    this.#invalidatePaste();
    this.#hideContextMenu();
    const resources = this.#resources;
    if (!resources || resources.disposed) return;
    resources.disposed = true;
    resources.client?.close();
    resources.observer?.disconnect();
    if (resources.animationFrame !== null) cancelAnimationFrame(resources.animationFrame);
    if (resources.resizeTimer !== null) window.clearTimeout(resources.resizeTimer);
    for (const dispose of resources.eventDisposers.splice(0)) dispose();
    for (const subscription of resources.subscriptions.splice(0)) subscription.dispose();
    resources.terminal.dispose();
    this.#host.replaceChildren();
    this.#resources = null;
  }

  #setState(state: DockState, detail = ""): void {
    if (state !== "running") this.#invalidatePaste();
    this.#state = state;
    this.#message.textContent = detail;
    this.#message.hidden = detail === "";
    this.#renderState();
  }

  #renderState(): void {
    this.#dock.dataset.state = this.#state;
    this.#body.hidden = this.#state === "closed";
    this.#status.textContent =
      {
        closed: "Closed",
        opening: "Opening…",
        running: "Running",
        exited: "Exited",
        failed: "Failed",
      }[this.#state];
    this.#open.hidden = this.#state !== "closed";
    this.#restart.hidden = this.#state !== "exited" && this.#state !== "failed";
    this.#close.hidden = this.#state === "closed" || this.#state === "failed";
    this.#open.disabled = this.#operationBusy;
    this.#restart.disabled = this.#operationBusy;
    this.#close.disabled = this.#operationBusy;
    this.#profile.disabled =
      this.#operationBusy || this.#state === "opening" || this.#state === "running";
    if (this.#state === "closed") {
      this.#title.textContent = "Stopped";
    }
  }

  #selectedProfileName(): string {
    return this.#profile.selectedOptions[0]?.textContent || "Terminal";
  }

  #maximumDockHeight(): number {
    const workArea = this.#dock.parentElement;
    return Math.max(minimumDockHeight, Math.floor((workArea?.clientHeight ?? 800) * 0.75));
  }

  #setDockHeight(height: number): void {
    this.#dockHeight = Math.max(minimumDockHeight, Math.min(height, this.#maximumDockHeight()));
    this.#dock.style.setProperty("--terminal-dock-height", `${this.#dockHeight}px`);
    this.#separator.setAttribute("aria-valuemax", String(this.#maximumDockHeight()));
    this.#separator.setAttribute("aria-valuenow", String(Math.round(this.#dockHeight)));
    if (this.#resources) {
      if (this.#resources.animationFrame !== null) cancelAnimationFrame(this.#resources.animationFrame);
      this.#resources.animationFrame = requestAnimationFrame(() => {
        if (!this.#resources) return;
        this.#resources.animationFrame = null;
        this.#fit(this.#resources, true);
      });
    }
  }

  #beginDockResize(event: PointerEvent): void {
    if (this.#state === "closed") return;
    event.preventDefault();
    this.#dragCleanup?.();
    const startY = event.clientY;
    const startHeight = this.#dockHeight;
    const pointerID = event.pointerId;
    const move = (moveEvent: PointerEvent) => {
      if (moveEvent.pointerId !== pointerID) return;
      this.#setDockHeight(startHeight + startY - moveEvent.clientY);
    };
    const cleanup = () => {
      this.#separator.removeEventListener("pointermove", move);
      this.#separator.removeEventListener("pointerup", finish);
      this.#separator.removeEventListener("pointercancel", finish);
      this.#separator.removeEventListener("lostpointercapture", finish);
      if (this.#separator.hasPointerCapture(pointerID)) {
        this.#separator.releasePointerCapture(pointerID);
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
      if (this.#resources) this.#fit(this.#resources, true);
    };
    this.#dragCleanup = cleanup;
    this.#separator.setPointerCapture(pointerID);
    this.#separator.addEventListener("pointermove", move);
    this.#separator.addEventListener("pointerup", finish);
    this.#separator.addEventListener("pointercancel", finish);
    this.#separator.addEventListener("lostpointercapture", finish);
  }

  #resizeDockFromKeyboard(event: KeyboardEvent): void {
    if (this.#state === "closed") return;
    let nextHeight = this.#dockHeight;
    if (event.key === "ArrowUp") nextHeight += 16;
    else if (event.key === "ArrowDown") nextHeight -= 16;
    else if (event.key === "PageUp") nextHeight += 64;
    else if (event.key === "PageDown") nextHeight -= 64;
    else if (event.key === "Home") nextHeight = minimumDockHeight;
    else if (event.key === "End") nextHeight = this.#maximumDockHeight();
    else return;
    event.preventDefault();
    this.#setDockHeight(nextHeight);
  }
}

export async function mountTerminalDock(options: MountOptions): Promise<void> {
  const dock = new TerminalDock(options);
  await dock.initialize();
}
