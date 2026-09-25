import type { FitAddon } from "@xterm/addon-fit";
import type { SearchAddon } from "@xterm/addon-search";
import type { Terminal } from "@xterm/xterm";

import type { Backend } from "../backend";
import { readTerminalPreferenceOverrides } from "../settings/preferences";
import { THEME_STORAGE_KEY, terminalThemeName } from "../theme";
import type { AppContext } from "../workspace/app-context";
import { element } from "../workspace/dom";
import { messageFrom } from "../workspace/format";
import {
  findTerminalPane,
  maximumWorkspaceTabs,
  paneIds,
  type Workspace,
  type WorkspaceTab,
} from "../workspace/model";
import type { TerminalStreamClaim, TerminalWindowAssignment } from "../workspace/snapshot-types";
import { WorkspaceSplitView } from "../workspace/split-view";
import { workspaceTabElementIds } from "../workspace/tab-bar";
import {
  WorkspaceTabController,
  createCryptoIdFactory,
} from "../workspace/tab-controller";
import { TerminalStreamClient, type StreamState } from "./client";
import { terminalControlIcon } from "./control-icon";
import { terminalDiagnosticView } from "./diagnostics";
import { TerminalDiagnosticsPopover } from "./diagnostics-popover";
import type { DiscoveredTerminalProfile } from "./linked-launch";
import {
  binaryStringToBytes,
  commitClipboardPaste,
  pasteReviewSummary,
  prepareClipboardPaste,
  splitTerminalInput,
  terminalKeyShortcut,
  terminalTextToBytes,
} from "./paste";
import { terminalPlatform } from "./platform";
import {
  detachedLastTabCloseTitle,
  detachedTabCloseIntent,
  reclaimStream,
  reclaimingStreamNotice,
  streamClaimEnded,
  streamCloseDisposition,
  streamOutputEndedNotice,
  streamReclaimFailedNotice,
  terminalGapNotice,
  terminalWindowStatusLabel,
} from "./pop-out";
import {
  clampTerminalFontSize,
  readTerminalProfileFontSize,
  terminalZoomFontSize,
  writeTerminalProfileFontSize,
} from "./preferences";
import {
  loadTerminalFont,
  normalizeTerminalProfileSettings,
  terminalProfileTheme,
  type NormalizedTerminalProfileSettings,
} from "./profile-settings";
import { applyTerminalTheme, createTerminalRenderer, paintTerminalBackground } from "./renderer";
import { TerminalResizeDispatcher } from "./resize-dispatch";
import {
  detachedScratchpadStorage,
  scratchpadChangedEventName,
  type ScratchpadChangedEvent,
} from "./scratchpad";
import { ScratchpadPanel, type ScratchpadPanelElements } from "./scratchpad-panel";
import { terminalSearchOptions, terminalSearchResultLabel } from "./search";
import {
  applyShellSignal,
  initialShellState,
  parseShellOSC,
  type ShellState,
} from "./shell-integration";
import { readModernUnicodeSetting } from "./unicode";
import {
  detachedDiagnosticInput,
  TerminalWindowInfo,
  type DetachedPaneFacts,
  type TerminalWindowInfoElements,
} from "./window-info";

/** The detached window's page: everything outside the dock it reuses. */
interface TerminalWindowView {
  section: HTMLElement;
  status: HTMLParagraphElement;
  gap: HTMLParagraphElement;
  gapDetail: HTMLSpanElement;
  host: HTMLDivElement;
  heading: HTMLHeadingElement;
  searchBar: HTMLDivElement;
  searchInput: HTMLInputElement;
  searchResults: HTMLSpanElement;
  searchClose: HTMLButtonElement;
  tabs: HTMLDivElement;
  controls: HTMLDivElement;
  info: HTMLElement;
  facts: TerminalWindowInfoElements;
  diagnostics: ConstructorParameters<typeof TerminalDiagnosticsPopover>[0];
  scratchpad: ScratchpadPanelElements;
}

function terminalWindowView(): TerminalWindowView {
  const info = element("#terminal-window-info", HTMLElement);
  const body = element("#terminal-window-body", HTMLElement);
  return {
    section: element("#terminal-window", HTMLElement),
    status: element("#terminal-window-status", HTMLParagraphElement),
    gap: element("#terminal-window-gap", HTMLParagraphElement),
    gapDetail: element("#terminal-window-gap-detail", HTMLSpanElement),
    host: element("#terminal-window-host", HTMLDivElement),
    heading: element("#terminal-window-heading", HTMLHeadingElement),
    searchBar: element("#terminal-window-search", HTMLDivElement),
    searchInput: element("#terminal-window-search-input", HTMLInputElement),
    searchResults: element("#terminal-window-search-results", HTMLSpanElement),
    searchClose: element("#terminal-window-search-close", HTMLButtonElement),
    tabs: element("#terminal-window-tabs", HTMLDivElement),
    controls: element("#terminal-window-controls", HTMLDivElement),
    info,
    facts: {
      state: element("#terminal-window-state", HTMLElement),
      profile: element("#terminal-window-profile", HTMLElement),
      cwd: element("#terminal-window-cwd", HTMLElement),
      association: element("#terminal-window-association", HTMLElement),
      associationLabel: element("#terminal-window-association-label", HTMLElement),
    },
    diagnostics: {
      toggle: element("#terminal-window-diagnostics-toggle", HTMLButtonElement),
      popover: element("#terminal-window-diagnostics", HTMLElement),
      close: element("#terminal-window-diagnostics-close", HTMLButtonElement),
      process: element("#terminal-window-diagnostic-process", HTMLElement),
      stream: element("#terminal-window-diagnostic-stream", HTMLElement),
      renderer: element("#terminal-window-diagnostic-renderer", HTMLElement),
      layout: element("#terminal-window-diagnostic-layout", HTMLElement),
      updated: element("#terminal-window-diagnostic-updated", HTMLElement),
      header: info,
      surface: element("#terminal-window", HTMLElement),
    },
    scratchpad: {
      toggle: element("#terminal-window-scratchpad-toggle", HTMLButtonElement),
      panel: element("#terminal-window-scratchpad", HTMLElement),
      splitter: element("#terminal-window-scratchpad-splitter", HTMLElement),
      state: element("#terminal-window-scratchpad-state", HTMLElement),
      close: element("#terminal-window-scratchpad-close", HTMLButtonElement),
      text: element("#terminal-window-scratchpad-text", HTMLTextAreaElement),
      add: element("#terminal-window-scratchpad-add", HTMLButtonElement),
      list: element("#terminal-window-scratchpad-snippets", HTMLElement),
      empty: element("#terminal-window-scratchpad-empty", HTMLElement),
      body,
    },
  };
}

/**
 * Uses the native clipboard, WebView clipboard, or neither.
 */
function windowClipboard(): ((text: string) => Promise<void>) | null {
  const write = window.runtime?.ClipboardSetText;
  if (typeof write === "function") {
    return async (text) => {
      if ((await write(text)) !== true) throw new Error("Native clipboard copy failed");
    };
  }
  const clipboard = typeof navigator === "undefined" ? undefined : navigator.clipboard;
  if (clipboard && typeof clipboard.writeText === "function") {
    return (text) => clipboard.writeText(text);
  }
  return null;
}

type PaneStreamState = StreamState;

/** One pane's renderer and the stream it draws. */
class WindowPane {
  readonly resize: TerminalResizeDispatcher;
  state: PaneStreamState = "connecting";
  sequence = 0;
  attempts = 0;
  reclaiming = false;
  ended = false;
  sessionEnded = false;
  exitStatus = "";
  /** The exit carried an error rather than an exit code. */
  failed = false;
  /** Advisory shell-integration state, for the info row only. */
  shellState: ShellState = initialShellState;
  /** Known only for a session this window started itself. */
  shellNonce = "";
  changedAt = Date.now();
  client: TerminalStreamClient | null = null;

  constructor(
    readonly sessionId: string,
    readonly terminal: Terminal,
    readonly fit: FitAddon,
    readonly search: SearchAddon,
    readonly host: HTMLDivElement,
    readonly profileId: string,
    public fontSize: number,
    readonly baseFontSize: number,
    dispatch: (pane: WindowPane, size: { rows: number; columns: number }) => void,
  ) {
    this.resize = new TerminalResizeDispatcher({
      now: () => performance.now(),
      setTimer: (callback, delay) => window.setTimeout(callback, delay),
      clearTimer: (timer) => window.clearTimeout(timer),
      accepted: () => this.state === "open" && !this.ended && this.host.isConnected && this.host.getBoundingClientRect().width > 0,
      dispatch: (size) => dispatch(this, size),
    });
  }

  dispose(): void {
    this.ended = true;
    this.client?.close();
    this.resize.dispose();
    this.terminal.dispose();
  }
}

/**
 * Renders an assigned tab and returns its shape to the main window on close.
 */
class DetachedTerminalWindow {
  readonly #panes = new Map<string, WindowPane>();
  readonly #controller: WorkspaceTabController;
  readonly #originalTab: WorkspaceTab;
  #splitView: WorkspaceSplitView | null = null;
  #assignmentWrites: Promise<void> = Promise.resolve();
  #resizeFrame = 0;
  #busy = false;
  readonly #info: TerminalWindowInfo;
  readonly #diagnostics: TerminalDiagnosticsPopover;
  #scratchpad: ScratchpadPanel | null = null;
  #clipboardWrite: Promise<void> = Promise.resolve();
  // Read once when the window opens, as the main window's dock does.
  readonly #overrides = readTerminalPreferenceOverrides(localStorage);

  constructor(
    readonly ctx: AppContext,
    readonly label: string,
    readonly generation: number,
    readonly view: TerminalWindowView,
    readonly profiles: readonly DiscoveredTerminalProfile[],
    readonly projectRoot: string,
    controller: WorkspaceTabController,
    originalTab: WorkspaceTab,
  ) {
    this.#controller = controller;
    this.#originalTab = originalTab;
    this.#info = new TerminalWindowInfo(view.facts);
    this.#diagnostics = new TerminalDiagnosticsPopover(
      view.diagnostics,
      () => terminalDiagnosticView(this.diagnosticInput()),
    );
  }

  setStatus(message: string): void {
    this.view.status.textContent = message;
    this.view.status.dataset.connected = String(message === terminalWindowStatusLabel("open"));
  }

  showGap(): void {
    this.view.gap.hidden = false;
  }

  #api(): Backend {
    return this.ctx.api();
  }

  // Apply the same stored profile overrides as the main window.
  settingsForProfile(profileId: string): NormalizedTerminalProfileSettings {
    const overrides = this.#overrides;
    const profile = this.profiles.find((candidate) => candidate.id === profileId);
    return normalizeTerminalProfileSettings({
      ...(profile ?? {}),
      fontFamily: overrides.fontFamily || profile?.fontFamily,
      scrollback: overrides.scrollback || profile?.scrollback,
    });
  }

  get workspace(): Workspace {
    return this.#controller.workspace;
  }

  currentTab(): WorkspaceTab | undefined {
    return this.workspace.tabs.find((item) => item.id === this.workspace.activeTabId);
  }

  activePane(): WindowPane | undefined {
    return this.#panes.get(this.currentTab()?.activePaneId ?? "");
  }

  createPane(paneId: string, sessionId: string): WindowPane {
    const owner = this.workspace.tabs.find((item) => paneIds(item.root).includes(paneId));
    const paneHost = document.createElement("div");
    paneHost.className = "terminal-window-pane";
    const profileId = owner ? findTerminalPane(owner.root, paneId)?.profileId ?? "" : "";
    const settings = this.settingsForProfile(profileId);
    const fontSize = readTerminalProfileFontSize(localStorage, profileId, settings.fontSize);
    // The dock's renderer, add-ons and link rule (terminal/renderer.ts).
    const { terminal, fit, search } = createTerminalRenderer({
      settings: {
        ...settings,
        theme: terminalThemeName(settings.theme, document.documentElement.dataset.theme),
      },
      fontSize,
      modernUnicode: readModernUnicodeSetting(localStorage),
      onLinkError: (error) => this.setStatus(messageFrom(error)),
    });
    terminal.open(paneHost);
    paintTerminalBackground(terminal);
    const agent = this.profiles.find((candidate) => candidate.id === profileId)?.kind === "agent";
    terminal.textarea?.setAttribute(
      "aria-label",
      `Terminal session — ${owner?.title}`,
    );
    const pane = new WindowPane(
      sessionId,
      terminal,
      fit,
      search,
      paneHost,
      profileId,
      fontSize,
      settings.fontSize,
      (resized, size) => {
        void this.#api().ResizeTerminalV2(this.generation, resized.sessionId, size.rows, size.columns).catch((error: unknown) => {
          resized.resize.invalidate(size);
          this.setStatus(messageFrom(error));
        });
      },
    );
    this.#panes.set(paneId, pane);
    // Claimed sessions lack the main window nonce; read only standard markers.
    if (!agent) {
      for (const identifier of [7, 133, 633] as const) {
        terminal.parser.registerOscHandler(identifier, (payload) => {
          const signal = parseShellOSC(identifier, payload, pane.shellNonce);
          if (signal && !pane.ended) {
            pane.shellState = applyShellSignal(pane.shellState, signal, performance.now());
            if (pane === this.activePane()) this.renderInfo();
          }
          return true;
        });
      }
    }
    terminal.onSelectionChange(() => {
      if (pane === this.activePane()) this.#scratchpad?.renderSelection();
    });
    return pane;
  }

  // The least-connected pane supplies the window status.
  renderStatus(): void {
    const tab = this.currentTab();
    const states = tab ? paneIds(tab.root).map((id) => this.#panes.get(id)?.state || "connecting") : [];
    const aggregate = (["error", "closed", "connecting"] as const).find((candidate) =>
      states.includes(candidate),
    ) ?? "open";
    this.setStatus(terminalWindowStatusLabel(aggregate));
    this.renderInfo();
  }

  // ------------------------------------------------------------ info row
  // The dock's header facts for the active pane, the ⓘ diagnostics, and the
  // linked plan or task — read-only here; editing stays in the main window.

  #paneFacts(pane: WindowPane | undefined): DetachedPaneFacts | null {
    if (!pane) return null;
    return {
      stream: pane.state,
      ended: pane.ended,
      failed: pane.failed,
      shell: pane.shellState,
      changedAt: pane.changedAt,
    };
  }

  diagnosticInput() {
    return detachedDiagnosticInput({
      pane: this.#paneFacts(this.activePane()),
      linked: this.currentTab()?.association !== undefined,
      visible: document.visibilityState === "visible",
    });
  }

  renderInfo(): void {
    const tab = this.currentTab();
    const pane = this.activePane();
    const descriptor = tab ? findTerminalPane(tab.root, tab.activePaneId) : undefined;
    const profileId = pane?.profileId || descriptor?.profileId || "";
    const profile = this.profiles.find((candidate) => candidate.id === profileId);
    this.#info.render({
      pane: this.#paneFacts(pane),
      profileName: profile?.name || profileId || "Default profile",
      cwd: descriptor?.cwd || this.projectRoot,
      association: tab?.association,
    });
    if (this.#diagnostics.open) {
      this.#diagnostics.render(terminalDiagnosticView(this.diagnosticInput()));
    }
    this.#scratchpad?.renderSelection();
  }

  // ---------------------------------------------------------- scratchpad
  // Shared, generation-fenced scratchpad panel.

  async writeClipboard(text: string): Promise<void> {
    const write = windowClipboard();
    if (!write) throw new Error("Clipboard access is unavailable");
    const pending = this.#clipboardWrite.then(() => write(text));
    this.#clipboardWrite = pending.catch(() => {});
    await pending;
  }

  mountScratchpad(): void {
    const panel = new ScratchpadPanel(this.view.scratchpad, {
      generation: this.generation,
      backend: {
        get: (generation) => this.#api().GetScratchpadV1(generation),
        set: (generation, revision, scratchpad) =>
          this.#api().SetScratchpadV1(generation, revision, scratchpad),
      },
      storage: detachedScratchpadStorage(localStorage),
      bodyWidth: () => this.view.scratchpad.body.clientWidth,
      hasSelection: () => this.activePane()?.terminal.hasSelection() ?? false,
      selection: () => this.activePane()?.terminal.getSelection() ?? null,
      pasteReady: () => {
        const pane = this.activePane();
        return Boolean(pane && pane.state === "open" && !pane.ended);
      },
      paste: async (text) => {
        const pane = this.activePane();
        if (pane && pane.state === "open" && !pane.ended) await this.pasteText(pane, text);
      },
      clipboardAvailable: () => windowClipboard() !== null,
      setClipboardText: (text) => this.writeClipboard(text),
      openChanged: () => this.scheduleFit(),
      resized: () => this.scheduleFit(),
      reportError: (error) => this.reportError(error),
      setTimeout: (callback, delay) => window.setTimeout(callback, delay),
      clearTimeout: (handle) => window.clearTimeout(handle as number),
    });
    this.#scratchpad = panel;
    panel.mount();
    window.runtime?.EventsOnMultiple?.(scratchpadChangedEventName, (payload) => {
      const change = payload as Partial<ScratchpadChangedEvent> | null;
      if (change?.generation !== this.generation) return;
      void panel.refresh(Number(change.revision));
    }, -1);
    // Flush before unload because the debounce may not run.
    window.addEventListener("beforeunload", () => panel.flush());
    window.addEventListener("blur", () => panel.flush());
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "hidden") panel.flush();
    });
  }

  async copySelection(pane: WindowPane): Promise<void> {
    const selection = pane.terminal.getSelection();
    if (!selection) return;
    try {
      await this.writeClipboard(selection);
      // Only explicit selections reach the scratchpad; secret-looking copies do not.
      this.#scratchpad?.capture(selection);
    } catch (error) {
      this.reportError(error);
    }
  }

  fitPane(pane: WindowPane): void {
    if (!pane.host.isConnected || pane.host.getBoundingClientRect().width === 0 || pane.host.getBoundingClientRect().height === 0) return;
    pane.fit.fit();
    pane.resize.queue({ rows: pane.terminal.rows, columns: pane.terminal.cols });
  }

  fitAll(): void {
    const tab = this.currentTab();
    if (!tab) return;
    for (const id of paneIds(tab.root)) {
      const pane = this.#panes.get(id);
      if (pane) this.fitPane(pane);
    }
  }

  scheduleFit(): void {
    cancelAnimationFrame(this.#resizeFrame);
    this.#resizeFrame = requestAnimationFrame(() => this.fitAll());
  }

  saveWindow(): Promise<void> {
    const workspace = this.workspace;
    // A closed original tab returns without a tree.
    const source = workspace.tabs.find((item) => item.id === this.#originalTab.id) ??
      { id: this.#originalTab.id, title: this.#originalTab.title };
    const ids = workspace.tabs.flatMap((item) => paneIds(item.root));
    const owned = ids.map((id) => this.#panes.get(id)?.sessionId);
    if (owned.length === 0 || owned.some((id) => !id)) return this.#assignmentWrites;
    const sessions = owned.filter((id): id is string => Boolean(id));
    const shape = { ...source, windowTabs: workspace.tabs, activeWindowTabId: workspace.activeTabId };
    this.#assignmentWrites = this.#assignmentWrites.catch(() => {}).then(() =>
      this.#api().SetTerminalWindowTab(this.label, sessions, shape));
    return this.#assignmentWrites;
  }

  // ------------------------------------------------- per-session surfaces
  // Shared search, paste, and zoom controls; project editing stays in main.
  runSearch(incremental: boolean, backwards = false): void {
    const pane = this.activePane();
    if (!pane) return;
    const query = this.view.searchInput.value;
    if (!query) {
      pane.search.clearDecorations();
      this.view.searchResults.textContent = "";
      return;
    }
    const found = backwards
      ? pane.search.findPrevious(query, terminalSearchOptions(false))
      : pane.search.findNext(query, terminalSearchOptions(incremental));
    if (!found) this.view.searchResults.textContent = "No results";
  }

  openSearch(): void {
    this.view.searchBar.hidden = false;
    this.view.searchInput.focus();
    this.view.searchInput.select();
  }

  closeSearch(): void {
    this.activePane()?.search.clearDecorations();
    this.view.searchBar.hidden = true;
    this.view.searchResults.textContent = "";
    this.activePane()?.terminal.focus();
  }

  bindSearch(): void {
    const { searchInput, searchClose } = this.view;
    searchInput.addEventListener("input", () => this.runSearch(true));
    searchInput.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        this.closeSearch();
      } else if (event.key === "Enter") {
        event.preventDefault();
        this.runSearch(false, event.shiftKey);
      }
    });
    searchClose.addEventListener("click", () => this.closeSearch());
  }

  zoomPane(pane: WindowPane, nextSize: number): void {
    pane.fontSize = clampTerminalFontSize(nextSize);
    pane.terminal.options.fontSize = pane.fontSize;
    if (pane.profileId) {
      writeTerminalProfileFontSize(localStorage, pane.profileId, pane.fontSize);
    }
    requestAnimationFrame(() => this.fitPane(pane));
  }

  bindPaneKeys(pane: WindowPane): void {
    pane.search.onDidChangeResults((result) => {
      if (pane !== this.activePane()) return;
      this.view.searchResults.textContent = terminalSearchResultLabel(
        result,
        this.view.searchInput.value !== "",
      );
    });
    pane.terminal.attachCustomKeyEventHandler((event) => {
      const action = terminalKeyShortcut(
        event,
        terminalPlatform(),
        pane.terminal.hasSelection(),
      );
      if (!action) return true;
      if (event.type !== "keydown" || event.repeat) return false;
      event.preventDefault();
      switch (action) {
        case "search":
          this.openSearch();
          break;
        case "zoom-in":
        case "zoom-out":
        case "zoom-reset":
          this.zoomPane(pane, terminalZoomFontSize(action, pane.fontSize, pane.baseFontSize));
          break;
        case "copy":
          void this.copySelection(pane);
          break;
        case "select-all":
          pane.terminal.selectAll();
          break;
        case "clear":
          pane.terminal.clear();
          break;
        default:
          // Paste arrives through the DOM paste event below, where the
          // clipboard's own payload feeds the guard.
          return true;
      }
      return false;
    });
  }

  // Paste review does not use advisory shell state; alternate-screen output
  // alone cannot bypass multi-line review.
  bindPanePaste(pane: WindowPane): void {
    pane.terminal.textarea?.addEventListener("paste", (event) => {
      event.preventDefault();
      event.stopPropagation();
      void this.pasteText(pane, event.clipboardData?.getData("text") ?? "");
    });
  }

  /** The one paste path: the clipboard's payload and a scratchpad snippet alike. */
  async pasteText(pane: WindowPane, text: string): Promise<void> {
    const request = prepareClipboardPaste(
      text,
      { alternateScreen: pane.terminal.buffer.active.type === "alternate" },
    );
    await commitClipboardPaste(
      request,
      (pending) => Promise.resolve(window.confirm(
        `Paste into the terminal? ${pasteReviewSummary(pending)}.`,
      )),
      (pending) => pane.terminal.paste(pending),
    );
  }

  bindPaneInput(pane: WindowPane): void {
    pane.terminal.onData((data) => {
      for (const chunk of splitTerminalInput(terminalTextToBytes(data))) {
        pane.client?.sendInput(chunk);
      }
    });
    pane.terminal.onBinary((data) => {
      for (const chunk of splitTerminalInput(binaryStringToBytes(data))) {
        pane.client?.sendInput(chunk);
      }
    });
  }

  // A stream the renderer lost is claimed back or ends for good.
  onStreamState(pane: WindowPane, next: TerminalStreamClient, state: StreamState): void {
    if (pane.client !== next) return;
    pane.state = state;
    pane.changedAt = Date.now();
    this.renderStatus();
    // Only a stream that opened earns a fresh re-claim budget.
    if (state === "open") {
      pane.attempts = 0;
      // The stream owns the renderer lease only after attachment.
      // Re-send even an unchanged size when a new lease takes over.
      pane.resize.invalidate({ rows: pane.terminal.rows, columns: pane.terminal.cols });
      this.fitPane(pane);
    }
    if (state !== "closed" && state !== "error") return;
    // A normal closure means the shell's output ended: waiting for
    // its exit beats replaying the same scrollback forever.
    const disposition = streamCloseDisposition({
      outputEnded: state === "closed" && next.outputEnded,
      sessionEnded: pane.sessionEnded,
      recoverable: !pane.ended,
    });
    if (disposition === "ended") {
      pane.ended = true;
      this.setStatus(pane.exitStatus || streamOutputEndedNotice);
      this.renderInfo();
    } else if (disposition === "reclaim") {
      this.scheduleReclaim(pane);
    }
  }

  // Fresh single-use tickets and clients fence released renderers from the PTY.
  attach(pane: WindowPane, url: string, from: number, sessionEnded = false): void {
    pane.sequence = Number(from || 0);
    pane.sessionEnded = sessionEnded;
    const next: TerminalStreamClient = new TerminalStreamClient({
      createWebSocket: (streamUrl) => new WebSocket(streamUrl),
      // Resume from rendered bytes, not the socket position.
      writeOutput: (output, done) => pane.terminal.write(output, () => {
        pane.sequence += output.byteLength;
        done();
      }),
      onStateChange: (state) => this.onStreamState(pane, next, state),
      // Accept the server sequence when replay wrapped after ticket minting.
      onGap: (sequence) => {
        if (pane.client !== next) return;
        if (sequence !== null) pane.sequence = sequence;
        this.showGap();
      },
    });
    pane.client = next;
    next.connect(url);
  }

  // Reclaim unexpected stream loss from the rendered sequence with a bound.
  scheduleReclaim(pane: WindowPane): void {
    if (pane.reclaiming) return;
    pane.reclaiming = true;
    void reclaimStream({
      recoverable: () => !pane.ended,
      sequence: () => pane.sequence,
      wait: (delay) => new Promise((resolve) => window.setTimeout(resolve, delay)),
      claim: (fromSequence) => this.#api().ClaimTerminalStream(pane.sessionId, fromSequence),
      attach: (claim) => {
        if (claim.gap) this.showGap();
        this.attach(pane, claim.url, claim.fromSequence, streamClaimEnded(claim));
      },
      reclaiming: () => {
        pane.attempts += 1;
        this.setStatus(reclaimingStreamNotice);
      },
      exhausted: () => {
        this.setStatus(streamReclaimFailedNotice);
      },
    }, pane.attempts).finally(() => {
      pane.reclaiming = false;
    });
  }

  async connectPane(pane: WindowPane, streamUrl?: string): Promise<void> {
    this.bindPaneKeys(pane);
    this.bindPanePaste(pane);
    this.bindPaneInput(pane);
    const claim: TerminalStreamClaim = streamUrl ? { url: streamUrl, fromSequence: 0, gap: false }
      : await this.#api().ClaimTerminalStream(pane.sessionId, 0);
    if (claim.gap) this.showGap();
    // A shell that ended before this window claimed it still replays; its
    // stream then ends for good instead of being claimed back.
    if (streamClaimEnded(claim)) pane.ended = true;
    this.attach(pane, claim.url, claim.fromSequence, streamClaimEnded(claim));
  }

  // Subscribe before claims so exits during connection are not lost.
  listenForExits(): void {
    window.runtime?.EventsOnMultiple?.("terminal:exit", (payload) => {
      const exit = payload && typeof payload === "object"
        ? payload as { sessionId?: unknown; error?: unknown; exitCode?: unknown }
        : null;
      for (const pane of this.#panes.values()) {
        if (exit?.sessionId !== pane.sessionId) continue;
        pane.ended = true;
        pane.state = "closed";
        pane.failed = typeof exit.error === "string" && exit.error !== "";
        pane.changedAt = Date.now();
        pane.exitStatus = (typeof exit.error === "string" && exit.error) || `Exited (${exit.exitCode})`;
        this.setStatus(pane.exitStatus);
        this.renderInfo();
      }
    }, -1);
  }

  // The window's panes follow the app theme the same way the dock does.
  followAppTheme(): void {
    new MutationObserver(() => {
      const appTheme = document.documentElement.dataset.theme;
      for (const pane of this.#panes.values()) {
        applyTerminalTheme(
          pane.terminal,
          terminalProfileTheme(
            terminalThemeName(this.settingsForProfile(pane.profileId).theme, appTheme),
          ),
        );
      }
    }).observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  }

  bindWindowEvents(): void {
    const resizeObserver = new ResizeObserver(() => this.scheduleFit());
    resizeObserver.observe(this.view.host);
    window.addEventListener("resize", () => this.scheduleFit());
    window.addEventListener("focus", () => {
      this.scheduleFit();
      if (this.view.searchBar.hidden) this.activePane()?.terminal.focus();
    });
    window.addEventListener("pagehide", () => {
      // Flushes the note: the write is issued before anything is torn down.
      this.#scratchpad?.dispose();
      this.#diagnostics.dispose();
      resizeObserver.disconnect();
      cancelAnimationFrame(this.#resizeFrame);
      for (const pane of this.#panes.values()) pane.dispose();
    }, { once: true });
  }

  // ---------------------------------------------------------------- tabs

  iconButton(label: string, icon: string, action: () => unknown): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "terminal-tab-action";
    button.title = label;
    button.setAttribute("aria-label", label);
    if (icon === "close") button.append(terminalControlIcon("close"));
    else {
      const source = icon.startsWith("#") ? document.querySelector(`${icon} svg`) : null;
      if (source) button.append(source.cloneNode(true));
      else button.textContent = icon;
    }
    button.addEventListener("click", () => { void action(); });
    return button;
  }

  reportError(error: unknown): void {
    this.setStatus(messageFrom(error));
  }

  async closeTab(item: WorkspaceTab | undefined): Promise<void> {
    if (!item) return;
    const members = paneIds(item.root)
      .map((id) => this.#panes.get(id))
      .filter((pane): pane is WindowPane => pane !== undefined);
    const intent = detachedTabCloseIntent({
      tabCount: this.workspace.tabs.length,
      ended: members.every((pane) => pane.ended),
    });
    if (this.#busy || !intent.allowed) return;
    this.#busy = true;
    try {
      if (intent.confirm && !(await this.ctx.recent.showConfirmation({
        eyebrow: "Terminal", heading: `Close ${item.title}?`,
        detail: "This stops the shell and any programs running in this tab.",
        cancel: "Keep open", submit: "Close terminal",
      }))) return;
      for (const pane of members) {
        // A shell that already ended has nothing left to stop.
        await this.#api().CloseTerminalV2(this.generation, pane.sessionId, false).catch((error: unknown) => {
          if (!pane.ended) throw error;
        });
        pane.dispose();
      }
      for (const id of paneIds(item.root)) this.#panes.delete(id);
      this.#controller.dispatch({ type: "close-tab", tabId: item.id });
      await this.saveWindow();
    } catch (error) { this.reportError(error); }
    finally { this.#busy = false; this.renderTabs(); }
  }

  shellProfile(): DiscoveredTerminalProfile | undefined {
    return this.profiles.find((profile) => profile.kind === "shell");
  }

  async addTab(): Promise<void> {
    const shell = this.shellProfile();
    if (this.#busy || !shell || this.workspace.tabs.length >= maximumWorkspaceTabs) return;
    this.#busy = true;
    let createdSession = "";
    let added: WorkspaceTab | undefined;
    try {
      const current = this.currentTab();
      const cwd = (current && findTerminalPane(current.root, current.activePaneId)?.cwd) || this.projectRoot || "";
      const created = await this.#api().CreateTerminalV2(this.generation, shell.id, cwd, 24, 80);
      createdSession = created.sessionId;
      if (Number(created.generation) !== this.generation) throw new Error("Project changed while opening the terminal");
      const next = this.#controller.dispatch({ type: "create-tab", title: `Terminal ${this.workspace.tabs.length + 1}`, profileId: shell.id, cwd });
      if (!next) throw new Error("Could not create a terminal tab");
      added = this.currentTab();
      if (!added) throw new Error("Could not create a terminal tab");
      const pane = this.createPane(added.activePaneId, created.sessionId);
      pane.shellNonce = created.shellIntegration?.nonce ?? "";
      this.#splitView?.refresh(this.workspace);
      this.#splitView?.mountForPane(added.activePaneId)?.append(pane.host);
      await this.saveWindow();
      await this.connectPane(pane, created.streamUrl);
      this.scheduleFit();
      pane.terminal.focus();
    } catch (error) {
      if (createdSession) await this.#api().CloseTerminalV2(this.generation, createdSession, false).catch(() => {});
      if (added) {
        this.#panes.get(added.activePaneId)?.dispose();
        this.#panes.delete(added.activePaneId);
        this.#controller.dispatch({ type: "close-tab", tabId: added.id });
        await this.saveWindow().catch(() => {});
      }
      this.reportError(error);
    } finally { this.#busy = false; this.renderTabs(); }
  }

  #addButton: HTMLButtonElement | null = null;

  renderTabs(): void {
    const list = this.view.tabs;
    list.replaceChildren();
    for (const item of this.workspace.tabs) {
      const wrapper = document.createElement("div");
      wrapper.className = "terminal-tab-item";
      const button = document.createElement("button");
      button.type = "button";
      button.className = "terminal-tab";
      button.textContent = item.title || "Terminal";
      button.setAttribute("role", "tab");
      const ids = workspaceTabElementIds(item.id);
      button.id = ids.tabButtonId;
      button.setAttribute("aria-controls", ids.panelId);
      button.setAttribute("aria-selected", String(item.id === this.workspace.activeTabId));
      button.tabIndex = item.id === this.workspace.activeTabId ? 0 : -1;
      button.addEventListener("click", () => this.#controller.dispatch({ type: "select-tab", tabId: item.id }));
      wrapper.append(button);
      const close = this.iconButton(`Close ${item.title}`, "close", () => this.closeTab(item));
      if (this.workspace.tabs.length === 1) {
        close.disabled = true;
        close.title = detachedLastTabCloseTitle;
      }
      wrapper.append(close);
      list.append(wrapper);
    }
    const current = this.currentTab();
    this.view.host.dataset.singlePane = String(current ? paneIds(current.root).length === 1 : false);
    this.view.heading.textContent = "p-track";
    if (this.#addButton) {
      this.#addButton.disabled = this.#busy || !this.shellProfile() || this.workspace.tabs.length >= maximumWorkspaceTabs;
    }
  }

  selectRelative(delta: number): void {
    const all = this.workspace.tabs;
    const index = all.findIndex((item) => item.id === this.workspace.activeTabId);
    this.#controller.dispatch({ type: "select-tab", tabId: all[(index + delta + all.length) % all.length].id });
  }

  setupWindowTabs(): void {
    const list = this.view.tabs;
    const add = this.iconButton("New terminal tab (⌘T / Ctrl+Shift+T)", "+", () => this.addTab());
    this.#addButton = add;
    this.view.controls.append(
      add,
      this.iconButton("Search terminal output", "#terminal-search-open", () => this.openSearch()),
      this.iconButton("Decrease font size", "−", () => { const pane = this.activePane(); if (pane) this.zoomPane(pane, pane.fontSize - 1); }),
      this.iconButton("Increase font size", "+", () => { const pane = this.activePane(); if (pane) this.zoomPane(pane, pane.fontSize + 1); }),
      this.iconButton("Clear scrollback", "#terminal-clear", () => this.activePane()?.terminal.clear()),
    );
    list.addEventListener("keydown", (event) => {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      event.preventDefault();
      this.selectRelative(event.key === "ArrowLeft" ? -1 : 1);
      list.querySelector<HTMLElement>('[aria-selected="true"]')?.focus();
    });
    window.addEventListener("keydown", (event) => {
      const modifier = terminalPlatform() === "mac" ? event.metaKey : event.ctrlKey && event.shiftKey;
      if (event.type !== "keydown" || event.repeat || event.isComposing) return;
      if (event.ctrlKey && event.key === "Tab") {
        event.preventDefault();
        event.stopImmediatePropagation();
        this.selectRelative(event.shiftKey ? -1 : 1);
      } else if (modifier && event.key.toLowerCase() === "t") {
        event.preventDefault();
        event.stopImmediatePropagation();
        void this.addTab();
      } else if (modifier && event.key.toLowerCase() === "w") {
        event.preventDefault();
        event.stopImmediatePropagation();
        void this.closeTab(this.currentTab());
      }
    }, true);
    this.#controller.subscribe(() => {
      this.#splitView?.refresh(this.workspace);
      this.renderTabs();
      this.scheduleFit();
      if (this.view.searchBar.hidden) this.activePane()?.terminal.focus();
      this.renderStatus();
      void this.saveWindow().catch((error: unknown) => this.reportError(error));
    });
    this.renderTabs();
  }

  // A theme picked in the main window reaches this one through the shared
  // stored record; the OS preference path is already followed by initTheme.
  followStoredTheme(): void {
    window.addEventListener("storage", (event) => {
      if (event.key !== THEME_STORAGE_KEY && event.key !== null) return;
      this.ctx.settings.themeController.setTheme(event.key === null ? "system" : event.newValue);
    });
  }

  async start(sessions: readonly string[]): Promise<void> {
    // Sessions were recorded in pane order when the tab moved; the traversal
    // order survives normalization because the tree structure does.
    const paneOrder = this.workspace.tabs.flatMap((item) => paneIds(item.root));
    for (const [index, paneId] of paneOrder.entries()) {
      if (sessions[index]) this.createPane(paneId, sessions[index]);
    }
    this.followAppTheme();
    this.#splitView = new WorkspaceSplitView({
      container: this.view.host,
      controller: this.#controller,
      hostForPane: (paneId) => this.#panes.get(paneId)?.host ?? null,
      // Panes are closed where the tab lives — the chrome is hidden here.
      closePane: () => {},
      fitPanes: (paneIdList) => {
        for (const paneId of paneIdList) {
          const pane = this.#panes.get(paneId);
          if (pane) requestAnimationFrame(() => this.fitPane(pane));
        }
      },
    });
    this.bindSearch();
    this.view.info.hidden = false;
    this.#diagnostics.mount();
    this.mountScratchpad();
    this.listenForExits();
    for (const pane of this.#panes.values()) await this.connectPane(pane);
    this.bindWindowEvents();
    this.followStoredTheme();
    this.setupWindowTabs();
    this.fitAll();
    this.renderStatus();
    this.activePane()?.terminal.focus();
  }
}

function windowWorkspace(assignment: TerminalWindowAssignment): Workspace {
  return {
    version: 1,
    activeTabId: assignment.shape?.activeWindowTabId || assignment.shape?.id || "",
    tabs: assignment.shape?.windowTabs || [assignment.shape],
  };
}

export function createTerminalWindow(ctx: AppContext) {
  const { api } = ctx;

  async function waitForBridge(): Promise<Backend> {
    for (let attempt = 0; attempt < 50; attempt += 1) {
      try {
        return api();
      } catch {
        await new Promise((resolve) => window.setTimeout(resolve, 100));
      }
    }
    throw new Error("The desktop runtime is not ready");
  }

  async function startTerminalWindow(label: string): Promise<void> {
    const view = terminalWindowView();
    const setStatus = (message: string) => {
      view.status.textContent = message;
      view.status.dataset.connected = String(message === terminalWindowStatusLabel("open"));
    };
    view.section.hidden = false;
    view.section.dataset.macos = String(terminalPlatform() === "mac");
    view.gapDetail.textContent = terminalGapNotice;

    try {
      await waitForBridge();
      await loadTerminalFont();
      const assignment = await api().GetTerminalWindowTab(label);
      const sessions = assignment?.sessions;
      if (!assignment || !sessions || sessions.length === 0) {
        setStatus("This window no longer shows a terminal. Close it.");
        return;
      }
      const workspaceState = await api().GetWorkspaceState();
      const generation = Number(workspaceState?.generation || 0);

      const controller = new WorkspaceTabController(
        createCryptoIdFactory(),
        windowWorkspace(assignment),
        {
          // Every tab closes here, the original included; the reducer keeps the
          // last one, whose way out is the window's own close.
          allowAction: (action) =>
            ["resize-split", "focus-pane", "select-tab", "create-tab", "close-tab"].includes(action.type),
        },
      );
      const tab = controller.workspace.tabs.find((item) => item.id === assignment.shape.id);
      if (!tab) {
        setStatus("This window no longer shows a terminal. Close it.");
        return;
      }
      if (tab.title) {
        view.heading.textContent = "p-track";
        document.title = `Terminal — ${tab.title}`;
      }
      const profiles = await api().GetTerminalProfiles().catch(() => []);
      const detached = new DetachedTerminalWindow(
        ctx,
        label,
        generation,
        view,
        profiles,
        workspaceState?.project?.root ?? "",
        controller,
        tab,
      );
      await detached.start(sessions);
    } catch (error) {
      setStatus(messageFrom(error));
    }
  }

  function bind(): void {
    // Listeners bind after the assigned tab is known.
  }

  return {
    bind,
    startTerminalWindow,
  };
}

export type TerminalWindow = ReturnType<typeof createTerminalWindow>;
