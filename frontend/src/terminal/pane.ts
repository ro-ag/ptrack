import type { FitAddon } from "@xterm/addon-fit";
import type { SearchAddon } from "@xterm/addon-search";
import type { ISearchResultChangeEvent } from "@xterm/addon-search";
import { UnicodeGraphemesAddon } from "@xterm/addon-unicode-graphemes";
import { WebglAddon } from "@xterm/addon-webgl";
import type { IDisposable, Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

import { TerminalStreamClient } from "./client";
import type { StreamState } from "./client";
import { terminalPaneInputLabel } from "./accessibility";
import {
  terminalDiagnosticsTop,
  terminalDiagnosticView,
  type TerminalDiagnosticInput,
  type TerminalDiagnosticLayout,
  type TerminalDiagnosticProcess,
  type TerminalDiagnosticRenderer,
  type TerminalDiagnosticStream,
} from "./diagnostics";
import {
  completeLinkedLaunchTransaction,
  installedAgentProfiles,
  LinkedLaunchPersistenceStage,
  linkedAssociationPointer,
  persistUnlessLinkedLaunchStaged,
  selectedInstalledAgentProfile,
  type DiscoveredTerminalProfile,
  type InstalledAgentProfile,
  type LinkedLaunchRequest,
} from "./linked-launch";
import {
  commitTerminalAssociationMutation,
  terminalHasLinkedOrigin,
  type ActiveTerminalAssociation,
  type TerminalAssociationMutationResult,
} from "./association-editor";
import {
  terminalWritebackStateMatches,
  type TerminalWritebackKind,
  type TerminalWritebackPreview,
  type TerminalWritebackResult,
} from "./writeback";
import {
  binaryStringToBytes,
  commitClipboardPaste,
  isTerminalCompositionEvent,
  pasteReviewSummary,
  prepareClipboardPaste,
  splitTerminalInput,
  terminalKeyShortcut,
  terminalTextToBytes,
} from "./paste";
import type {
  ClipboardPasteRequest,
  TerminalShortcutAction,
} from "./paste";
import { terminalPlatform } from "./platform";
import { createTerminalRenderer } from "./renderer";
import {
  clampTerminalFontSize,
  defaultTerminalFontSize,
  minimumTerminalFontSize,
  maximumTerminalFontSize,
  readTerminalFontSize,
  readTerminalProfileFontSize,
  terminalZoomFontSize,
  terminalZoomLabel,
  writeTerminalProfileFontSize,
} from "./preferences";
import {
  loadTerminalFont,
  normalizeTerminalProfileSettings,
  terminalProfileClosesAfterExit,
  terminalProfileTheme,
  type NormalizedTerminalProfileSettings,
} from "./profile-settings";
import { terminalThemeName } from "../theme";
import {
  maximumWebglRecoveryAttempts,
  webglAttachAllowed,
  webglRecoveryAfterSuppression,
  webglRecoveryDelay,
  webglRecoveryPolicyAction,
  type WebglAttachSource,
} from "./renderer-recovery";
import {
  forceStopTerminalRecovery,
  resetTerminalWorkspaceRecovery,
  restartTerminalRecovery,
  retryTerminalRendererRecovery,
} from "./recovery-actions";
import {
  PopOutExitLedger,
  panesHoldPoppedOutTerminal,
  popOutTerminal,
  poppedOutCloseRefusedNotice,
  poppedOutExitNotice,
  poppedOutPaneNotice,
  reclaimStream,
  reclaimingStreamNotice,
  returnedWindowTabs,
  streamClaimEnded,
  streamCloseDisposition,
  streamLossIsRecoverable,
  streamOutputEndedNotice,
  streamReclaimFailedNotice,
  terminalGapNotice,
  terminalPopOutControl,
  type ReturnedWindowTab,
} from "./pop-out";
import { TerminalResizeDispatcher } from "./resize-dispatch";
import { terminalControlIcon } from "./control-icon";
import {
  addSnippet,
  clampScratchpadWidth,
  defaultScratchpadWidth,
  maximumScratchpadWidth,
  orderedSnippets,
  readScratchpadOpen,
  readScratchpadWidth,
  removeSnippet,
  ScratchpadSaver,
  scratchpadNotices,
  scratchpadSplitterWidth,
  snippetPreview,
  terminalBodyVisible,
  togglePinned,
  writeScratchpadOpen,
  writeScratchpadWidth,
  type Scratchpad,
  type ScratchpadSnippet,
} from "./scratchpad";
import { terminalSearchOptions, terminalSearchResultLabel } from "./search";
import { looksLikeSecret, secretCaptureNotice } from "./secrets";
import {
  applyShellSignal,
  initialShellState,
  nextShellCWDValidation,
  parseShellOSC,
  shellStatusLabel,
  type ShellIntegrationDescriptor,
  type ShellSignal,
  type ShellState,
} from "./shell-integration";
import {
  acknowledgePaneActivity,
  aggregateTabIndicator,
  paneIndicator,
  paneIndicatorChanged,
  recordExit,
  recordOutput,
  resetPaneActivity,
  type TerminalProfileKind,
} from "./activity";
import { readModernUnicodeSetting } from "./unicode";
import {
  readTerminalPreferenceOverrides,
  webglPreferredByPreference,
} from "../settings/preferences";
import {
  activeTerminalDescriptor,
  earlyExitCacheLimit,
  ensureStoppedWorkspaceRuntimes,
  paneRuntimeEventAccepted,
  paneRuntimeTransition,
  PaneRuntimeRegistry,
  runtimeDescriptorEditable,
  type PaneRuntime,
  type PaneRuntimeState,
  type PaneRuntimeTicket,
} from "./runtime";
import {
  closeIntentConfirmed,
  PaneLifecycleCoordinator,
  PendingSessionCloseCoordinator,
  PendingSessionCloseError,
  runDescriptorCloseIntent,
} from "./lifecycle";
import {
  createWorkspace,
  findTerminalPane,
  maximumPanesPerTab,
  maximumWorkspaceTabs,
  paneIds,
  type TerminalDescriptor,
  type AssociationPointerV1,
  type Workspace,
  type WorkspaceTab,
} from "../workspace/model";
import {
  clearTerminalWorkspaceAfterReplace,
  defaultDockRatio,
  loadTerminalWorkspace,
  normalizeDockRatio,
  repairWorkspaceDescriptors,
  saveTerminalWorkspace,
  savedWorkspaceCwds,
  type TerminalCWDValidation,
  WorkspacePersistenceScheduler,
} from "../workspace/persistence";
import { focusCycleIndex } from "../workspace/presentation";
import type { WorkspaceAction } from "../workspace/reducer";
import {
  structuralCloseFocusTarget,
  WorkspaceTabBar,
} from "../workspace/tab-bar";
import {
  createCryptoIdFactory,
  WorkspaceTabController,
} from "../workspace/tab-controller";
import {
  activeTabDockInteractionEligible,
  leafRects,
  paneFocusShortcutIntent,
  paneInDirection,
  preferredWebglPaneIds,
  terminalPanePresentationPolicy,
  WorkspaceSplitView,
  type PaneDirection,
} from "../workspace/split-view";

type DockState = PaneRuntimeState;

type TerminalProfile = DiscoveredTerminalProfile;

interface TerminalSession {
  sessionId: string;
  profileId: string;
  cwd: string;
  state: string;
  streamUrl: string;
  associationRevision?: number;
  linkedLaunch?: boolean;
  shellIntegration?: ShellIntegrationDescriptor;
}

interface TerminalStreamClaim {
  url: string;
  fromSequence: number;
  gap: boolean;
  /** The session state at mint time; an ended session only replays. */
  state?: string;
}

/** Payload of the event a closing terminal window frees its tab with. */
interface TerminalWindowClosed {
  generation?: number;
  label?: string;
  sessions?: string[];
  shape?: unknown;
}

interface TerminalExit {
  generation?: number;
  sessionId: string;
  exitCode: number;
  state: string;
  error?: string;
}

export interface TerminalBackend {
  GetTerminalProfiles(): Promise<TerminalProfile[]>;
  ValidateTerminalCWDs(cwds: string[]): Promise<TerminalCWDValidation[]>;
  CreateTerminal(
    profileID: string,
    cwd: string,
    rows: number,
    columns: number,
  ): Promise<TerminalSession>;
  LaunchLinkedAgent(
    profileID: string,
    cwd: string,
    rows: number,
    columns: number,
    association: LinkedLaunchRequest["association"],
  ): Promise<TerminalSession>;
  RollbackLinkedAgent(sessionID: string): Promise<void>;
  MutateTerminalAssociation(
    sessionID: string,
    expectedRevision: number,
    association?: AssociationPointerV1,
  ): Promise<TerminalAssociationMutationResult>;
  PreviewTerminalWriteback(
    sessionID: string,
    expectedRevision: number,
    kind: TerminalWritebackKind,
    content: string,
  ): Promise<TerminalWritebackPreview>;
  WriteTerminalMemory(
    sessionID: string,
    expectedRevision: number,
    requestID: string,
    kind: TerminalWritebackKind,
    content: string,
    confirmSummary: boolean,
  ): Promise<TerminalWritebackResult>;
  ResizeTerminal(sessionID: string, rows: number, columns: number): Promise<void>;
  CloseTerminal(sessionID: string, force: boolean): Promise<void>;
  OpenTerminalWindow(
    sessions: readonly string[],
    shape: WorkspaceTab,
  ): Promise<{ label: string }>;
  ClaimTerminalStream(
    sessionID: string,
    fromSequence: number,
  ): Promise<TerminalStreamClaim>;
  GetScratchpadV1(
    generation: number,
  ): Promise<{ generation: number; scratchpad: Scratchpad }>;
  SetScratchpadV1(
    generation: number,
    revision: number,
    scratchpad: Scratchpad,
  ): Promise<{ generation: number; revision: number }>;
}

interface MountOptions {
  backend: TerminalBackend;
  projectRoot: string;
  workspaceGeneration?: number;
  showError(error: unknown): void;
}

export interface TerminalDockHandle {
  ready: Promise<void>;
  agentProfiles(): Promise<InstalledAgentProfile[]>;
  launchLinked(request: LinkedLaunchRequest): Promise<void>;
  associationState(): ActiveTerminalAssociation | null;
  mutateAssociation(
    expected: ActiveTerminalAssociation,
    association?: AssociationPointerV1,
    accepts?: () => boolean,
  ): Promise<ActiveTerminalAssociation>;
  previewWriteback(
    expected: ActiveTerminalAssociation,
    kind: TerminalWritebackKind,
    content: string,
    accepts?: () => boolean,
  ): Promise<TerminalWritebackPreview>;
  writeback(
    expected: ActiveTerminalAssociation,
    requestID: string,
    kind: TerminalWritebackKind,
    content: string,
    confirmSummary: boolean,
    accepts?: () => boolean,
  ): Promise<TerminalWritebackResult>;
  setVisible(visible: boolean): void;
  setLayoutLocked(locked: boolean): void;
  /**
   * Applies the stored Unicode mode to every open pane. Settings owns the
   * preference and has already saved it; the dock only follows it.
   */
  setModernUnicode(enabled: boolean): void;
  /**
   * Starts the active tab's shell, exactly as the Open control would, and does
   * nothing while that control is unavailable.
   */
  startSession(): void;
  setApplicationOverlayOpen(open: boolean, focusTerminal: false): void;
  /**
   * Writes pending project-scoped edits (the scratchpad note) and resolves once
   * they settled, bounded so a stalled write cannot hold a project switch.
   * Awaited before a workspace transition, while the runtime still accepts
   * this dock's generation.
   */
  flushPending(): Promise<void>;
  dispose(): void;
}

interface PaneResources {
  host: HTMLElement;
  profileId: string;
  fontSize: number;
  shellState: ShellState;
  shellCWDRequest: number;
  lastShellCWD: string;
  terminal: Terminal;
  fit: FitAddon;
  search: SearchAddon;
  unicode: UnicodeGraphemesAddon | null;
  webgl: WebglAddon | null;
  webglContextLoss: IDisposable | null;
  client: TerminalStreamClient | null;
  /** Bytes rendered so far: the sequence a re-claim resumes from. */
  sequence: number;
  /** The last claim found the session already ended: its stream only replays. */
  sessionEnded: boolean;
  reclaiming: boolean;
  reclaimAttempts: number;
  observer: ResizeObserver | null;
  subscriptions: IDisposable[];
  eventDisposers: Array<() => void>;
  animationFrame: number | null;
  resizeDispatcher: TerminalResizeDispatcher | null;
  webglRecoveryTimer: number | null;
  webglRecoveryAttempts: number;
  webglRecoveryPaused: boolean;
  diagnosticChangedAt: number;
  disposed: boolean;
}

type DockPaneRuntime = PaneRuntime<TerminalSession, PaneResources>;

interface LinkedTabStage {
  tab: WorkspaceTab;
  paneId: string;
}

const minimumDockHeight = 180;
/** How long a workspace transition waits for the dock's last writes. */
const pendingFlushLimitMs = 2_000;
const defaultDockHeight = 300;
const terminalFontSizeStep = 1;

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
  const runtime = window.runtime;
  if (typeof runtime?.EventsOnMultiple !== "function") return () => {};
  return runtime.EventsOnMultiple(name, callback, -1);
}


function nativeClipboard(): {
  getText(): Promise<string>;
  setText(text: string): Promise<void>;
} {
  const readText = window.runtime?.ClipboardGetText;
  const writeText = window.runtime?.ClipboardSetText;
  if (typeof readText !== "function" || typeof writeText !== "function") {
    throw new Error("Native clipboard access is unavailable");
  }
  return {
    getText: () => readText(),
    setText: async (text) => {
      if ((await writeText(text)) !== true) {
        throw new Error("Native clipboard copy failed");
      }
    },
  };
}

class TerminalDock {
  readonly #backend: TerminalBackend;
  readonly #showError: (error: unknown) => void;
  readonly #workspaceGeneration: number;
  readonly #dock = requiredElement<HTMLElement>("#terminal-dock");
  readonly #workArea = requiredElement<HTMLElement>(".work-area");
  readonly #body = requiredElement<HTMLElement>("#terminal-body");
  readonly #host = requiredElement<HTMLElement>("#terminal-host");
  readonly #message = requiredElement<HTMLElement>("#terminal-message");
  readonly #status = requiredElement<HTMLElement>("#terminal-status");
  readonly #title = requiredElement<HTMLElement>("#terminal-title");
  readonly #profile = requiredElement<HTMLSelectElement>("#terminal-profile");
  readonly #cwd = requiredElement<HTMLInputElement>("#terminal-cwd");
  readonly #open = requiredElement<HTMLButtonElement>("#terminal-open");
  // The stopped pane's labelled start control: the Open control's twin, so it
  // shares its availability.
  readonly #startShell = requiredElement<HTMLButtonElement>("#terminal-start-shell");
  // The stopped-pane notice. It sits above the body while the body is hidden
  // and moves into the stopped pane itself when the body stays up (an open
  // scratchpad keeps it visible), so the start action is always reachable.
  readonly #empty = requiredElement<HTMLElement>("#terminal-empty");
  readonly #emptyHome = this.#empty.parentElement;
  readonly #start = requiredElement<HTMLButtonElement>("#terminal-start");
  readonly #popOut = requiredElement<HTMLButtonElement>("#terminal-pop-out");
  readonly #restart = requiredElement<HTMLButtonElement>("#terminal-restart");
  readonly #close = requiredElement<HTMLButtonElement>("#terminal-close");
  readonly #forceStop = requiredElement<HTMLButtonElement>("#terminal-force-stop");
  readonly #rendererRetry = requiredElement<HTMLButtonElement>(
    "#terminal-renderer-retry",
  );
  readonly #diagnosticsToggle = requiredElement<HTMLButtonElement>(
    "#terminal-diagnostics-toggle",
  );
  readonly #diagnostics = requiredElement<HTMLElement>("#terminal-diagnostics");
  readonly #diagnosticsClose = requiredElement<HTMLButtonElement>(
    "#terminal-diagnostics-close",
  );
  readonly #toolbar = requiredElement<HTMLElement>("#terminal-dock .terminal-toolbar");
  readonly #diagnosticProcess = requiredElement<HTMLElement>(
    "#terminal-diagnostic-process",
  );
  readonly #diagnosticStream = requiredElement<HTMLElement>(
    "#terminal-diagnostic-stream",
  );
  readonly #diagnosticRenderer = requiredElement<HTMLElement>(
    "#terminal-diagnostic-renderer",
  );
  readonly #diagnosticLayout = requiredElement<HTMLElement>(
    "#terminal-diagnostic-layout",
  );
  readonly #diagnosticUpdated = requiredElement<HTMLElement>(
    "#terminal-diagnostic-updated",
  );
  readonly #association = requiredElement<HTMLButtonElement>(
    "#terminal-link-context",
  );
  readonly #writeback = requiredElement<HTMLButtonElement>("#terminal-writeback");
  readonly #searchOpen = requiredElement<HTMLButtonElement>("#terminal-search-open");
  readonly #searchForm = requiredElement<HTMLFormElement>("#terminal-search");
  readonly #searchInput = requiredElement<HTMLInputElement>("#terminal-search-input");
  readonly #searchResults = requiredElement<HTMLElement>("#terminal-search-results");
  readonly #searchPrevious = requiredElement<HTMLButtonElement>(
    "#terminal-search-previous",
  );
  readonly #searchClose = requiredElement<HTMLButtonElement>("#terminal-search-close");
  readonly #zoomOut = requiredElement<HTMLButtonElement>("#terminal-zoom-out");
  readonly #zoomReset = requiredElement<HTMLButtonElement>("#terminal-zoom-reset");
  readonly #zoomIn = requiredElement<HTMLButtonElement>("#terminal-zoom-in");
  readonly #clear = requiredElement<HTMLButtonElement>("#terminal-clear");
  readonly #boardToggle = requiredElement<HTMLButtonElement>(
    "#board-panel-toggle",
  );
  readonly #terminalToggle = requiredElement<HTMLButtonElement>(
    "#terminal-panel-toggle",
  );
  readonly #separator = requiredElement<HTMLElement>("#terminal-resize");
  readonly #resetWorkspace = requiredElement<HTMLButtonElement>(
    "#terminal-reset-workspace",
  );
  readonly #runtimes = new PaneRuntimeRegistry<TerminalSession, PaneResources>();
  readonly #lifecycle: PaneLifecycleCoordinator<TerminalSession, PaneResources>;
  readonly #pendingSessionCloses: PendingSessionCloseCoordinator<PaneResources>;
  readonly #tabController: WorkspaceTabController;
  readonly #tabBar: WorkspaceTabBar;
  readonly #splitView: WorkspaceSplitView;
  readonly #ids = createCryptoIdFactory();
  readonly #projectRoot: string;
  readonly #loadedPersistedWorkspace: boolean;
  readonly #persistenceScheduler: WorkspacePersistenceScheduler;
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
  readonly #terminationModal = requiredElement<HTMLElement>(
    "#terminal-termination-modal",
  );
  readonly #terminationBackdrop = requiredElement<HTMLButtonElement>(
    "#terminal-termination-backdrop",
  );
  readonly #terminationCancel = requiredElement<HTMLButtonElement>(
    "#terminal-termination-cancel",
  );
  readonly #terminationConfirm = requiredElement<HTMLButtonElement>(
    "#terminal-termination-confirm",
  );
  readonly #terminationDetail = requiredElement<HTMLElement>(
    "#terminal-termination-detail",
  );
  readonly #contextMenu = requiredElement<HTMLElement>("#terminal-context-menu");
  readonly #menuCopy = requiredElement<HTMLButtonElement>("#terminal-menu-copy");
  readonly #menuPaste = requiredElement<HTMLButtonElement>("#terminal-menu-paste");
  readonly #menuSelectAll = requiredElement<HTMLButtonElement>(
    "#terminal-menu-select-all",
  );
  readonly #menuSearch = requiredElement<HTMLButtonElement>("#terminal-menu-search");
  readonly #menuClear = requiredElement<HTMLButtonElement>("#terminal-menu-clear");
  readonly #menuReset = requiredElement<HTMLButtonElement>("#terminal-menu-reset");
  readonly #stage = requiredElement<HTMLElement>("#terminal-stage");
  readonly #scratchpadToggle = requiredElement<HTMLButtonElement>(
    "#terminal-scratchpad-toggle",
  );
  readonly #scratchpad = requiredElement<HTMLElement>("#terminal-scratchpad");
  readonly #scratchpadSplitter = requiredElement<HTMLElement>(
    "#terminal-scratchpad-splitter",
  );
  readonly #scratchpadState = requiredElement<HTMLElement>(
    "#terminal-scratchpad-state",
  );
  readonly #scratchpadClose = requiredElement<HTMLButtonElement>(
    "#terminal-scratchpad-close",
  );
  readonly #scratchpadText = requiredElement<HTMLTextAreaElement>(
    "#terminal-scratchpad-text",
  );
  readonly #scratchpadAdd = requiredElement<HTMLButtonElement>(
    "#terminal-scratchpad-add",
  );
  readonly #scratchpadList = requiredElement<HTMLElement>(
    "#terminal-scratchpad-snippets",
  );
  readonly #scratchpadEmpty = requiredElement<HTMLElement>(
    "#terminal-scratchpad-empty",
  );
  readonly #scratchpadSaver: ScratchpadSaver;

  #dockHeight = defaultDockHeight;
  #dockRatio = defaultDockRatio;
  #boardHidden = false;
  #terminalHidden = false;
  #workspaceViewVisible = true;
  #layoutLocked = false;
  #applicationOverlayOpen = false;
  #panelVisibilityRevision = 0;
  #modernUnicodeEnabled = true;
  #fontSize = defaultTerminalFontSize;
  #defaultProfileId = "";
  #profiles: TerminalProfile[] = [];
  #profileKinds = new Map<string, TerminalProfileKind>();
  #profileSettings = new Map<string, NormalizedTerminalProfileSettings>();
  #profileFontSizes = new Map<string, number>();
  #earlyExit = new Map<string, TerminalExit>();
  #dragCleanup: (() => void) | null = null;
  #pasteResolve: ((confirmed: boolean) => void) | null = null;
  #terminationResolve: ((confirmed: boolean) => void) | null = null;
  #terminationPromise: Promise<boolean> | null = null;
  #terminationInvoker: HTMLElement | null = null;
  #clipboardWrite: Promise<void> = Promise.resolve();
  #pasteBusy = false;
  #pasteRequest = 0;
  #disposed = false;
  #resetPromise: Promise<void> | null = null;
  #diagnosticsOpen = false;
  // Follows the header while the popover is open: the toolbar wraps with the
  // dock width, and the popover must stay below it rather than over it.
  #diagnosticsObserver: ResizeObserver | null = null;
  #layoutDiagnosticState: TerminalDiagnosticLayout = "default";
  #layoutRepairCount = 0;
  #layoutDiagnosticChangedAt = Date.now();
  readonly #linkedPersistenceStage = new LinkedLaunchPersistenceStage();
  #linkedLaunchPaneIds = new Set<string>();
  /** Session id → the pane holding its place while it lives in a window. */
  #poppedOut = new Map<string, string>();
  /** Exits that arrive while a tab is moving into its window. */
  readonly #popOutExits = new PopOutExitLedger<TerminalExit>();
  /**
   * Popped-out sessions whose shell already ended in the window. Their held
   * pane shows the exit; pop-in only closes them, it never reopens a tab.
   */
  readonly #endedPoppedOut = new Set<string>();
  #authorizedRuntimeRemoval = new Set<string>();
  #dockDisposers: Array<() => void> = [];
  #scratchpadOpen = false;
  #scratchpadWidth = defaultScratchpadWidth;
  #scratchpadDragCleanup: (() => void) | null = null;

  constructor(options: MountOptions) {
    this.#backend = options.backend;
    this.#showError = options.showError;
    this.#projectRoot = options.projectRoot;
    this.#workspaceGeneration = options.workspaceGeneration ?? 0;
    const restored = loadTerminalWorkspace(
      localStorage,
      this.#projectRoot,
      (message) => this.#showError(new Error(message)),
    );
    this.#loadedPersistedWorkspace = restored.workspace !== null;
    this.#layoutDiagnosticState = restored.invalidReason !== null
      ? "discarded"
      : restored.workspace !== null
        ? "restored"
        : "default";
    this.#layoutDiagnosticChangedAt = Date.now();
    this.#dockRatio = restored.dockRatio;
    this.#persistenceScheduler = new WorkspacePersistenceScheduler(
      {
        setTimeout: (callback, delay) => window.setTimeout(callback, delay),
        clearTimeout: (handle) => window.clearTimeout(handle as number),
      },
      () => {
        persistUnlessLinkedLaunchStaged(this.#linkedPersistenceStage, () => {
          saveTerminalWorkspace(
            localStorage,
            this.#projectRoot,
            this.#tabController.workspace,
            this.#dockRatio,
          );
        });
      },
    );
    this.#scratchpadWidth = readScratchpadWidth(localStorage);
    this.#scratchpadOpen = readScratchpadOpen(localStorage);
    this.#scratchpadSaver = new ScratchpadSaver({
      // Zero means "no project is open", which the runtime treats as an
      // unfenced call: the dock is only mounted with an open workspace's
      // generation (>= 1), so zero disables scratchpad traffic entirely.
      generation: this.#workspaceGeneration,
      backend: {
        get: (generation) => this.#backend.GetScratchpadV1(generation),
        set: (generation, revision, scratchpad) =>
          this.#backend.SetScratchpadV1(generation, revision, scratchpad),
      },
      clock: {
        setTimeout: (callback, delay) => window.setTimeout(callback, delay),
        clearTimeout: (handle) => window.clearTimeout(handle as number),
      },
      setText: (text) => this.#writeClipboard(text),
      status: (text) => this.#setScratchpadStatus(text),
      applyRecord: (record, replaceLocalText) =>
        this.#applyScratchpadRecord(record, replaceLocalText),
      reportError: (error) => {
        if (!this.#disposed) this.#showError(error);
      },
    });
    this.#lifecycle = new PaneLifecycleCoordinator(this.#runtimes, {
      closeSession: (sessionId, force) =>
        this.#backend.CloseTerminal(sessionId, force),
      disposeResources: (resources) => this.#disposeResources(resources),
      deleteEarlyExit: (sessionId) => this.#earlyExit.delete(sessionId),
    });
    this.#pendingSessionCloses = new PendingSessionCloseCoordinator({
      forceClose: async (sessionId) => {
        this.#earlyExit.delete(sessionId);
        await this.#backend.CloseTerminal(sessionId, true);
      },
      resourcesDisposed: (resources) => resources.disposed,
      disposeResources: (resources) => this.#disposeResources(resources),
    });
    this.#tabController = new WorkspaceTabController(
      this.#ids,
      restored.workspace ?? undefined,
      {
        interceptAction: (action) => this.#defaultTabIntent(action),
      },
    );
    ensureStoppedWorkspaceRuntimes(this.#runtimes, this.#tabController.workspace);
    this.#tabBar = new WorkspaceTabBar({
      tabList: requiredElement<HTMLElement>("#terminal-tabs"),
      actionToolbar: requiredElement<HTMLElement>("#terminal-tab-actions"),
      newTabButton: requiredElement<HTMLButtonElement>("#terminal-new-tab"),
      controller: this.#tabController,
      closeIntent: (action) => void this.#handleStructuralClose(action),
      indicatorForTab: (tab) => aggregateTabIndicator(
        paneIds(tab.root),
        (paneId) => {
          const runtime = this.#runtimes.get(paneId);
          return runtime
            ? paneIndicator(
              runtime.activity,
              runtime.state,
              this.#isGenuinelyForeground(runtime),
            )
            : { kind: "closed", unread: false };
        },
      ),
    });
    this.#splitView = new WorkspaceSplitView({
      container: this.#host,
      controller: this.#tabController,
      hostForPane: (paneId) => this.#runtimes.get(paneId)?.resources?.host ?? null,
      closePane: (action) => void this.#handleStructuralClose(action),
      linkedOriginForPane: (paneId) => this.#linkedLaunchPaneIds.has(paneId) ||
        this.#runtimes.get(paneId)?.session?.linkedLaunch === true,
      fitPanes: (paneIdList) => this.#fitPanes(paneIdList),
    });
    // Open panes repaint in the new palette when the app theme changes.
    const themeObserver = new MutationObserver(() => this.#applyAppTheme());
    themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    this.#dockDisposers.push(
      () => themeObserver.disconnect(),
      this.#tabController.subscribe((workspace, previous) => {
        this.#reconcileWorkspace(workspace, previous);
        this.#markPersistenceDirty();
      }),
      eventsOn("terminal:exit", (payload: TerminalExit) =>
        this.#routeTerminalExit(payload),
      ),
      eventsOn("terminal:window-closed", (payload: TerminalWindowClosed) =>
        this.#popTerminalBackIn(payload),
      ),
    );
    this.#listen(this.#popOut, "click", () =>
      void this.#runOperation(() => this.#popOutTab()),
    );
    this.#listen(this.#open, "click", () =>
      void this.#runOperation((runtime) => this.#openTerminal(runtime)),
    );
    // The tab-row play control is the discoverable session start: it starts
    // a closed session, restarts an exited or failed one, and hides while a
    // live session is running.
    this.#listen(this.#start, "click", () =>
      void this.#runOperation((runtime) =>
        runtime.state === "closed"
          ? this.#openTerminal(runtime)
          : this.#restartTerminal(runtime),
      ),
    );
    this.#listen(this.#restart, "click", () =>
      void this.#runOperation((runtime) => this.#restartTerminal(runtime)),
    );
    this.#listen(this.#close, "click", () =>
      void this.#closeTerminal(this.#activeRuntime()),
    );
    this.#listen(this.#forceStop, "click", () =>
      void this.#runOperation((runtime) => this.#forceStopTerminal(runtime)),
    );
    this.#listen(this.#rendererRetry, "click", () => this.#retryActiveRenderer());
    this.#listen(this.#diagnosticsToggle, "click", () =>
      this.#setDiagnosticsOpen(!this.#diagnosticsOpen),
    );
    this.#listen(this.#diagnostics, "keydown", (event) => {
      const keyEvent = event as KeyboardEvent;
      if (keyEvent.key !== "Escape") return;
      keyEvent.preventDefault();
      this.#setDiagnosticsOpen(false, true);
    });
    this.#listen(this.#scratchpadToggle, "click", () =>
      this.#setScratchpadOpen(!this.#scratchpadOpen),
    );
    this.#listen(this.#scratchpadClose, "click", () => {
      this.#setScratchpadOpen(false);
      this.#scratchpadToggle.focus();
    });
    this.#listen(this.#scratchpadText, "input", () =>
      this.#scratchpadSaver.markText(this.#scratchpadText.value),
    );
    // Leaving the note writes it now. A project switch is reached by clicking
    // away first, and the runtime fences a write issued after the generation
    // has already moved, so the earlier the note lands the better.
    this.#listen(this.#scratchpadText, "blur", () => this.#flushScratchpad());
    this.#listen(this.#scratchpadAdd, "click", () => this.#addSelectionToScratchpad());
    this.#listen(this.#scratchpadSplitter, "pointerdown", (event) =>
      this.#beginScratchpadResize(event as PointerEvent),
    );
    this.#listen(this.#scratchpadSplitter, "keydown", (event) =>
      this.#resizeScratchpadFromKeyboard(event as KeyboardEvent),
    );
    this.#listen(this.#diagnosticsClose, "click", () =>
      this.#setDiagnosticsOpen(false, true),
    );
    // A popover: a press anywhere else dismisses it. The toggle is excluded
    // so its own click handler decides, rather than closing and reopening.
    this.#listen(document, "pointerdown", (event) => {
      if (!this.#diagnosticsOpen) return;
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (
        this.#diagnostics.contains(target) || this.#diagnosticsToggle.contains(target)
      ) return;
      this.#setDiagnosticsOpen(false);
    }, true);
    this.#listen(this.#resetWorkspace, "click", () =>
      void this.#resetTerminalWorkspace(),
    );
    this.#listen(this.#boardToggle, "click", () => {
      if (!this.#layoutLocked) this.#setBoardHidden(!this.#boardHidden);
    });
    this.#listen(this.#terminalToggle, "click", () => {
      if (!this.#layoutLocked) this.#setTerminalHidden(!this.#terminalHidden);
    });
    this.#terminalToggle.disabled = this.#layoutLocked;
    this.#modernUnicodeEnabled = readModernUnicodeSetting(localStorage);
    this.#listen(this.#startShell, "click", () => this.startSession());
    this.#listen(this.#profile, "change", () => {
      this.#updateEditableDescriptor({ profileId: this.#profile.value });
      this.#syncActiveProfileFontSize();
      this.#renderZoomState();
    });
    this.#listen(this.#cwd, "change", () =>
      this.#updateEditableDescriptor({ cwd: this.#cwd.value }),
    );
    this.#fontSize = readTerminalFontSize(localStorage);
    this.#renderZoomState();
    this.#listen(this.#searchOpen, "click", () => this.#openSearch());
    this.#listen(this.#zoomOut, "click", () =>
      this.#setFontSize(this.#fontSize - terminalFontSizeStep),
    );
    this.#listen(this.#zoomReset, "click", () =>
      this.#setFontSize(this.#activeProfileDefaultFontSize()),
    );
    this.#listen(this.#zoomIn, "click", () =>
      this.#setFontSize(this.#fontSize + terminalFontSizeStep),
    );
    this.#listen(this.#clear, "click", () => this.#clearBuffer());
    this.#listen(this.#searchForm, "submit", (event) => {
      event.preventDefault();
      this.#findNext();
    });
    this.#listen(this.#searchInput, "input", () => this.#updateSearch(true));
    this.#listen(this.#searchInput, "keydown", (event) => {
      const keyEvent = event as KeyboardEvent;
      if (keyEvent.key === "Escape") {
        keyEvent.preventDefault();
        this.#closeSearch();
      } else if (keyEvent.key === "Enter" && keyEvent.shiftKey) {
        keyEvent.preventDefault();
        this.#findPrevious();
      }
    });
    this.#listen(this.#searchPrevious, "click", () => this.#findPrevious());
    this.#listen(this.#searchClose, "click", () => this.#closeSearch());
    this.#listen(this.#separator, "pointerdown", (event) =>
      this.#beginDockResize(event as PointerEvent),
    );
    this.#listen(this.#separator, "keydown", (event) =>
      this.#resizeDockFromKeyboard(event as KeyboardEvent),
    );
    this.#listen(this.#pasteForm, "submit", (event) => {
      event.preventDefault();
      this.#finishPasteConfirmation(true);
    });
    this.#listen(this.#pasteBackdrop, "click", () =>
      this.#finishPasteConfirmation(false),
    );
    this.#listen(this.#pasteCancel, "click", () =>
      this.#finishPasteConfirmation(false),
    );
    this.#listen(this.#terminationBackdrop, "click", () =>
      this.#finishTerminationConfirmation(false),
    );
    this.#listen(this.#terminationCancel, "click", () =>
      this.#finishTerminationConfirmation(false),
    );
    this.#listen(this.#terminationConfirm, "click", () =>
      this.#finishTerminationConfirmation(true),
    );
    this.#listen(this.#terminationModal, "keydown", (event) => {
      const keyEvent = event as KeyboardEvent;
      if (keyEvent.key === "Escape") {
        keyEvent.preventDefault();
        this.#finishTerminationConfirmation(false);
      } else if (keyEvent.key === "Tab") {
        this.#trapTerminationFocus(keyEvent);
      }
    });
    this.#listen(this.#menuCopy, "click", () => {
      this.#hideContextMenu();
      void this.#copySelection();
    });
    this.#listen(this.#menuPaste, "click", () => {
      this.#hideContextMenu();
      const runtime = this.#activeRuntime();
      if (runtime.resources) void this.#requestNativePaste(runtime, runtime.resources);
    });
    this.#listen(this.#menuSelectAll, "click", () => {
      this.#hideContextMenu();
      const resources = this.#activeRuntime().resources;
      resources?.terminal.selectAll();
      this.#focusAfterApplicationOverlayClose(resources?.terminal);
    });
    this.#listen(this.#menuSearch, "click", () => {
      this.#hideContextMenu();
      this.#openSearch(false);
      this.#focusAfterApplicationOverlayClose(this.#searchInput);
    });
    this.#listen(this.#menuClear, "click", () => {
      this.#hideContextMenu();
      const resources = this.#activeRuntime().resources;
      this.#clearBuffer(false);
      this.#focusAfterApplicationOverlayClose(resources?.terminal);
    });
    this.#listen(this.#menuReset, "click", () => {
      this.#hideContextMenu();
      const resources = this.#activeRuntime().resources;
      this.#resetTerminal(false);
      this.#focusAfterApplicationOverlayClose(resources?.terminal);
    });
    this.#listen(this.#contextMenu, "keydown", (event) =>
      this.#navigateContextMenu(event as KeyboardEvent),
    );
    this.#listen(window, "beforeunload", () => this.dispose());
    this.#listen(window, "pagehide", () => {
      this.#flushPersistence();
      this.#flushScratchpad();
    });
    this.#listen(window, "focus", () => this.#recoverTerminalPresentation());
    this.#listen(window, "pageshow", () => this.#recoverTerminalPresentation());
    this.#listen(window, "resize", () => this.#recoverTerminalPresentation());
    this.#listen(document, "visibilitychange", () =>
      this.#handleDocumentVisibilityChange(),
    );
    this.#setShortcutLabels();
    this.#setScratchpadOpen(this.#scratchpadOpen, false);
    this.#setDockHeight(this.#heightForDockRatio(this.#dockRatio), false);
    this.#renderPanelVisibility();
    this.#renderState();
  }

  async initialize(): Promise<void> {
    try {
      await loadTerminalFont();
      const profiles = await this.#backend.GetTerminalProfiles();
      if (this.#disposed) return;
      this.#profiles = profiles.map((profile) => ({ ...profile }));
      this.#profile.replaceChildren();
      this.#profileKinds.clear();
      this.#profileSettings.clear();
      this.#profileFontSizes.clear();
      // The stored Settings record overrides the profile's own defaults for
      // terminals opened from here on; running sessions are untouched.
      const overrides = readTerminalPreferenceOverrides(localStorage);
      for (const profile of profiles) {
        this.#profileKinds.set(profile.id, profile.kind);
        const settings = normalizeTerminalProfileSettings({
          ...profile,
          fontFamily: overrides.fontFamily || profile.fontFamily,
          scrollback: overrides.scrollback || profile.scrollback,
        });
        this.#profileSettings.set(profile.id, settings);
        this.#profileFontSizes.set(
          profile.id,
          readTerminalProfileFontSize(localStorage, profile.id, settings.fontSize),
        );
        const option = document.createElement("option");
        option.value = profile.id;
        option.textContent = `${profile.name}${profile.kind === "agent" ? " · agent" : ""}`;
        this.#profile.append(option);
      }
      if (profiles.length === 0) {
        throw new Error("No installed terminal profiles were discovered");
      }
      this.#defaultProfileId = profiles.some(
          (profile) => profile.id === overrides.defaultProfileId,
        )
        ? overrides.defaultProfileId
        : profiles[0].id;
      this.#syncActiveProfileFontSize();
      const savedCwds = savedWorkspaceCwds(this.#tabController.workspace);
      let cwdValidations: TerminalCWDValidation[] | null = [];
      let cwdValidationUnavailable = false;
      if (savedCwds.length > 0) {
        try {
          cwdValidations = await this.#backend.ValidateTerminalCWDs(savedCwds);
        } catch {
          if (this.#disposed) return;
          cwdValidations = null;
          cwdValidationUnavailable = true;
        }
      }
      if (this.#disposed) return;
      const repair = repairWorkspaceDescriptors(
        this.#tabController.workspace,
        new Set(profiles.map((profile) => profile.id)),
        profiles[0].id,
        cwdValidations,
      );
      if (repair.workspace !== this.#tabController.workspace) {
        this.#tabController.replace(repair.workspace);
        this.#flushPersistence();
      }
      if (
        repair.repairedProfiles > 0 ||
        repair.repairedCwds > 0 ||
        cwdValidationUnavailable
      ) {
        this.#layoutDiagnosticState = "repaired";
        this.#layoutRepairCount = repair.repairedProfiles + repair.repairedCwds +
          (cwdValidationUnavailable ? 1 : 0);
        this.#layoutDiagnosticChangedAt = Date.now();
      }
      if (
        this.#loadedPersistedWorkspace &&
        (repair.workspace !== this.#tabController.workspace ||
          repair.repairedProfiles > 0 ||
          repair.repairedCwds > 0 ||
          cwdValidationUnavailable)
      ) {
        const message = cwdValidationUnavailable
          ? "Saved terminal working directories could not be validated; they will be checked when opened"
          : `Saved terminal workspace repaired (${repair.repairedProfiles} profile, ${repair.repairedCwds} working directory)`;
        this.#showError(new Error(message));
      }
      this.#renderState();
    } catch (error) {
      if (this.#disposed) return;
      this.#setState(this.#activeRuntime(), "failed", messageFrom(error));
      this.#showError(error);
    }
  }

  async #openTerminal(runtime: DockPaneRuntime): Promise<void> {
    const descriptor = this.#descriptorFor(runtime.paneId);
    if (
      this.#disposed ||
      runtime.state === "opening" ||
      runtime.state === "running" ||
      !descriptor ||
      !descriptor.pane.profileId
    ) return;
    if (this.#paneIsLinked(runtime, descriptor.association)) {
      throw new Error(
        "Linked agent tabs must be launched again from their plan or task",
      );
    }
    await this.#pendingSessionCloses.retryPending();
    if (this.#disposed || !this.#descriptorFor(runtime.paneId)) return;
    this.#lifecycle.prepareOpen(runtime.paneId);
    this.#teardownRuntime(runtime);
    runtime.activity = resetPaneActivity(
      this.#profileKinds.get(descriptor.pane.profileId) ?? null,
    );
    const ticket = this.#runtimes.begin(runtime.paneId);
    runtime.session = null;
    runtime.closing = false;
    runtime.title = "";
    this.#body.hidden = false;
    this.#message.hidden = true;
    this.#setState(runtime, "opening");
    let createdSession: TerminalSession | null = null;
    let resources: PaneResources | null = null;

    try {
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      if (!this.#accepts(runtime, ticket)) return;
      resources = this.#createRenderer(runtime, ticket);
      runtime.resources = resources;
      this.#splitView.mountForPane(runtime.paneId)?.append(resources.host);
      this.#updateWebglPolicy();
      if (this.#isActive(runtime)) this.#renderState();
      this.#fit(runtime, resources, false);

      const session = await this.#backend.CreateTerminal(
        descriptor.pane.profileId,
        descriptor.pane.cwd,
        resources.terminal.rows,
        resources.terminal.cols,
      );
      createdSession = session;
      try {
        if (!(await this.#attachCreatedSession(
          runtime,
          descriptor,
          ticket,
          resources,
          session,
        ))) {
          createdSession = null;
          return;
        }
      } catch (error) {
        createdSession = null;
        if (error instanceof PendingSessionCloseError) {
          if (!this.#disposed) this.#showError(error);
          return;
        }
        throw error;
      }
      createdSession = null;
    } catch (error) {
      if (createdSession) {
        this.#earlyExit.delete(createdSession.sessionId);
        await this.#backend.CloseTerminal(createdSession.sessionId, true).catch(() => {});
      }
      if (!this.#accepts(runtime, ticket)) {
        if (resources) this.#disposeResources(resources);
        return;
      }
      const profileKind = runtime.activity.profileKind;
      this.#teardownRuntime(runtime);
      runtime.activity = resetPaneActivity(profileKind);
      this.#setState(runtime, "failed", messageFrom(error));
      this.#showError(error);
    }
  }

  async #attachCreatedSession(
    runtime: DockPaneRuntime,
    descriptor: NonNullable<ReturnType<typeof activeTerminalDescriptor>>,
    ticket: PaneRuntimeTicket,
    resources: PaneResources,
    session: TerminalSession,
  ): Promise<boolean> {
    const accepted = await this.#pendingSessionCloses.settle({
      sessionId: session.sessionId,
      resources,
      accepts: () => this.#accepts(runtime, ticket),
    });
    if (!accepted) return false;
    runtime.session = session;
    if (session.linkedLaunch === true) {
      this.#linkedLaunchPaneIds.add(runtime.paneId);
      this.#splitView.refresh(this.#tabController.workspace);
    }
    runtime.activity = {
      ...runtime.activity,
      profileKind: this.#profileKinds.get(session.profileId) ??
        runtime.activity.profileKind,
    };
    this.#tabController.dispatch({
      type: "update-pane",
      tabId: descriptor.tabId,
      paneId: runtime.paneId,
      changes: { profileId: session.profileId, cwd: session.cwd },
    });
    this.#connectStream(runtime, ticket, resources, session.sessionId, session.streamUrl);

    const earlyExit = this.#earlyExit.get(session.sessionId);
    if (earlyExit) {
      this.#earlyExit.delete(session.sessionId);
      this.#handleExit(earlyExit, runtime, ticket);
    }
    this.#pruneEarlyExits();
    if (this.#isActive(runtime)) resources.terminal.focus();
    return true;
  }

  async #restartTerminal(runtime: DockPaneRuntime): Promise<void> {
    const descriptor = this.#descriptorFor(runtime.paneId);
    await restartTerminalRecovery({
      linked: this.#paneIsLinked(runtime, descriptor?.association),
      close: () => this.#lifecycle.close(runtime.paneId),
      accepted: () => !this.#disposed && Boolean(this.#descriptorFor(runtime.paneId)),
      open: async () => {
        this.#lifecycle.prepareOpen(runtime.paneId);
        runtime.busy = true;
        await this.#openTerminal(runtime);
      },
    });
  }

  /**
   * Move the active tab — split tree and all — into its own window (step 3
   * §3). Every pane's renderer is released first so the window can claim the
   * leases; if anything fails every session is claimed back here, because a
   * failed pop-out must never leave any session unowned, and a tab must move
   * whole or not at all.
   */
  async #popOutTab(): Promise<void> {
    const workspace = this.#tabController.workspace;
    const tab = workspace.tabs.find((candidate) => candidate.id === workspace.activeTabId);
    if (this.#disposed || !tab) return;
    const panes: { paneId: string; runtime: DockPaneRuntime; sessionId: string }[] = [];
    for (const paneId of paneIds(tab.root)) {
      const runtime = this.#runtimes.get(paneId);
      const sessionId = runtime?.session?.sessionId;
      // The control is only shown when every pane is running with a session;
      // a pane that lost its session since renders the control's gate stale,
      // and a partial move must be unrepresentable.
      if (!runtime || !sessionId) return;
      panes.push({ paneId, runtime, sessionId });
    }
    if (panes.length === 0) return;
    const shape = structuredClone(tab);
    // From the teardown on, the pane no longer owns the session and the
    // window does not yet listen: an exit in between is kept, not dropped.
    this.#popOutExits.begin(panes.map((pane) => pane.sessionId));
    const result = await popOutTerminal({
      release: () => {
        for (const pane of panes) this.#teardownRuntime(pane.runtime);
      },
      open: () =>
        this.#backend.OpenTerminalWindow(panes.map((pane) => pane.sessionId), shape),
      reclaim: async () => {
        // Every pane is tried: one refused claim must not strand the rest.
        let failure: unknown = null;
        for (const pane of panes) {
          try {
            await this.#claimSessionIntoPane(pane.runtime, pane.sessionId);
          } catch (error) {
            failure = failure ?? error;
            this.#setState(pane.runtime, "failed", messageFrom(error));
          }
        }
        if (failure !== null) throw failure;
      },
    });
    const exits = this.#popOutExits.finish();
    if (this.#disposed) return;
    if (result.outcome === "popped-out") {
      for (const pane of panes) {
        this.#poppedOut.set(pane.sessionId, pane.paneId);
        this.#setState(pane.runtime, "closed", poppedOutPaneNotice);
        const exit = exits.get(pane.sessionId);
        if (exit) this.#releaseHeldPane(exit, pane.paneId);
      }
      // The control that was just pressed is now hidden, so focus lands on the
      // pane's tab rather than falling back to the document.
      this.#focusWorkspaceSurvivor();
      return;
    }
    // Claimed back here: an exit that arrived mid-move ends the pane now.
    for (const pane of panes) {
      const exit = exits.get(pane.sessionId);
      const ticket = this.#runtimes.capture(pane.paneId);
      if (exit && ticket) this.#handleExit(exit, pane.runtime, ticket);
    }
    this.#showError(result.error);
  }

  /**
   * Attach a running session to a pane with a freshly minted ticket. The
   * sequence asked for is 0: a new renderer has no scrollback, so the server
   * replays everything it still retains and reports a gap when it kept less.
   */
  async #claimSessionIntoPane(
    runtime: DockPaneRuntime,
    sessionId: string,
  ): Promise<void> {
    const descriptor = this.#descriptorFor(runtime.paneId);
    if (this.#disposed || !descriptor) throw new Error("The terminal pane is gone");
    this.#teardownRuntime(runtime);
    this.#lifecycle.prepareOpen(runtime.paneId);
    const ticket = this.#runtimes.begin(runtime.paneId);
    this.#body.hidden = false;
    this.#message.hidden = true;
    this.#setState(runtime, "opening");
    const resources = this.#createRenderer(runtime, ticket);
    runtime.resources = resources;
    this.#updateWebglPolicy();
    this.#fit(runtime, resources, false);
    let claim: TerminalStreamClaim;
    try {
      claim = await this.#backend.ClaimTerminalStream(sessionId, 0);
    } catch (error) {
      if (this.#accepts(runtime, ticket)) this.#teardownRuntime(runtime);
      else this.#disposeResources(resources);
      throw error;
    }
    if (!this.#accepts(runtime, ticket)) {
      this.#disposeResources(resources);
      return;
    }
    runtime.session = {
      sessionId,
      profileId: descriptor.pane.profileId,
      cwd: descriptor.pane.cwd,
      state: "running",
      streamUrl: claim.url,
    };
    this.#connectStream(runtime, ticket, resources, sessionId, claim.url, claim);
    if (this.#isActive(runtime)) resources.terminal.focus();
  }

  /**
   * One stream client per attach. The client's single-use rule and its write
   * generation are what stop input from a released renderer reaching a
   * re-claimed PTY, so a re-attach mints a fresh ticket and a fresh client
   * rather than reopening the old one.
   */
  #connectStream(
    runtime: DockPaneRuntime,
    ticket: PaneRuntimeTicket,
    resources: PaneResources,
    sessionId: string,
    url: string,
    claim?: TerminalStreamClaim,
  ): void {
    resources.sequence = claim?.fromSequence ?? 0;
    let gapShown = claim?.gap === true;
    const client: TerminalStreamClient = new TerminalStreamClient({
      createWebSocket: (streamUrl) => new WebSocket(streamUrl),
      // The rendered byte count is the sequence: a re-claim resumes exactly
      // where the renderer stopped drawing, never where the socket stopped.
      writeOutput: (output, done) => resources.terminal.write(output, () => {
        resources.sequence += output.byteLength;
        done();
      }),
      // A superseded client says nothing: only the pane's current stream
      // drives its state.
      onStateChange: (state) => {
        if (resources.client === client) {
          this.#streamStateChanged(runtime, ticket, sessionId, state);
        }
      },
      onOutput: (byteLength) => {
        if (resources.client === client) {
          this.#recordPaneOutput(runtime, ticket, sessionId, byteLength);
        }
      },
      // The buffer wrapped again between the mint and the connect: the replay
      // starts later than the claim said, so the count restarts where it
      // actually does, or every later re-claim would ask for the wrong bytes.
      onGap: (sequence) => {
        if (resources.client !== client) return;
        if (sequence !== null) resources.sequence = sequence;
        if (!gapShown) {
          gapShown = true;
          resources.terminal.writeln(`\r\n[p-track] ${terminalGapNotice}\r\n`);
        }
      },
    });
    resources.client = client;
    resources.sessionEnded = claim !== undefined && streamClaimEnded(claim);
    client.connect(url);
    if (claim?.gap) resources.terminal.writeln(`\r\n[p-track] ${terminalGapNotice}\r\n`);
  }

  /**
   * Claim the session back after the stream ended without anyone asking it to
   * — a reload, a missed pong, a write stall. Bounded retries inside the
   * re-claim grace window; after that the session is gone and the pane says so.
   */
  #scheduleStreamReclaim(
    runtime: DockPaneRuntime,
    ticket: PaneRuntimeTicket,
    resources: PaneResources,
    sessionId: string,
  ): void {
    if (resources.reclaiming) return;
    resources.reclaiming = true;
    void reclaimStream({
      recoverable: () => this.#reclaimAccepted(runtime, ticket, resources, sessionId),
      sequence: () => resources.sequence,
      wait: (delay) => new Promise((resolve) => window.setTimeout(resolve, delay)),
      claim: (fromSequence) => this.#backend.ClaimTerminalStream(sessionId, fromSequence),
      attach: (claim) => {
        const superseded = resources.client;
        resources.client = null;
        superseded?.close();
        this.#connectStream(runtime, ticket, resources, sessionId, claim.url, claim);
        this.#setState(runtime, "running", "");
      },
      reclaiming: () => {
        resources.reclaimAttempts += 1;
        this.#setState(runtime, "running", reclaimingStreamNotice);
      },
      exhausted: () => this.#setState(runtime, "failed", streamReclaimFailedNotice),
    }, resources.reclaimAttempts).finally(() => {
      resources.reclaiming = false;
    });
  }

  #reclaimAccepted(
    runtime: DockPaneRuntime,
    ticket: PaneRuntimeTicket,
    resources: PaneResources,
    sessionId: string,
  ): boolean {
    return !this.#disposed &&
      this.#accepts(runtime, ticket) &&
      !resources.disposed &&
      runtime.session?.sessionId === sessionId &&
      streamLossIsRecoverable({
        state: runtime.state,
        closing: runtime.closing,
        hasSession: runtime.session !== null,
        hasRenderer: runtime.resources === resources,
      });
  }

  /**
   * Splits resized inside the terminal window come back by position: the
   * child can only resize, never restructure, so the returned tree pairs
   * one-to-one with the held tab's and each pair's ratio is applied by the
   * held split's own id. Anything malformed is simply not applied.
   */
  #applyReturnedShape(payload: TerminalWindowClosed): void {
    const returnedRoot = (payload?.shape as { root?: unknown } | undefined)?.root;
    if (!returnedRoot) return;
    const heldPaneId = (payload?.sessions ?? [])
      .map((sessionId) => this.#poppedOut.get(sessionId))
      .find((paneId) => paneId !== undefined);
    if (heldPaneId === undefined) return;
    const workspace = this.#tabController.workspace;
    const tab = workspace.tabs.find((candidate) =>
      paneIds(candidate.root).includes(heldPaneId)
    );
    if (!tab) return;
    const resizes: { splitId: string; ratio: number }[] = [];
    const collect = (mine: Workspace["tabs"][number]["root"], theirs: unknown): void => {
      if (mine.kind !== "split") return;
      const returned = theirs as { kind?: string; ratio?: number; first?: unknown; second?: unknown };
      if (returned?.kind !== "split") return;
      const ratio = Number(returned.ratio);
      if (Number.isFinite(ratio) && ratio !== mine.ratio) {
        resizes.push({ splitId: mine.splitId, ratio });
      }
      collect(mine.first, returned.first);
      collect(mine.second, returned.second);
    };
    collect(tab.root, returnedRoot);
    for (const resize of resizes) {
      this.#tabController.dispatch({
        type: "resize-split",
        tabId: tab.id,
        splitId: resize.splitId,
        ratio: resize.ratio,
      });
    }
  }

  /**
   * A terminal window closed and handed its tab back (§6). The panes that
   * held its place take the sessions — and the shape comes back as the
   * window last had it, so a split resized there stays resized here. A pane
   * that is gone or already busy closes its session cleanly rather than
   * orphaning it.
   */
  #popTerminalBackIn(payload: TerminalWindowClosed): void {
    if (this.#disposed) return;
    if (
      this.#workspaceGeneration !== 0 &&
      payload?.generation !== undefined &&
      payload.generation !== this.#workspaceGeneration
    ) return;
    this.#applyReturnedShape(payload);
    const ended = (sessionId: string) => this.#endedPoppedOut.delete(sessionId);
    // Tabs the window opened itself come back as new tabs: closing the window
    // returns everything it holds, and a running shell is never stopped
    // without being asked.
    const returned = returnedWindowTabs(
      payload ?? {},
      (sessionId) => this.#poppedOut.has(sessionId) || this.#endedPoppedOut.has(sessionId),
    );
    for (const sessionId of payload?.sessions ?? []) {
      if (!sessionId) continue;
      const paneId = this.#poppedOut.get(sessionId);
      if (paneId === undefined) {
        // A shell that ended in the window already told its held pane so.
        if (ended(sessionId)) {
          void this.#backend.CloseTerminal(sessionId, false).catch(() => {});
        }
        continue;
      }
      this.#poppedOut.delete(sessionId);
      const runtime = this.#runtimes.get(paneId);
      if (!runtime || runtime.session || runtime.state !== "closed" || runtime.busy) {
        void this.#backend.CloseTerminal(sessionId, false).catch(() => {});
        continue;
      }
      void this.#claimSessionIntoPane(runtime, sessionId).catch((error) => {
        if (!this.#disposed) this.#showError(error);
      });
    }
    for (const tab of returned) this.#returnWindowTab(tab);
  }

  /**
   * A tab the terminal window opened comes back as a new tab here. Only when
   * no tab can be added does the session end, and then it says so.
   */
  #returnWindowTab(returned: ReturnedWindowTab): void {
    const workspace = this.#tabController.dispatch({
      type: "create-tab",
      title: returned.title,
      ...(returned.profileId ? { profileId: returned.profileId } : {}),
      cwd: returned.cwd,
    });
    const tab = workspace?.tabs.find((candidate) => candidate.id === workspace.activeTabId);
    const runtime = tab ? this.#runtimes.ensure(tab.activePaneId) : null;
    if (!tab || !runtime || runtime.session || runtime.busy) {
      void this.#backend.CloseTerminal(returned.sessionId, false).catch(() => {});
      this.#showError(new Error(
        `${returned.title} could not be brought back from its window and was closed.`,
      ));
      return;
    }
    void this.#claimSessionIntoPane(runtime, returned.sessionId).catch((error) => {
      if (!this.#disposed) this.#showError(error);
    });
  }

  async #closeTerminal(runtime: DockPaneRuntime): Promise<void> {
    if (this.#disposed) return;
    try {
      if (!(await closeIntentConfirmed(
        [runtime],
        () => this.#confirmTermination(1),
      ))) return;
      const closing = this.#lifecycle.close(runtime.paneId);
      if (this.#isActive(runtime)) this.#renderState();
      await closing;
      this.#pruneEarlyExits();
      if (!this.#disposed && this.#isActive(runtime)) this.#renderState();
      if (!this.#disposed) this.#tabBar.refresh();
      if (!this.#disposed) this.#focusWorkspaceSurvivor();
    } catch (error) {
      if (!this.#disposed && this.#isActive(runtime)) this.#renderState();
      this.#showError(error);
    }
  }

  async #forceStopTerminal(runtime: DockPaneRuntime): Promise<void> {
    const result = await forceStopTerminalRecovery({
      capture: () => this.#runtimes.capture(runtime.paneId),
      currentSessionId: () => runtime.session?.sessionId ?? null,
      closing: () => runtime.closing,
      confirm: () => this.#confirmTermination(1, true),
      accepted: (ticket) => this.#accepts(runtime, ticket),
      close: () => {
        const closing = this.#lifecycle.close(runtime.paneId, true);
        if (this.#isActive(runtime)) this.#renderState();
        return closing;
      },
    });
    if (result !== "stopped") return;
    if (this.#isActive(runtime)) this.#renderState();
    this.#pruneEarlyExits();
    if (!this.#disposed) {
      this.#tabBar.refresh();
      this.#focusWorkspaceSurvivor();
    }
  }

  async #runOperation(
    operation: (runtime: DockPaneRuntime) => Promise<void>,
  ): Promise<void> {
    const runtime = this.#activeRuntime();
    if (this.#disposed || runtime.busy) return;
    runtime.busy = true;
    this.#renderState();
    try {
      await operation(runtime);
    } catch (error) {
      if (!this.#disposed) this.#showError(error);
    } finally {
      runtime.busy = false;
      if (!this.#disposed && this.#isActive(runtime)) this.#renderState();
    }
  }

  #openSearch(focus = true): void {
    const resources = this.#activeRuntime().resources;
    if (!resources || resources.disposed) return;
    this.#hideContextMenu();
    this.#searchForm.hidden = false;
    if (focus) {
      this.#searchInput.focus();
      this.#searchInput.select();
    }
    if (this.#searchInput.value) this.#updateSearch(false);
  }

  #closeSearch(focusTerminal = true): void {
    this.#searchForm.hidden = true;
    this.#searchResults.textContent = "";
    const resources = this.#activeRuntime().resources;
    if (!resources || resources.disposed) return;
    resources.search.clearDecorations();
    if (focusTerminal) resources.terminal.focus();
  }

  #updateSearch(incremental: boolean): void {
    const resources = this.#activeRuntime().resources;
    if (!resources || resources.disposed) return;
    const query = this.#searchInput.value;
    if (!query) {
      resources.search.clearDecorations();
      this.#searchResults.textContent = "";
      return;
    }
    const found = resources.search.findNext(
      query,
      terminalSearchOptions(incremental),
    );
    if (!found) this.#searchResults.textContent = "No results";
  }

  #findNext(): void {
    this.#updateSearch(false);
    this.#searchInput.focus();
  }

  #findPrevious(): void {
    const resources = this.#activeRuntime().resources;
    const query = this.#searchInput.value;
    if (!resources || resources.disposed || !query) return;
    const found = resources.search.findPrevious(query, terminalSearchOptions(false));
    if (!found) this.#searchResults.textContent = "No results";
    this.#searchInput.focus();
  }

  #renderSearchResults(result: ISearchResultChangeEvent): void {
    this.#searchResults.textContent = terminalSearchResultLabel(
      result,
      this.#searchInput.value.length > 0,
    );
  }

  #setFontSize(fontSize: number): void {
    this.#fontSize = clampTerminalFontSize(fontSize);
    const profileId = activeTerminalDescriptor(this.#tabController.workspace)?.pane.profileId;
    if (!profileId) return;
    this.#profileFontSizes.set(profileId, this.#fontSize);
    writeTerminalProfileFontSize(localStorage, profileId, this.#fontSize);
    this.#renderZoomState();
    for (const runtime of this.#runtimes.values()) {
      const resources = runtime.resources;
      if (!resources || resources.disposed || resources.profileId !== profileId) continue;
      resources.fontSize = this.#fontSize;
      resources.terminal.options.fontSize = this.#fontSize;
      if (this.#isPaneVisible(runtime.paneId)) {
        this.#fit(runtime, resources, true);
      }
    }
    this.#activeRuntime().resources?.terminal.focus();
  }

  #renderZoomState(): void {
    const resources = this.#activeRuntime().resources;
    this.#zoomReset.textContent = terminalZoomLabel(
      this.#fontSize,
      this.#activeProfileDefaultFontSize(),
    );
    this.#zoomReset.disabled = !resources;
    this.#zoomOut.disabled =
      !resources || this.#fontSize <= minimumTerminalFontSize;
    this.#zoomIn.disabled =
      !resources || this.#fontSize >= maximumTerminalFontSize;
  }

  #clearBuffer(focus = true): void {
    const resources = this.#activeRuntime().resources;
    if (!resources || resources.disposed) return;
    this.#closeSearch(false);
    resources.terminal.clear();
    if (focus) resources.terminal.focus();
  }

  #resetTerminal(focus = true): void {
    const runtime = this.#activeRuntime();
    const resources = runtime.resources;
    if (!resources || resources.disposed) return;
    this.#closeSearch(false);
    resources.terminal.reset();
    this.#fit(runtime, resources, true);
    if (focus) resources.terminal.focus();
  }

  #configureTerminalInput(
    runtime: DockPaneRuntime,
    resources: PaneResources,
    ticket: PaneRuntimeTicket,
  ): void {
    resources.terminal.attachCustomKeyEventHandler((event) => {
      if (isTerminalCompositionEvent(event)) return true;
      const paneShortcut = paneFocusShortcutIntent(
        event,
        terminalPlatform() === "mac",
      );
      if (paneShortcut) {
        event.preventDefault();
        event.stopPropagation();
        if (paneShortcut.focus) {
          this.#focusPaneInDirection(runtime, paneShortcut.direction);
        }
        return false;
      }
      const action = terminalKeyShortcut(
        event,
        terminalPlatform(),
        resources.terminal.hasSelection(),
      );
      if (!action) return true;
      event.preventDefault();
      event.stopPropagation();
      if (event.type === "keydown" && !event.repeat) {
        this.#handleTerminalShortcut(action, runtime, resources);
      }
      return false;
    });

    const interceptPaste = (event: Event) => {
      event.preventDefault();
      event.stopPropagation();
      void this.#requestNativePaste(runtime, resources);
    };
    const interceptRightMouseDown = (event: MouseEvent) => {
      if (event.button !== 2) return;
      event.preventDefault();
      event.stopPropagation();
    };
    const showContextMenu = (event: MouseEvent) => {
      event.preventDefault();
      event.stopPropagation();
      this.#showContextMenu(event.clientX, event.clientY, runtime, resources);
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
      if (!this.#isActive(runtime)) return;
      if (event.defaultPrevented) return;
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
        this.#focusAfterApplicationOverlayClose(resources.terminal);
      } else if (!this.#searchForm.hidden) {
        event.preventDefault();
        this.#closeSearch();
      }
    };
    const dismiss = () => this.#hideContextMenu();

    resources.host.addEventListener("paste", interceptPaste, true);
    resources.host.addEventListener("mousedown", interceptRightMouseDown, true);
    resources.host.addEventListener("contextmenu", showContextMenu, true);
    document.addEventListener("pointerdown", dismissOnPointer, true);
    document.addEventListener("keydown", dismissOnKey);
    window.addEventListener("blur", dismiss);
    window.addEventListener("resize", dismiss);
    resources.eventDisposers.push(() => {
      resources.host.removeEventListener("paste", interceptPaste, true);
      resources.host.removeEventListener("mousedown", interceptRightMouseDown, true);
      resources.host.removeEventListener("contextmenu", showContextMenu, true);
      document.removeEventListener("pointerdown", dismissOnPointer, true);
      document.removeEventListener("keydown", dismissOnKey);
      window.removeEventListener("blur", dismiss);
      window.removeEventListener("resize", dismiss);
    });
  }

  #handleTerminalShortcut(
    action: TerminalShortcutAction,
    runtime: DockPaneRuntime,
    resources: PaneResources,
  ): void {
    if (action === "copy") {
      void this.#copySelection(runtime, resources);
    } else if (action === "paste") {
      void this.#requestNativePaste(runtime, resources);
    } else if (action === "select-all") {
      resources.terminal.selectAll();
    } else if (action === "context-menu") {
      const bounds = resources.host.getBoundingClientRect();
      this.#showContextMenu(bounds.left + 24, bounds.top + 24, runtime, resources);
    } else if (action === "search") {
      this.#openSearch();
    } else if (
      action === "zoom-out" || action === "zoom-reset" || action === "zoom-in"
    ) {
      this.#setFontSize(terminalZoomFontSize(
        action,
        this.#fontSize,
        this.#activeProfileDefaultFontSize(),
      ));
    } else if (action === "clear") {
      this.#clearBuffer();
    }
  }

  async #copySelection(
    runtime = this.#activeRuntime(),
    resources = runtime.resources,
  ): Promise<void> {
    if (!resources || resources.disposed || !resources.terminal.hasSelection()) return;
    const ticket = this.#runtimes.capture(runtime.paneId);
    if (!ticket) return;
    const selection = resources.terminal.getSelection();
    const write = this.#clipboardWrite.then(() =>
      nativeClipboard().setText(selection),
    );
    this.#clipboardWrite = write.catch(() => {});
    try {
      await write;
      // Content-free persistence still holds: only an explicit copy reaches
      // the scratchpad, and only the text the user selected.
      if (this.#accepts(runtime, ticket)) this.#captureSnippet(selection);
    } catch (error) {
      if (this.#accepts(runtime, ticket) && !resources.disposed) {
        this.#showError(error);
      }
    } finally {
      if (this.#accepts(runtime, ticket) && !resources.disposed && this.#isActive(runtime)) {
        resources.terminal.focus();
      }
    }
  }

  #requestNativePaste(
    runtime: DockPaneRuntime,
    resources: PaneResources,
  ): Promise<void> {
    return this.#pasteText(runtime, resources, async (accepts) => {
      // Let a copy that is still on its way to the system clipboard land first,
      // so ⌘C immediately followed by ⌘V pastes what was just copied.
      await this.#clipboardWrite;
      return accepts() ? nativeClipboard().getText() : null;
    });
  }

  /**
   * The one paste path. Whatever the text comes from — the system clipboard or
   * a scratchpad snippet — it crosses the same ticket fence, the same
   * single-flight latch, and the same alternate-screen and multi-line review
   * decisions, so the two callers cannot drift apart.
   */
  async #pasteText(
    runtime: DockPaneRuntime,
    resources: PaneResources,
    readText: (accepts: () => boolean) => Promise<string | null>,
  ): Promise<void> {
    if (resources.disposed || runtime.state !== "running" || this.#pasteBusy) return;
    const ticket = this.#runtimes.capture(runtime.paneId);
    if (!ticket) return;
    this.#pasteBusy = true;
    const requestID = ++this.#pasteRequest;
    const accepts = () => this.#canPaste(runtime, resources, ticket, requestID);
    try {
      const text = await readText(accepts);
      if (text === null || !accepts()) return;
      const request = prepareClipboardPaste(text, {
        alternateScreen: resources.terminal.buffer.active.type === "alternate",
        shell: resources.shellState,
      });
      await commitClipboardPaste(
        request,
        (pending) => this.#confirmPaste(pending),
        (pending) => {
          if (accepts()) resources.terminal.paste(pending);
        },
      );
      if (accepts() && this.#pasteModal.hidden) resources.terminal.focus();
    } catch (error) {
      if (accepts()) this.#showError(error);
    } finally {
      if (requestID === this.#pasteRequest) this.#pasteBusy = false;
    }
  }

  #canPaste(
    runtime: DockPaneRuntime,
    resources: PaneResources,
    ticket: PaneRuntimeTicket,
    requestID: number,
  ): boolean {
    return (
      requestID === this.#pasteRequest &&
      this.#accepts(runtime, ticket) &&
      runtime.resources === resources &&
      this.#isActive(runtime) &&
      !resources.disposed &&
      runtime.state === "running" &&
      !runtime.closing
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
    this.#pasteDetail.textContent =
      `${pasteReviewSummary(request)}. Review the text before sending it to the terminal.`;
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
    const next = focusCycleIndex(focusable.length, current, event.shiftKey);
    event.preventDefault();
    focusable[next].focus();
  }

  #confirmTermination(paneCount: number, force = false): Promise<boolean> {
    if (this.#disposed) return Promise.resolve(false);
    if (this.#terminationPromise) return this.#terminationPromise;
    this.#terminationInvoker = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    this.#finishPasteConfirmation(false);
    this.#hideContextMenu();
    this.#terminationDetail.textContent = force
      ? "The selected terminal will be killed immediately after its access is revoked."
      : paneCount === 1
        ? "The live terminal will be terminated and its access revoked before closing."
        : `${paneCount} terminal panes will be terminated and their access revoked before closing.`;
    this.#terminationConfirm.textContent = force ? "Force stop" : "Terminate";
    this.#terminationModal.hidden = false;
    this.#terminationCancel.focus();
    this.#terminationPromise = new Promise<boolean>((resolve) => {
      this.#terminationResolve = resolve;
    });
    return this.#terminationPromise;
  }

  #finishTerminationConfirmation(confirmed: boolean): void {
    const resolve = this.#terminationResolve;
    const invoker = this.#terminationInvoker;
    this.#terminationResolve = null;
    this.#terminationPromise = null;
    this.#terminationInvoker = null;
    this.#terminationModal.hidden = true;
    if (resolve) resolve(confirmed);
    this.#focusAfterApplicationOverlayClose(invoker);
  }

  #trapTerminationFocus(event: KeyboardEvent): void {
    const focusable = [this.#terminationCancel, this.#terminationConfirm];
    const current = focusable.indexOf(document.activeElement as HTMLButtonElement);
    const next = focusCycleIndex(focusable.length, current, event.shiftKey);
    event.preventDefault();
    focusable[next].focus();
  }

  #showContextMenu(
    x: number,
    y: number,
    runtime: DockPaneRuntime,
    resources: PaneResources,
  ): void {
    this.#finishPasteConfirmation(false);
    this.#menuCopy.disabled = !resources.terminal.hasSelection();
    this.#menuPaste.disabled = runtime.state !== "running";
    this.#contextMenu.hidden = false;
    const width = this.#contextMenu.offsetWidth;
    const height = this.#contextMenu.offsetHeight;
    this.#contextMenu.style.left = `${Math.max(8, Math.min(x, window.innerWidth - width - 8))}px`;
    this.#contextMenu.style.top = `${Math.max(8, Math.min(y, window.innerHeight - height - 8))}px`;
    [
      this.#menuCopy,
      this.#menuPaste,
      this.#menuSelectAll,
      this.#menuSearch,
      this.#menuClear,
      this.#menuReset,
    ]
      .find((button) => !button.disabled)
      ?.focus();
  }

  #hideContextMenu(): void {
    this.#contextMenu.hidden = true;
  }

  #focusAfterApplicationOverlayClose(
    target: { readonly isConnected?: boolean; focus(): void } | null | undefined,
  ): void {
    if (!target) return;
    requestAnimationFrame(() => {
      if (
        !this.#disposed &&
        !this.#applicationOverlayOpen &&
        target.isConnected !== false
      ) target.focus();
    });
  }

  #navigateContextMenu(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      this.#hideContextMenu();
      this.#focusAfterApplicationOverlayClose(
        this.#activeRuntime().resources?.terminal,
      );
      return;
    }
    const buttons = [
      this.#menuCopy,
      this.#menuPaste,
      this.#menuSelectAll,
      this.#menuSearch,
      this.#menuClear,
      this.#menuReset,
    ].filter((button) => !button.disabled);
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
      terminalPlatform() === "mac"
        ? {
            copy: "⌘C",
            paste: "⌘V",
            selectAll: "⌘A",
            search: "⌘F",
            clear: "⌘K",
          }
        : {
            copy: "Ctrl+Shift+C",
            paste: "Ctrl+V",
            selectAll: "Ctrl+Shift+A",
            search: "Ctrl+Shift+F",
            clear: "",
          };
    requiredElement<HTMLElement>("#terminal-menu-copy-shortcut").textContent =
      labels.copy;
    requiredElement<HTMLElement>("#terminal-menu-paste-shortcut").textContent =
      labels.paste;
    requiredElement<HTMLElement>("#terminal-menu-select-all-shortcut").textContent =
      labels.selectAll;
    requiredElement<HTMLElement>("#terminal-menu-search-shortcut").textContent =
      labels.search;
    requiredElement<HTMLElement>("#terminal-menu-clear-shortcut").textContent =
      labels.clear;
  }

  #createRenderer(
    runtime: DockPaneRuntime,
    ticket: PaneRuntimeTicket,
  ): PaneResources {
    const descriptor = this.#descriptorFor(runtime.paneId);
    const profileId = descriptor?.pane.profileId || this.#defaultProfileId;
    const settings = this.#profileSettings.get(profileId) ??
      normalizeTerminalProfileSettings({});
    const fontSize = this.#profileFontSizes.get(profileId) ?? settings.fontSize;
    const host = document.createElement("div");
    host.className = "terminal-pane-host";
    host.hidden = !this.#isPaneVisible(runtime.paneId);
    (this.#splitView.mountForPane(runtime.paneId) ?? this.#host).append(host);
    const { terminal, fit, search, unicode } = createTerminalRenderer({
      settings: {
        ...settings,
        theme: terminalThemeName(settings.theme, document.documentElement.dataset.theme),
      },
      fontSize,
      modernUnicode: this.#modernUnicodeEnabled,
      onLinkError: (error) => {
        if (!this.#disposed) this.#showError(error);
      },
    });
    terminal.open(host);
    const tab = this.#tabController.workspace.tabs.find((candidate) =>
      paneIds(candidate.root).includes(runtime.paneId)
    );
    const paneIndex = tab ? paneIds(tab.root).indexOf(runtime.paneId) + 1 : 1;
    terminal.textarea?.setAttribute(
      "aria-label",
      terminalPaneInputLabel(tab?.title ?? "Terminal", paneIndex),
    );

    const resources: PaneResources = {
      host,
      profileId,
      fontSize,
      shellState: initialShellState,
      shellCWDRequest: 0,
      lastShellCWD: "",
      terminal,
      fit,
      search,
      unicode,
      webgl: null,
      webglContextLoss: null,
      client: null,
      sequence: 0,
      sessionEnded: false,
      reclaiming: false,
      reclaimAttempts: 0,
      observer: null,
      subscriptions: [],
      eventDisposers: [],
      animationFrame: null,
      resizeDispatcher: null,
      webglRecoveryTimer: null,
      webglRecoveryAttempts: 0,
      webglRecoveryPaused: false,
      diagnosticChangedAt: Date.now(),
      disposed: false,
    };
    resources.resizeDispatcher = new TerminalResizeDispatcher({
      now: () => performance.now(),
      setTimer: (callback, delay) => window.setTimeout(callback, delay),
      clearTimer: (timer) => window.clearTimeout(timer),
      accepted: () => Boolean(
        runtime.session && !resources.disposed && this.#accepts(runtime, ticket)
      ),
      dispatch: (size) => {
        const sessionID = runtime.session?.sessionId;
        if (!sessionID) return;
        void this.#backend.ResizeTerminal(sessionID, size.rows, size.columns).catch((error) => {
          resources.resizeDispatcher?.invalidate(size);
          if (!this.#disposed && this.#accepts(runtime, ticket) && !resources.disposed) {
            this.#showError(error);
          }
        });
      },
    });

    for (const identifier of [7, 133, 633] as const) {
      resources.subscriptions.push(
        terminal.parser.registerOscHandler(identifier, (payload) => {
          if (!this.#accepts(runtime, ticket) ||
            this.#profileKinds.get(profileId) === "agent") return true;
          const signal = parseShellOSC(
            identifier,
            payload,
            runtime.session?.shellIntegration?.nonce ?? "",
          );
          if (signal) this.#handleShellSignal(runtime, resources, ticket, signal);
          return true;
        }),
      );
    }

    this.#configureTerminalInput(runtime, resources, ticket);
    resources.subscriptions.push(
      terminal.onData((data) => {
        if (!this.#accepts(runtime, ticket)) return;
        const bytes = terminalTextToBytes(data);
        for (const chunk of splitTerminalInput(bytes)) resources.client?.sendInput(chunk);
      }),
      terminal.onBinary((data) => {
        if (!this.#accepts(runtime, ticket)) return;
        for (const chunk of splitTerminalInput(binaryStringToBytes(data))) {
          resources.client?.sendInput(chunk);
        }
      }),
      terminal.onSelectionChange(() => {
        if (this.#accepts(runtime, ticket) && this.#isActive(runtime)) {
          this.#renderScratchpadSelection();
        }
      }),
      terminal.onTitleChange((title) => {
        if (this.#accepts(runtime, ticket) && title) {
          runtime.title = title;
          if (this.#isActive(runtime)) this.#title.textContent = title;
        }
      }),
      search.onDidChangeResults((result) => {
        if (this.#accepts(runtime, ticket) && this.#isActive(runtime)) {
          this.#renderSearchResults(result);
        }
      }),
    );

    if ("ResizeObserver" in window) {
      resources.observer = new ResizeObserver(() => {
        if (resources.animationFrame !== null) return;
        resources.animationFrame = requestAnimationFrame(() => {
          resources.animationFrame = null;
          if (!this.#accepts(runtime, ticket)) return;
          this.#fit(runtime, resources, true);
        });
      });
      resources.observer.observe(host);
    }

    return resources;
  }

  #attachWebgl(
    runtime: DockPaneRuntime,
    resources: PaneResources,
    ticket: PaneRuntimeTicket,
    source: WebglAttachSource = "policy",
  ): void {
    if (!webglAttachAllowed({
      disposed: resources.disposed,
      attached: resources.webgl !== null,
      timerPending: resources.webglRecoveryTimer !== null,
      attempts: resources.webglRecoveryAttempts,
      accepted: this.#accepts(runtime, ticket),
      preferred: this.#shouldUseWebgl(runtime.paneId),
      terminalHidden: this.#terminalHidden,
      documentHidden: document.visibilityState === "hidden",
    }, source)) return;
    let webgl: WebglAddon | null = null;
    try {
      webgl = new WebglAddon();
      resources.terminal.loadAddon(webgl);
      resources.webgl = webgl;
      if (resources.webglRecoveryTimer !== null) {
        window.clearTimeout(resources.webglRecoveryTimer);
        resources.webglRecoveryTimer = null;
      }
      resources.webglRecoveryAttempts = 0;
      resources.webglRecoveryPaused = false;
      resources.diagnosticChangedAt = Date.now();
      const attached = webgl;
      const contextLoss = attached.onContextLoss(() => {
        contextLoss.dispose();
        if (resources.webgl === attached) {
          resources.webgl = null;
          resources.webglContextLoss = null;
        }
        attached.dispose();
        if (resources.disposed || !this.#accepts(runtime, ticket)) return;
        resources.diagnosticChangedAt = Date.now();
        resources.terminal.refresh(0, resources.terminal.rows - 1);
        this.#scheduleWebglRecovery(runtime, resources, ticket);
        if (this.#isActive(runtime)) this.#renderState();
      });
      resources.webglContextLoss = contextLoss;
      if (this.#isActive(runtime)) this.#renderState();
    } catch {
      try {
        webgl?.dispose();
      } catch {
        // A partial activation is abandoned within the bounded retry policy.
      }
      resources.diagnosticChangedAt = Date.now();
      this.#scheduleWebglRecovery(runtime, resources, ticket);
      if (this.#isActive(runtime)) this.#renderState();
    }
  }

  #scheduleWebglRecovery(
    runtime: DockPaneRuntime,
    resources: PaneResources,
    ticket: PaneRuntimeTicket,
  ): void {
    const delay = webglRecoveryDelay({
      disposed: resources.disposed,
      attached: resources.webgl !== null,
      timerPending: resources.webglRecoveryTimer !== null,
      attempts: resources.webglRecoveryAttempts,
      accepted: this.#accepts(runtime, ticket),
      preferred: this.#shouldUseWebgl(runtime.paneId),
      terminalHidden: this.#terminalHidden,
      documentHidden: document.visibilityState === "hidden",
    });
    if (delay === null) return;
    resources.diagnosticChangedAt = Date.now();
    resources.webglRecoveryTimer = window.setTimeout(() => {
      resources.webglRecoveryTimer = null;
      resources.webglRecoveryAttempts += 1;
      resources.diagnosticChangedAt = Date.now();
      this.#attachWebgl(runtime, resources, ticket, "retry");
    }, delay);
    resources.webglRecoveryPaused = false;
  }

  #routeTerminalExit(payload: TerminalExit): void {
    if (!payload?.sessionId) return;
    if (
      this.#workspaceGeneration !== 0 &&
      payload.generation !== this.#workspaceGeneration
    ) return;
    if (this.#popOutExits.record(payload)) return;
    const heldPaneId = this.#poppedOut.get(payload.sessionId);
    if (heldPaneId !== undefined) {
      this.#releaseHeldPane(payload, heldPaneId);
      return;
    }
    const runtime = this.#runtimes.findBySessionId(payload.sessionId);
    const ticket = runtime ? this.#runtimes.capture(runtime.paneId) : null;
    if (runtime && ticket) {
      this.#handleExit(payload, runtime, ticket);
    } else {
      if (this.#openingRuntimeCount() === 0) return;
      this.#earlyExit.delete(payload.sessionId);
      this.#earlyExit.set(payload.sessionId, payload);
      this.#pruneEarlyExits();
    }
  }

  /**
   * A popped-out shell ended in its window — closed there, or exited on its
   * own. The pane that held its place stops holding it: nothing is coming
   * back, so the notice that kept the pane closed gives way to an ordinary
   * exited pane the user can restart or close. Without this the held pane
   * kept promising a terminal until the window itself was closed.
   */
  #releaseHeldPane(result: TerminalExit, paneId: string): void {
    this.#poppedOut.delete(result.sessionId);
    this.#endedPoppedOut.add(result.sessionId);
    const runtime = this.#runtimes.get(paneId);
    if (!runtime || runtime.session || runtime.state !== "closed" || runtime.busy) {
      return;
    }
    const failed = result.state === "failed";
    runtime.activity = recordExit(
      runtime.activity,
      runtime.activity.profileKind,
      failed ? "failed" : "exited",
      result.exitCode,
      result.error,
      Date.now(),
    );
    this.#acknowledgeIfForeground(runtime);
    this.#setState(runtime, failed ? "failed" : "exited", poppedOutExitNotice(result));
  }

  #handleExit(
    result: TerminalExit,
    runtime: DockPaneRuntime,
    ticket: PaneRuntimeTicket,
  ): void {
    if (!paneRuntimeEventAccepted({
      ticketAccepted: this.#accepts(runtime, ticket),
      closing: runtime.closing,
      sessionId: runtime.session?.sessionId ?? null,
      eventSessionId: result.sessionId,
    })) return;
    const detail = result.error
      ? result.error
      : `Process exited with code ${result.exitCode}`;
    const transition = paneRuntimeTransition(runtime.state, {
      kind: "process-exit",
      failed: result.state === "failed",
      detail,
    });
    if (!transition) return;
    const resources = runtime.resources;
    resources?.resizeDispatcher?.dispose();
    if (resources) resources.resizeDispatcher = null;
    runtime.activity = recordExit(
      runtime.activity,
      runtime.activity.profileKind,
      result.state === "failed" ? "failed" : "exited",
      result.exitCode,
      result.error,
      Date.now(),
    );
    this.#acknowledgeIfForeground(runtime);
    this.#setState(runtime, transition.state, transition.detail);
    const behavior = this.#profileSettings.get(runtime.session?.profileId ?? "")
      ?.exitBehavior ?? "keep";
    if (terminalProfileClosesAfterExit(behavior, result.exitCode)) {
      void this.#lifecycle.close(runtime.paneId).then(() => {
        if (!this.#disposed && this.#isActive(runtime)) this.#renderState();
      }).catch((error) => {
        if (!this.#disposed) this.#showError(error);
      });
    }
  }

  #handleShellSignal(
    runtime: DockPaneRuntime,
    resources: PaneResources,
    ticket: PaneRuntimeTicket,
    signal: ShellSignal,
  ): void {
    if (!this.#accepts(runtime, ticket) || resources.disposed) return;
    resources.shellState = applyShellSignal(resources.shellState, signal, performance.now());
    if (this.#isActive(runtime)) this.#renderState();
    if (signal.kind !== "cwd" || !signal.authenticated || !runtime.session) return;

    const decision = nextShellCWDValidation(
      resources.shellCWDRequest,
      resources.lastShellCWD,
      signal.cwd,
    );
    resources.shellCWDRequest = decision.request;
    if (!decision.validate) return;
    const request = decision.request;
    const sessionId = runtime.session.sessionId;
    void this.#backend.ValidateTerminalCWDs([signal.cwd]).then((results) => {
      if (!this.#accepts(runtime, ticket) || resources.disposed ||
        request !== resources.shellCWDRequest ||
        runtime.session?.sessionId !== sessionId) return;
      const validation = results[0];
      if (!validation || validation.requested !== signal.cwd ||
        !validation.valid || !validation.cwd) return;
      const descriptor = this.#descriptorFor(runtime.paneId);
      if (!descriptor) return;
      resources.lastShellCWD = validation.cwd;
      this.#tabController.dispatch({
        type: "update-pane",
        tabId: descriptor.tabId,
        paneId: runtime.paneId,
        changes: { cwd: validation.cwd },
      });
    }).catch(() => {
      // OSC paths are untrusted terminal presentation hints. Invalid or stale
      // candidates are ignored without surfacing their content in diagnostics.
    });
  }

  #recordPaneOutput(
    runtime: DockPaneRuntime,
    ticket: PaneRuntimeTicket,
    sessionId: string,
    byteLength: number,
  ): void {
    if (
      byteLength <= 0 ||
      !this.#accepts(runtime, ticket) ||
      runtime.session?.sessionId !== sessionId
    ) return;
    const foreground = this.#isGenuinelyForeground(runtime);
    const previousIndicator = paneIndicator(
      runtime.activity,
      runtime.state,
      foreground,
    );
    runtime.activity = recordOutput(
      runtime.activity,
      foreground,
      Date.now(),
    );
    const nextIndicator = paneIndicator(
      runtime.activity,
      runtime.state,
      foreground,
    );
    if (paneIndicatorChanged(previousIndicator, nextIndicator)) {
      this.#tabBar.refresh();
    }
  }

  #handleDocumentVisibilityChange(): void {
    if (this.#disposed) return;
    if (document.visibilityState === "hidden") {
      this.#flushPersistence();
      this.#flushScratchpad();
      this.#updateWebglPolicy();
      return;
    }
    this.#recoverTerminalPresentation();
  }

  #recoverTerminalPresentation(): void {
    if (this.#disposed || document.visibilityState === "hidden") return;
    for (const paneId of this.#activeTabPaneIds()) {
      const resources = this.#runtimes.get(paneId)?.resources;
      if (!this.#applicationOverlayOpen && resources && !resources.disposed &&
        resources.webgl === null &&
        resources.webglRecoveryTimer === null) {
        resources.webglRecoveryAttempts = 0;
        resources.webglRecoveryPaused = false;
      }
    }
    this.#updateWebglPolicy();
    this.#fitPanes(this.#activeTabPaneIds());
    this.#acknowledgeIfForeground(this.#activeRuntime());
  }

  // ---- Scratchpad -------------------------------------------------------
  //
  // The panel is dock furniture: it hides with the dock and refits the panes
  // whenever its geometry changes. Every rule it obeys lives in ./scratchpad —
  // snippet limits and ordering, the width clamp, the storage keys, and the
  // load / dirty / write / conflict state machine (`ScratchpadSaver`). What
  // follows is DOM wiring only.

  #scratchpadRecord(): Scratchpad {
    return this.#scratchpadSaver.record;
  }

  #setScratchpadOpen(open: boolean, persist = true): void {
    this.#scratchpadOpen = open;
    this.#scratchpad.hidden = !open;
    this.#scratchpadSplitter.hidden = !open;
    this.#scratchpadToggle.setAttribute("aria-pressed", String(open));
    const label = open ? "Hide scratchpad" : "Show scratchpad";
    this.#scratchpadToggle.setAttribute("aria-label", label);
    this.#scratchpadToggle.title = label;
    // The dock's closed-and-collapsed CSS rule steps aside while this is
    // "true", so the panel stays usable with no live session.
    this.#dock.dataset.scratchpadOpen = String(open);
    if (persist) writeScratchpadOpen(localStorage, open);
    this.#applyScratchpadWidth();
    if (!open) this.#scratchpadDragCleanup?.();
    requestAnimationFrame(() => {
      if (!this.#disposed) this.#fitPanes(this.#activeTabPaneIds());
    });
    // Recomputes `#terminal-body`'s visibility now that the dataset flag
    // changed; harmless to call again since renderState is otherwise called
    // right after this during mount.
    this.#renderState();
    if (!open) {
      this.#scratchpadSaver.flush();
      return;
    }
    this.#renderScratchpadSelection();
    void this.#scratchpadSaver.ensureLoaded();
  }

  /**
   * A hidden dock measures zero. Reporting that as the body width would clamp
   * a stored panel width down to the floor, so an unmeasurable dock keeps only
   * the lower bound (see `clampScratchpadWidth`).
   */
  #scratchpadBodyWidth(): number {
    return this.#stage.clientWidth > 0 ? this.#body.clientWidth : 0;
  }

  #applyScratchpadWidth(): void {
    const width = this.#scratchpadWidth;
    this.#scratchpad.style.width = `${width}px`;
    // Body-level overlays (terminal search) step aside for an open panel.
    this.#body.style.setProperty(
      "--terminal-scratchpad-gutter",
      this.#scratchpadOpen ? `${width + scratchpadSplitterWidth}px` : "0px",
    );
    this.#scratchpadSplitter.setAttribute("aria-valuenow", String(width));
    const maximum = maximumScratchpadWidth(this.#scratchpadBodyWidth());
    if (Number.isFinite(maximum)) {
      this.#scratchpadSplitter.setAttribute("aria-valuemax", String(maximum));
    } else {
      this.#scratchpadSplitter.removeAttribute("aria-valuemax");
    }
  }

  #setScratchpadWidth(width: number, persist = true): void {
    this.#scratchpadWidth = clampScratchpadWidth(width, this.#scratchpadBodyWidth());
    this.#applyScratchpadWidth();
    if (persist) writeScratchpadWidth(localStorage, this.#scratchpadWidth);
    requestAnimationFrame(() => {
      if (!this.#disposed) this.#fitPanes(this.#activeTabPaneIds());
    });
  }

  #beginScratchpadResize(event: PointerEvent): void {
    if (!this.#scratchpadOpen) return;
    event.preventDefault();
    this.#scratchpadDragCleanup?.();
    const startX = event.clientX;
    const startWidth = this.#scratchpadWidth;
    const pointerID = event.pointerId;
    const move = (moveEvent: PointerEvent) => {
      if (moveEvent.pointerId !== pointerID) return;
      // The panel sits right of the splitter: dragging left widens it.
      this.#setScratchpadWidth(startWidth + startX - moveEvent.clientX, false);
    };
    const cleanup = () => {
      this.#scratchpadSplitter.removeEventListener("pointermove", move);
      this.#scratchpadSplitter.removeEventListener("pointerup", finish);
      this.#scratchpadSplitter.removeEventListener("pointercancel", finish);
      this.#scratchpadSplitter.removeEventListener("lostpointercapture", finish);
      if (this.#scratchpadSplitter.hasPointerCapture(pointerID)) {
        this.#scratchpadSplitter.releasePointerCapture(pointerID);
      }
      if (this.#scratchpadDragCleanup === cleanup) this.#scratchpadDragCleanup = null;
    };
    const finish = (finishEvent: PointerEvent) => {
      if (
        finishEvent.type !== "lostpointercapture" &&
        finishEvent.pointerId !== pointerID
      ) {
        return;
      }
      cleanup();
      writeScratchpadWidth(localStorage, this.#scratchpadWidth);
      this.#fitPanes(this.#activeTabPaneIds());
    };
    this.#scratchpadDragCleanup = cleanup;
    this.#scratchpadSplitter.setPointerCapture(pointerID);
    this.#scratchpadSplitter.addEventListener("pointermove", move);
    this.#scratchpadSplitter.addEventListener("pointerup", finish);
    this.#scratchpadSplitter.addEventListener("pointercancel", finish);
    this.#scratchpadSplitter.addEventListener("lostpointercapture", finish);
  }

  #resizeScratchpadFromKeyboard(event: KeyboardEvent): void {
    if (!this.#scratchpadOpen) return;
    let width = this.#scratchpadWidth;
    if (event.key === "ArrowLeft") width += 16;
    else if (event.key === "ArrowRight") width -= 16;
    else return;
    event.preventDefault();
    this.#setScratchpadWidth(width);
  }

  #setScratchpadStatus(text: string): void {
    this.#scratchpadState.textContent = text;
  }

  #flushScratchpad(): void {
    this.#scratchpadSaver.flush();
  }

  flushPending(): Promise<void> {
    if (this.#disposed) return Promise.resolve();
    return Promise.race([
      this.#scratchpadSaver.flushPending(),
      new Promise<void>((resolve) => window.setTimeout(resolve, pendingFlushLimitMs)),
    ]);
  }

  /**
   * Installs a record the saver read from the store. Returns the local text
   * that must win when the user is typing into the note right now.
   */
  #applyScratchpadRecord(record: Scratchpad, replaceLocalText: boolean): string | null {
    // A conflict reload can land after the dock is gone; there is nothing left
    // to render, and the record is still the one the final write carries.
    if (this.#disposed) return null;
    this.#renderSnippets();
    const editing = document.activeElement === this.#scratchpadText;
    if (!replaceLocalText && editing && this.#scratchpadText.value !== record.text) {
      return this.#scratchpadText.value;
    }
    this.#scratchpadText.value = record.text;
    return null;
  }

  #captureSnippet(text: string): void {
    // The scratchpad is stored in the project database: a copy that looks
    // like a credential stays on the clipboard only.
    if (looksLikeSecret(text)) {
      this.#setScratchpadStatus(secretCaptureNotice);
      return;
    }
    // A capture must not race the first read: adding to an unread record would
    // save revision 0 over the stored one and lose it to a conflict.
    if (!this.#scratchpadSaver.loaded && this.#scratchpadSaver.enabled) {
      void this.#scratchpadSaver.ensureLoaded().then(() => {
        if (this.#disposed) return;
        if (this.#scratchpadSaver.loaded) this.#applyCapturedSnippet(text);
        else this.#setScratchpadStatus(scratchpadNotices.unavailable);
      });
      return;
    }
    this.#applyCapturedSnippet(text);
  }

  #applyCapturedSnippet(text: string): void {
    const result = addSnippet(this.#scratchpadRecord().snippets, text, Date.now());
    if (!result.ok) {
      if (result.reason === "too-large") {
        this.#setScratchpadStatus(scratchpadNotices.tooLarge);
      } else if (result.reason === "all-pinned") {
        this.#setScratchpadStatus(scratchpadNotices.allPinned);
      }
      return;
    }
    this.#commitSnippets(result.snippets);
  }

  #commitSnippets(snippets: ScratchpadSnippet[]): void {
    const focus = this.#focusedSnippetAction();
    this.#scratchpadSaver.applySnippets(snippets);
    this.#renderSnippets();
    this.#restoreSnippetFocus(focus);
  }

  #addSelectionToScratchpad(): void {
    const resources = this.#activeRuntime().resources;
    if (!resources || resources.disposed) return;
    this.#captureSnippet(resources.terminal.getSelection());
  }

  #nativeClipboardAvailable(): boolean {
    try {
      nativeClipboard();
      return true;
    } catch {
      return false;
    }
  }

  #renderScratchpadSelection(): void {
    const runtime = this.#activeRuntime();
    const resources = runtime.resources;
    this.#scratchpadAdd.disabled = !resources || resources.disposed ||
      !resources.terminal.hasSelection();
    const clipboard = this.#nativeClipboardAvailable();
    const pasteReady = clipboard && runtime.state === "running" &&
      Boolean(resources) && !resources?.disposed;
    for (const button of this.#scratchpadList.querySelectorAll<HTMLButtonElement>(
      '[data-scratchpad-action="copy"]',
    )) {
      button.disabled = !clipboard;
      // The reason travels on the accessible name too, not only the tooltip.
      this.#labelScratchpadAction(
        button,
        clipboard ? "Copy snippet" : "Copy needs the native clipboard",
      );
    }
    for (const button of this.#scratchpadList.querySelectorAll<HTMLButtonElement>(
      '[data-scratchpad-action="paste"]',
    )) {
      button.disabled = !pasteReady;
      this.#labelScratchpadAction(
        button,
        !clipboard
          ? "Paste needs the native clipboard"
          : pasteReady
            ? "Paste snippet into the active pane"
            : "Paste needs a running terminal pane",
      );
    }
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
    const button = this.#scratchpadList.querySelector<HTMLButtonElement>(
      `[data-snippet-id="${focus.id}"] [data-scratchpad-action="${focus.action}"]`,
    );
    // A deleted row takes its buttons with it; the strip's own control is the
    // nearest place a keyboard user can carry on from.
    if (button && !button.disabled) button.focus();
    else if (!this.#scratchpadAdd.disabled) this.#scratchpadAdd.focus();
    else this.#scratchpadText.focus();
  }

  #renderSnippets(): void {
    const snippets = orderedSnippets(this.#scratchpadRecord().snippets);
    this.#scratchpadList.replaceChildren(
      ...snippets.map((snippet) => this.#scratchpadRow(snippet)),
    );
    this.#scratchpadEmpty.hidden = snippets.length > 0;
    this.#renderScratchpadSelection();
  }

  #scratchpadRow(snippet: ScratchpadSnippet): HTMLLIElement {
    const row = document.createElement("li");
    row.className = "terminal-scratchpad-snippet";
    row.dataset.pinned = String(snippet.pinned);
    row.dataset.snippetId = String(snippet.id);
    const preview = document.createElement("span");
    preview.className = "terminal-scratchpad-preview";
    const text = snippetPreview(snippet.text);
    preview.textContent = text;
    preview.title = text;
    const copy = this.#scratchpadAction("copy", "Copy snippet");
    copy.append(terminalControlIcon("duplicate"));
    copy.addEventListener("click", () => void this.#copySnippet(snippet.text));
    const paste = this.#scratchpadAction("paste", "Paste snippet into the active pane");
    paste.textContent = "Paste";
    paste.addEventListener("click", () => void this.#pasteSnippet(snippet.text));
    const pinLabel = snippet.pinned ? "Unpin snippet" : "Pin snippet";
    const pin = this.#scratchpadAction("pin", pinLabel);
    pin.textContent = snippet.pinned ? "Unpin" : "Pin";
    pin.addEventListener("click", () =>
      this.#commitSnippets(togglePinned(this.#scratchpadRecord().snippets, snippet.id)),
    );
    const remove = this.#scratchpadAction("delete", "Delete snippet");
    remove.append(terminalControlIcon("close"));
    remove.addEventListener("click", () =>
      this.#commitSnippets(removeSnippet(this.#scratchpadRecord().snippets, snippet.id)),
    );
    row.append(preview, copy, paste, pin, remove);
    return row;
  }

  #scratchpadAction(action: string, label: string): HTMLButtonElement {
    const button = document.createElement("button");
    button.className = "terminal-scratchpad-action";
    button.type = "button";
    button.dataset.scratchpadAction = action;
    this.#labelScratchpadAction(button, label);
    return button;
  }

  #labelScratchpadAction(button: HTMLButtonElement, label: string): void {
    button.setAttribute("aria-label", label);
    button.title = label;
  }

  async #writeClipboard(text: string): Promise<void> {
    const write = this.#clipboardWrite.then(() => nativeClipboard().setText(text));
    this.#clipboardWrite = write.catch(() => {});
    await write;
  }

  async #copySnippet(text: string): Promise<void> {
    try {
      await this.#writeClipboard(text);
    } catch (error) {
      if (!this.#disposed) this.#showError(error);
    }
  }

  /**
   * The snippet takes the clipboard's place in the existing paste path, so a
   * multi-line snippet still goes through the review dialog.
   */
  async #pasteSnippet(text: string): Promise<void> {
    const runtime = this.#activeRuntime();
    const resources = runtime.resources;
    if (!resources) return;
    await this.#pasteText(runtime, resources, () => Promise.resolve(text));
  }

  #markPersistenceDirty(): void {
    this.#persistenceScheduler.markDirty();
  }

  #flushPersistence(): void {
    this.#persistenceScheduler.flush();
  }

  #openingRuntimeCount(): number {
    return this.#runtimes.values().filter(
      (runtime) => runtime.state === "opening" && runtime.session === null,
    ).length;
  }

  #pruneEarlyExits(): void {
    const maximumCachedExits = earlyExitCacheLimit(
      this.#openingRuntimeCount(),
      maximumWorkspaceTabs * maximumPanesPerTab,
    );
    while (this.#earlyExit.size > maximumCachedExits) {
      const oldest = this.#earlyExit.keys().next().value as string | undefined;
      if (oldest === undefined) break;
      this.#earlyExit.delete(oldest);
    }
  }

  #streamStateChanged(
    runtime: DockPaneRuntime,
    ticket: PaneRuntimeTicket,
    sessionId: string,
    state: StreamState,
  ): void {
    if (!paneRuntimeEventAccepted({
      ticketAccepted: this.#accepts(runtime, ticket),
      closing: runtime.closing,
      sessionId: runtime.session?.sessionId ?? null,
      eventSessionId: sessionId,
    })) return;
    if (runtime.resources) runtime.resources.diagnosticChangedAt = Date.now();
    // Only a stream that opened earns a fresh re-claim budget.
    if (state === "open" && runtime.resources) runtime.resources.reclaimAttempts = 0;
    if (state === "connecting") {
      if (this.#isActive(runtime)) this.#renderState();
      return;
    }
    // A stream that ended without anyone asking for it is not the end of the
    // session: the PTY is still running and the lease can be claimed back.
    // One the server closed normally is: the output ended with the shell, so
    // the pane waits for the exit instead of replaying the same scrollback.
    const resources = runtime.resources;
    if (state !== "open" && resources && !resources.disposed) {
      const disposition = streamCloseDisposition({
        outputEnded: state === "closed" && resources.client?.outputEnded === true,
        sessionEnded: resources.sessionEnded,
        recoverable: streamLossIsRecoverable({
          state: runtime.state,
          closing: runtime.closing,
          hasSession: runtime.session !== null,
          hasRenderer: true,
        }),
      });
      if (disposition === "reclaim") {
        this.#scheduleStreamReclaim(runtime, ticket, resources, sessionId);
        return;
      }
      if (disposition === "ended") {
        if (runtime.state === "running" || runtime.state === "opening") {
          this.#setState(runtime, "exited", streamOutputEndedNotice);
        }
        return;
      }
    }
    const transition = paneRuntimeTransition(runtime.state, {
      kind: state === "open"
        ? "stream-open"
        : state === "error"
          ? "stream-error"
          : "stream-closed",
    });
    if (transition) this.#setState(runtime, transition.state, transition.detail);
  }

  #fit(
    runtime: DockPaneRuntime,
    resources: PaneResources,
    notifyBackend: boolean,
  ): void {
    if (
      resources.disposed ||
      resources.host.hidden ||
      this.#body.hidden ||
      resources.host.clientWidth === 0 ||
      resources.host.clientHeight === 0
    ) return;
    const buffer = resources.terminal.buffer.active;
    const wasAtBottom = buffer.viewportY === buffer.baseY;
    const viewportLine = buffer.viewportY;
    try {
      resources.fit.fit();
      if (!wasAtBottom) {
        resources.terminal.scrollToLine(Math.min(viewportLine, resources.terminal.buffer.active.baseY));
      }
      if (notifyBackend && runtime.session) {
        this.#scheduleBackendResize(runtime, resources);
      }
    } catch {
      // A later observer callback retries after layout becomes measurable.
    }
  }

  #fitPanes(paneIdList: readonly string[], notifyBackend = true): void {
    for (const paneId of paneIdList) {
      const runtime = this.#runtimes.get(paneId);
      const resources = runtime?.resources;
      if (runtime && resources) this.#fit(runtime, resources, notifyBackend);
    }
  }

  #scheduleBackendResize(
    _runtime: DockPaneRuntime,
    resources: PaneResources,
  ): void {
    resources.resizeDispatcher?.queue({
      rows: resources.terminal.rows,
      columns: resources.terminal.cols,
    });
  }

  #disposeResources(resources: PaneResources): void {
    if (resources.disposed) return;
    resources.disposed = true;
    resources.client?.close();
    resources.observer?.disconnect();
    if (resources.animationFrame !== null) cancelAnimationFrame(resources.animationFrame);
    resources.resizeDispatcher?.dispose();
    resources.resizeDispatcher = null;
    if (resources.webglRecoveryTimer !== null) {
      window.clearTimeout(resources.webglRecoveryTimer);
    }
    resources.webglContextLoss?.dispose();
    resources.webglContextLoss = null;
    for (const dispose of resources.eventDisposers.splice(0)) dispose();
    for (const subscription of resources.subscriptions.splice(0)) subscription.dispose();
    resources.terminal.dispose();
    resources.host.remove();
  }

  #teardownRuntime(runtime: DockPaneRuntime): void {
    const busy = runtime.busy;
    if (this.#isActive(runtime)) {
      this.#dragCleanup?.();
      this.#invalidatePaste();
      this.#hideContextMenu();
      this.#searchForm.hidden = true;
      this.#searchResults.textContent = "";
    }
    this.#lifecycle.releaseLocal(runtime.paneId);
    runtime.busy = busy;
    if (this.#isActive(runtime)) this.#renderZoomState();
  }

  #setState(runtime: DockPaneRuntime, state: DockState, detail = ""): void {
    if (this.#isActive(runtime) && state !== "running") this.#invalidatePaste();
    if (this.#isActive(runtime) && state === "closed") this.#setBoardHidden(false);
    runtime.state = state;
    runtime.detail = detail;
    if (runtime.resources) runtime.resources.diagnosticChangedAt = Date.now();
    if (state === "closed") runtime.title = "";
    if (state !== "opening") this.#pruneEarlyExits();
    if (this.#isActive(runtime)) this.#renderState();
    else this.#tabBar.refresh();
  }

  #diagnosticInput(
    runtime: DockPaneRuntime,
    resources: PaneResources | null,
  ): TerminalDiagnosticInput {
    const process: TerminalDiagnosticProcess = runtime.closing
      ? "stopping"
      : ({
        closed: "stopped",
        opening: "starting",
        running: "running",
        exited: "exited",
        failed: "failed",
      } satisfies Record<DockState, TerminalDiagnosticProcess>)[runtime.state];
    const clientState = resources?.client?.state;
    const stream: TerminalDiagnosticStream = !runtime.session || !clientState
      ? "idle"
      : ({
        closed: "disconnected",
        connecting: "connecting",
        open: "connected",
        error: "failed",
      } satisfies Record<StreamState, TerminalDiagnosticStream>)[clientState];
    const visible = Boolean(
      resources &&
      !resources.disposed &&
      !resources.host.hidden &&
      this.#isPaneVisible(runtime.paneId) &&
      document.visibilityState === "visible",
    );
    let renderer: TerminalDiagnosticRenderer = "none";
    if (resources) {
      renderer = resources.webgl
        ? "webgl"
        : resources.webglRecoveryTimer !== null
          ? "recovering"
          : resources.webglRecoveryAttempts >= maximumWebglRecoveryAttempts &&
              this.#shouldUseWebgl(runtime.paneId) && visible
            ? "fallback"
            : "dom";
    }
    const activeAssociation = this.#tabController.workspace.tabs.find(
      (tab) => tab.id === this.#tabController.workspace.activeTabId,
    )?.association;
    return {
      stream,
      renderer,
      process,
      layout: this.#layoutDiagnosticState,
      rendererAttempts: resources?.webglRecoveryAttempts ?? 0,
      layoutRepairs: this.#layoutRepairCount,
      changedAt: Math.max(
        this.#layoutDiagnosticChangedAt,
        resources?.diagnosticChangedAt ?? 0,
      ),
      hasSession: runtime.session !== null,
      linked: this.#paneIsLinked(runtime, activeAssociation),
      busy: runtime.busy || runtime.closing,
      selected: this.#isActive(runtime),
      visible,
    };
  }

  #renderDiagnostics(
    view: ReturnType<typeof terminalDiagnosticView>,
  ): void {
    const values = new Map(view.rows.map((row) => [row.key, row.value]));
    this.#diagnosticProcess.textContent = values.get("process") ?? "Stopped";
    this.#diagnosticStream.textContent = values.get("stream") ?? "Idle";
    this.#diagnosticRenderer.textContent = values.get("renderer") ?? "Not created";
    this.#diagnosticLayout.textContent = values.get("layout") ?? "Default";
    this.#diagnosticUpdated.textContent = values.get("updated") ?? "Not recorded";
  }

  #setDiagnosticsOpen(open: boolean, restoreFocus = false): void {
    this.#diagnosticsOpen = open;
    this.#diagnostics.hidden = !open;
    this.#diagnosticsToggle.setAttribute("aria-expanded", String(open));
    const label = open ? "Hide terminal diagnostics" : "Show terminal diagnostics";
    this.#diagnosticsToggle.setAttribute("aria-label", label);
    this.#diagnosticsToggle.title = label;
    this.#diagnosticsObserver?.disconnect();
    this.#diagnosticsObserver = null;
    if (open) {
      const runtime = this.#activeRuntime();
      this.#renderDiagnostics(
        terminalDiagnosticView(this.#diagnosticInput(runtime, runtime.resources)),
      );
      this.#placeDiagnostics();
      if ("ResizeObserver" in window) {
        this.#diagnosticsObserver = new ResizeObserver(() => this.#placeDiagnostics());
        this.#diagnosticsObserver.observe(this.#toolbar);
        this.#diagnosticsObserver.observe(this.#dock);
      }
      this.#diagnostics.focus();
    } else if (restoreFocus) {
      this.#diagnosticsToggle.focus();
    }
  }

  /**
   * The popover hangs just below the dock's header rows, measured rather than
   * assumed: at a fixed offset it sat over the toolbar's wrapped second row and
   * the very toggle that dismisses it.
   */
  #placeDiagnostics(): void {
    const dockTop = this.#dock.getBoundingClientRect().top;
    const headerBottom = this.#toolbar.getBoundingClientRect().bottom - dockTop;
    const top = terminalDiagnosticsTop({
      headerBottom,
      dockHeight: this.#dock.clientHeight,
    });
    this.#diagnostics.style.setProperty("--terminal-diagnostics-top", `${top}px`);
  }

  #retryActiveRenderer(): void {
    const runtime = this.#activeRuntime();
    const resources = runtime.resources;
    if (!resources || resources.disposed) return;
    const view = terminalDiagnosticView(this.#diagnosticInput(runtime, resources));
    retryTerminalRendererRecovery({
      allowed: view.canRetryRenderer,
      capture: () => this.#runtimes.capture(runtime.paneId),
      accepted: (ticket) => this.#accepts(runtime, ticket),
      reset: () => {
        resources.webglRecoveryAttempts = 0;
        resources.webglRecoveryPaused = false;
        resources.diagnosticChangedAt = Date.now();
      },
      refresh: () => resources.terminal.refresh(0, resources.terminal.rows - 1),
      attach: (ticket) => this.#attachWebgl(runtime, resources, ticket, "policy"),
      fit: () => this.#fit(runtime, resources, false),
      render: () => {
        if (this.#isActive(runtime)) this.#renderState();
      },
    });
  }

  #placeEmptyNotice(stopped: boolean, paneId: string): void {
    this.#empty.hidden = !stopped;
    if (!stopped) return;
    const mount = this.#body.hidden
      ? this.#emptyHome
      : this.#splitView.mountForPane(paneId) ?? this.#host;
    if (mount && this.#empty.parentElement !== mount) mount.append(this.#empty);
  }

  #renderState(): void {
    this.#syncTerminalInputLabels();
    const runtime = this.#activeRuntime();
    const resources = runtime.resources;
    const diagnosticInput = this.#diagnosticInput(runtime, resources);
    const diagnosticView = terminalDiagnosticView(diagnosticInput);
    const activeTab = this.#tabController.workspace.tabs.find(
      (tab) => tab.id === this.#tabController.workspace.activeTabId,
    );
    const dockInteractionEligible = this.#dockInteractionEligible();
    if (runtime.state === "closed" && !dockInteractionEligible) {
      this.#boardHidden = false;
    }
    this.#dock.dataset.state = runtime.state;
    this.#dock.dataset.layoutInteractive = String(dockInteractionEligible);
    // The pane holds the popped-out session's place, so it must not be reused
    // for a new terminal: that would leave the returning session nowhere to go.
    const poppedOut = this.#paneIsPoppedOut(runtime.paneId);
    // An empty pane that is waiting for a window keeps its body, so the notice
    // saying where its terminal went is actually visible. The scratchpad also
    // keeps the body up while it is open, even with no session, so its note
    // and clipboard strip stay reachable before any terminal starts.
    this.#body.hidden = !terminalBodyVisible({
      state: runtime.state,
      poppedOut,
      singlePane: !activeTab || paneIds(activeTab.root).length === 1,
      scratchpadOpen: this.#scratchpadOpen,
    });
    // Whenever the dock is expanded around a stopped pane, it says so and offers
    // Start shell; the collapsed bar keeps only its compact Open control.
    this.#placeEmptyNotice(
      runtime.state === "closed" && !poppedOut && (dockInteractionEligible || this.#scratchpadOpen),
      runtime.paneId,
    );
    this.#message.textContent = runtime.detail;
    this.#message.hidden = runtime.detail === "";
    const shellLabel = runtime.state === "running" && resources
      ? shellStatusLabel(resources.shellState)
      : null;
    this.#status.textContent = runtime.closing
      ? "Closing…"
      : poppedOut
        ? "In its own window"
      : runtime.activity.signal === "failed"
        ? "Failed"
        : runtime.activity.signal === "completed"
          ? "Completed"
        : {
        closed: "Closed",
        opening: "Opening…",
        running: shellLabel ?? "Running",
        exited: "Exited",
        failed: "Failed",
      }[runtime.state];
    this.#open.hidden = runtime.state !== "closed";
    this.#restart.hidden = runtime.state !== "exited" && runtime.state !== "failed";
    // Tab-row start affordance mirrors the session state in its label.
    const startLabel = runtime.state === "closed"
      ? "Start terminal"
      : runtime.state === "exited" || runtime.state === "failed"
      ? "Restart terminal"
      : null;
    this.#start.hidden = startLabel === null;
    if (startLabel) {
      this.#start.setAttribute("aria-label", startLabel);
      this.#start.title = startLabel;
    }
    this.#close.hidden =
      runtime.state === "closed" ||
      (runtime.state === "failed" && runtime.session === null);
    this.#rendererRetry.hidden = diagnosticInput.renderer !== "fallback";
    this.#forceStop.hidden = !diagnosticInput.hasSession ||
      !["starting", "running", "failed"].includes(diagnosticInput.process);
    this.#boardToggle.disabled = this.#layoutLocked || !dockInteractionEligible;
    const terminalActionsDisabled = !resources;
    this.#searchOpen.disabled = terminalActionsDisabled;
    this.#zoomReset.disabled = terminalActionsDisabled;
    this.#clear.disabled = terminalActionsDisabled;
    const workspace = this.#tabController.workspace;
    const descriptor = activeTerminalDescriptor(workspace);
    const activeAssociation = workspace.tabs.find(
      (tab) => tab.id === workspace.activeTabId,
    )?.association;
    const linked = this.#paneIsLinked(runtime, activeAssociation);
    const popOut = terminalPopOutControl({
      panes: this.#activeTabPaneIds().map((paneId) => {
        const paneRuntime = this.#runtimes.get(paneId);
        return {
          state: paneRuntime?.state ?? "closed",
          hasSession: Boolean(paneRuntime?.session),
        };
      }),
      busy: runtime.busy,
      closing: runtime.closing,
    });
    this.#popOut.hidden = !popOut.present;
    this.#popOut.disabled = popOut.disabled;
    this.#open.disabled = runtime.busy || runtime.closing || linked || poppedOut ||
      !descriptor?.pane.profileId;
    this.#startShell.disabled = this.#open.disabled || this.#open.hidden;
    this.#restart.disabled = !diagnosticView.canRestart;
    this.#start.disabled = runtime.state === "closed"
      ? this.#open.disabled
      : this.#restart.disabled;
    this.#close.disabled = runtime.busy || runtime.closing;
    this.#rendererRetry.disabled = !diagnosticView.canRetryRenderer;
    this.#forceStop.disabled = !diagnosticView.canForceStop;
    const associationState = this.associationState();
    this.#association.disabled = associationState === null;
    const associationLabel = associationState?.pointer
      ? "Relink terminal context"
      : associationState && associationState.revision > 0
        ? "Relink detached terminal context"
        : "Link terminal context";
    this.#association.setAttribute("aria-label", associationLabel);
    this.#association.title = associationLabel;
    this.#writeback.disabled = associationState?.pointer === undefined;
    this.#resetWorkspace.title = diagnosticView.canResetLayout
      ? "Reset repaired terminal workspace for this project"
      : "Reset terminal workspace for this project";
    const descriptorEditable = runtimeDescriptorEditable(runtime);
    this.#profile.disabled = !descriptorEditable || linked;
    this.#cwd.disabled = !descriptorEditable || linked;
    this.#syncDescriptorEditor();
    this.#renderZoomState();
    this.#title.textContent = runtime.state === "closed"
      ? "Stopped"
      : runtime.title || this.#selectedProfileName();
    this.#renderDiagnostics(diagnosticView);
    this.#renderScratchpadSelection();
    this.#renderPanelVisibility();
    this.#tabBar.refresh();
  }

  #syncTerminalInputLabels(): void {
    for (const tab of this.#tabController.workspace.tabs) {
      paneIds(tab.root).forEach((paneId, index) => {
        this.#runtimes.get(paneId)?.resources?.terminal.textarea?.setAttribute(
          "aria-label",
          terminalPaneInputLabel(tab.title, index + 1),
        );
      });
    }
  }

  #activeRuntime(): DockPaneRuntime {
    const active = activeTerminalDescriptor(this.#tabController.workspace);
    if (!active) throw new Error("Workspace has no active terminal descriptor");
    return this.#runtimes.ensure(active.pane.paneId);
  }

  #descriptorFor(paneId: string) {
    for (const tab of this.#tabController.workspace.tabs) {
      const pane = findTerminalPane(tab.root, paneId);
      if (pane) {
        return { tabId: tab.id, pane, association: tab.association };
      }
    }
    return null;
  }

  #paneIsLinked(
    runtime: DockPaneRuntime,
    pointer: AssociationPointerV1 | undefined,
  ): boolean {
    return terminalHasLinkedOrigin(
      pointer,
      runtime.session?.linkedLaunch === true,
      this.#linkedLaunchPaneIds.has(runtime.paneId),
    );
  }

  #paneIsPoppedOut(paneId: string): boolean {
    return panesHoldPoppedOutTerminal([paneId], this.#poppedOut.values());
  }

  /**
   * Refuse a close that would remove a pane holding a popped-out terminal. The
   * pane has no session, so the close intent never asks about it, and the
   * window that has the shell would find its place gone and close the session
   * instead. Both structural closes and the workspace reset come through here.
   */
  #poppedOutCloseRefused(closingPaneIds: readonly string[]): boolean {
    if (!panesHoldPoppedOutTerminal(closingPaneIds, this.#poppedOut.values())) {
      return false;
    }
    this.#showError(new Error(poppedOutCloseRefusedNotice));
    return true;
  }

  #isActive(runtime: DockPaneRuntime): boolean {
    return activeTerminalDescriptor(this.#tabController.workspace)?.pane.paneId ===
      runtime.paneId;
  }

  #activeTabPaneIds(): string[] {
    const workspace = this.#tabController.workspace;
    const tab = workspace.tabs.find((candidate) => candidate.id === workspace.activeTabId);
    return tab ? paneIds(tab.root) : [];
  }

  #dockInteractionEligible(): boolean {
    const activePaneIds = this.#activeTabPaneIds();
    return activeTabDockInteractionEligible({
      paneCount: activePaneIds.length,
      hasResources: activePaneIds.some((paneId) => Boolean(this.#runtimes.get(paneId)?.resources)),
      hasLiveRuntime: activePaneIds.some((paneId) => {
        const runtime = this.#runtimes.get(paneId);
        return Boolean(
          runtime?.session ||
          runtime?.state === "opening" ||
          runtime?.state === "running",
        );
      }),
    });
  }

  #isPaneVisible(paneId: string): boolean {
    return terminalPanePresentationPolicy({
      workspaceViewVisible: this.#workspaceViewVisible,
      applicationOverlayOpen: this.#applicationOverlayOpen,
      terminalHidden: this.#terminalHidden,
      documentVisible: document.visibilityState === "visible",
      activeTab: this.#activeTabPaneIds().includes(paneId),
      selected: false,
      hasResources: false,
      hostVisible: false,
      bodyVisible: !this.#body.hidden,
      dockVisible: !this.#dock.hidden,
    }).paneVisible;
  }

  #preferredWebglPaneIds(): string[] {
    const workspace = this.#tabController.workspace;
    const tab = workspace.tabs.find((candidate) => candidate.id === workspace.activeTabId);
    const policy = terminalPanePresentationPolicy({
      workspaceViewVisible: this.#workspaceViewVisible,
      applicationOverlayOpen: this.#applicationOverlayOpen,
      terminalHidden: this.#terminalHidden,
      documentVisible: document.visibilityState === "visible",
      activeTab: Boolean(tab),
      selected: false,
      hasResources: false,
      hostVisible: false,
      bodyVisible: !this.#body.hidden,
      dockVisible: !this.#dock.hidden,
    });
    if (!tab || !policy.webglAllowed) return [];
    return preferredWebglPaneIds(
      tab.root,
      tab.activePaneId,
      new Set(paneIds(tab.root)),
      4,
    );
  }

  #shouldUseWebgl(paneId: string): boolean {
    // "canvas" has no installed addon, so both it and "dom" mean the
    // unaccelerated renderer.
    if (
      !webglPreferredByPreference(
        readTerminalPreferenceOverrides(localStorage).renderer,
      )
    ) return false;
    return this.#preferredWebglPaneIds().includes(paneId);
  }

  #updateWebglPolicy(): void {
    const preferred = new Set(this.#preferredWebglPaneIds());
    for (const runtime of this.#runtimes.values()) {
      const resources = runtime.resources;
      if (!resources || resources.disposed) continue;
      if (!preferred.has(runtime.paneId)) {
        const recovery = webglRecoveryAfterSuppression({
          attempts: resources.webglRecoveryAttempts,
          timerPending: resources.webglRecoveryTimer !== null,
          paused: resources.webglRecoveryPaused,
        }, this.#applicationOverlayOpen);
        if (resources.webglRecoveryTimer !== null) {
          window.clearTimeout(resources.webglRecoveryTimer);
          resources.webglRecoveryTimer = null;
        }
        resources.webglContextLoss?.dispose();
        resources.webglContextLoss = null;
        const webgl = resources.webgl;
        resources.webgl = null;
        webgl?.dispose();
        resources.webglRecoveryAttempts = recovery.attempts;
        resources.webglRecoveryPaused = recovery.paused;
        continue;
      }
      const ticket = this.#runtimes.capture(runtime.paneId);
      if (!ticket) continue;
      const recoveryAction = webglRecoveryPolicyAction({
        attempts: resources.webglRecoveryAttempts,
        timerPending: resources.webglRecoveryTimer !== null,
        paused: resources.webglRecoveryPaused,
      });
      if (recoveryAction === "attach") {
        this.#attachWebgl(runtime, resources, ticket);
      } else if (recoveryAction === "schedule") {
        this.#scheduleWebglRecovery(runtime, resources, ticket);
      }
    }
  }

  #focusPaneInDirection(runtime: DockPaneRuntime, direction: PaneDirection): void {
    const workspace = this.#tabController.workspace;
    const tab = workspace.tabs.find((candidate) => candidate.id === workspace.activeTabId);
    if (!tab || tab.activePaneId !== runtime.paneId) return;
    const bounds = this.#host.getBoundingClientRect();
    const target = paneInDirection(
      leafRects(tab.root, {
        x: 0,
        y: 0,
        width: Math.max(1, bounds.width),
        height: Math.max(1, bounds.height),
      }),
      runtime.paneId,
      direction,
    );
    if (!target) return;
    this.#tabController.dispatch({ type: "focus-pane", tabId: tab.id, paneId: target });
  }

  #isGenuinelyForeground(runtime: DockPaneRuntime): boolean {
    return terminalPanePresentationPolicy({
      workspaceViewVisible: this.#workspaceViewVisible,
      applicationOverlayOpen: this.#applicationOverlayOpen,
      terminalHidden: this.#terminalHidden,
      documentVisible: document.visibilityState === "visible",
      activeTab: this.#activeTabPaneIds().includes(runtime.paneId),
      selected: this.#isActive(runtime),
      hasResources: Boolean(runtime.resources),
      hostVisible: Boolean(runtime.resources && !runtime.resources.host.hidden),
      bodyVisible: !this.#body.hidden,
      dockVisible: !this.#dock.hidden,
    }).foreground;
  }

  #acknowledgeIfForeground(runtime: DockPaneRuntime): void {
    if (!this.#isGenuinelyForeground(runtime)) return;
    const acknowledged = acknowledgePaneActivity(runtime.activity);
    if (acknowledged === runtime.activity) return;
    runtime.activity = acknowledged;
    this.#tabBar.refresh();
  }

  #accepts(runtime: DockPaneRuntime, ticket: PaneRuntimeTicket): boolean {
    return !this.#disposed &&
      this.#runtimes.get(runtime.paneId) === runtime &&
      this.#runtimes.accepts(ticket);
  }

  #defaultTabIntent(action: WorkspaceAction): WorkspaceAction | null {
    if (
      action.type === "split-pane" &&
      this.#linkedLaunchPaneIds.has(action.paneId)
    ) return null;
    if (action.type !== "create-tab") return action;
    return {
      ...action,
      profileId: action.profileId ?? this.#defaultProfileId,
      cwd: action.cwd ?? "",
    };
  }

  async #handleStructuralClose(
    action: Extract<WorkspaceAction, { type: "close-tab" | "close-pane" }>,
  ): Promise<void> {
    const workspace = this.#tabController.workspace;
    let closingPaneIds: string[] = [];
    if (action.type === "close-tab") {
      const tab = workspace.tabs.find((candidate) => candidate.id === action.tabId);
      if (tab) closingPaneIds = paneIds(tab.root);
    } else {
      closingPaneIds = [action.paneId];
    }
    if (closingPaneIds.length === 0) return;
    if (this.#poppedOutCloseRefused(closingPaneIds)) return;
    try {
      const result = await runDescriptorCloseIntent({
        paneIds: closingPaneIds,
        registry: this.#runtimes,
        lifecycle: this.#lifecycle,
        confirm: () => this.#confirmTermination(closingPaneIds.length),
        commit: () => {
          for (const paneId of closingPaneIds) {
            this.#authorizedRuntimeRemoval.add(paneId);
          }
          const changed = this.#tabController.dispatch(action);
          if (!changed) {
            for (const paneId of closingPaneIds) {
              this.#authorizedRuntimeRemoval.delete(paneId);
            }
          }
        },
      });
      this.#pruneEarlyExits();
      if (result === "closed") {
        if (structuralCloseFocusTarget(action.type) === "active-tab") {
          this.#tabBar.focusActiveTab();
        } else {
          this.#focusWorkspaceSurvivor();
        }
      }
    } catch (error) {
      this.#pruneEarlyExits();
      if (!this.#disposed) {
        this.#renderState();
        this.#showError(error);
      }
    }
  }

  #focusWorkspaceSurvivor(): void {
    const active = activeTerminalDescriptor(this.#tabController.workspace);
    if (!active) return;
    const resources = this.#runtimes.get(active.pane.paneId)?.resources;
    if (resources && !resources.disposed && this.#isPaneVisible(active.pane.paneId)) {
      resources.terminal.focus();
      return;
    }
    if (!this.#body.hidden && this.#splitView.focusPaneSelector(active.pane.paneId)) return;
    this.#tabBar.focusActiveTab();
  }

  #resetTerminalWorkspace(): Promise<void> {
    if (this.#resetPromise) return this.#resetPromise;
    this.#resetWorkspace.disabled = true;
    const operation = this.#performTerminalWorkspaceReset().finally(() => {
      this.#resetPromise = null;
      if (!this.#disposed) this.#resetWorkspace.disabled = false;
    });
    this.#resetPromise = operation;
    return operation;
  }

  async #performTerminalWorkspaceReset(): Promise<void> {
    const workspaceAtStart = this.#tabController.workspace;
    const paneIdList = workspaceAtStart.tabs.flatMap((tab) =>
      paneIds(tab.root)
    );
    if (this.#poppedOutCloseRefused(paneIdList)) return;
    const runtimes = paneIdList.map((paneId) => this.#runtimes.ensure(paneId));
    try {
      const result = await resetTerminalWorkspaceRecovery({
        confirm: () => closeIntentConfirmed(
          runtimes,
          () => this.#confirmTermination(paneIdList.length),
        ),
        close: () => this.#lifecycle.closeMany(paneIdList),
        accepted: () =>
          !this.#disposed && this.#tabController.workspace === workspaceAtStart,
        replace: () => {
          for (const paneId of paneIdList) this.#authorizedRuntimeRemoval.add(paneId);
          const replacement = createWorkspace(this.#ids, {
            profileId: this.#profile.value || this.#defaultProfileId,
            cwd: "",
          });
          const replaced = this.#tabController.replace(replacement);
          if (!replaced) {
            for (const paneId of paneIdList) {
              this.#authorizedRuntimeRemoval.delete(paneId);
            }
          }
          return replaced;
        },
        clear: (replaced) => {
          clearTerminalWorkspaceAfterReplace(localStorage, this.#projectRoot, replaced);
        },
      });
      if (result !== "reset") return;
      this.#layoutDiagnosticState = "default";
      this.#layoutRepairCount = 0;
      this.#layoutDiagnosticChangedAt = Date.now();
      this.#dockRatio = defaultDockRatio;
      this.#setDockHeight(this.#heightForDockRatio(defaultDockRatio), false);
      this.#markPersistenceDirty();
      this.#flushPersistence();
    } catch (error) {
      if (!this.#disposed) this.#showError(error);
    }
  }

  #reconcileWorkspace(workspace: Workspace, previous: Workspace): void {
    const previousActive = activeTerminalDescriptor(previous)?.pane.paneId;
    const active = activeTerminalDescriptor(workspace);
    if (!active) return;
    if (active.pane.profileId === "" && this.#defaultProfileId !== "") {
      this.#tabController.dispatch({
        type: "update-pane",
        tabId: active.tabId,
        paneId: active.pane.paneId,
        changes: { profileId: this.#defaultProfileId },
      });
      return;
    }
    const livePaneIds = new Set(
      workspace.tabs.flatMap((tab) => paneIds(tab.root)),
    );
    for (const runtime of this.#runtimes.values()) {
      if (!livePaneIds.has(runtime.paneId)) {
        if (this.#authorizedRuntimeRemoval.has(runtime.paneId)) {
          this.#lifecycle.releaseLocal(runtime.paneId);
          this.#runtimes.remove(runtime.paneId);
          this.#linkedLaunchPaneIds.delete(runtime.paneId);
          this.#authorizedRuntimeRemoval.delete(runtime.paneId);
        } else {
          this.#showError(
            new Error("Terminal descriptor was removed before runtime cleanup completed"),
          );
        }
      }
    }
    for (const paneId of livePaneIds) this.#runtimes.ensure(paneId);
    if (previousActive !== active.pane.paneId) {
      const previousRuntime = previousActive
        ? this.#runtimes.get(previousActive)
        : null;
      previousRuntime?.resources?.search.clearDecorations();
      this.#invalidatePaste();
      this.#hideContextMenu();
      this.#searchForm.hidden = true;
      this.#searchResults.textContent = "";
    }
    this.#splitView.refresh(workspace);
    const visiblePaneIds = new Set(this.#activeTabPaneIds());
    for (const runtime of this.#runtimes.values()) {
      if (runtime.resources) runtime.resources.host.hidden = !visiblePaneIds.has(runtime.paneId);
    }
    this.#updateWebglPolicy();
    requestAnimationFrame(() => {
      if (!this.#disposed) this.#fitPanes([...visiblePaneIds]);
    });
    this.#renderState();
  }

  #updateEditableDescriptor(changes: TerminalDescriptor): void {
    const active = activeTerminalDescriptor(this.#tabController.workspace);
    if (!active) return;
    const runtime = this.#runtimes.ensure(active.pane.paneId);
    if (!runtimeDescriptorEditable(runtime)) {
      this.#syncDescriptorEditor();
      return;
    }
    this.#tabController.dispatch({
      type: "update-pane",
      tabId: active.tabId,
      paneId: active.pane.paneId,
      changes,
    });
  }

  #syncDescriptorEditor(): void {
    const active = activeTerminalDescriptor(this.#tabController.workspace);
    if (!active) return;
    this.#profile.value = active.pane.profileId;
    this.#cwd.value = active.pane.cwd;
    this.#syncActiveProfileFontSize();
  }

  #syncActiveProfileFontSize(): void {
    const profileId = activeTerminalDescriptor(this.#tabController.workspace)?.pane.profileId ||
      this.#profile.value || this.#defaultProfileId;
    const settings = this.#profileSettings.get(profileId);
    this.#fontSize = this.#profileFontSizes.get(profileId) ??
      settings?.fontSize ?? defaultTerminalFontSize;
  }

  #activeProfileDefaultFontSize(): number {
    const profileId = activeTerminalDescriptor(this.#tabController.workspace)?.pane.profileId ||
      this.#profile.value || this.#defaultProfileId;
    return this.#profileSettings.get(profileId)?.fontSize ?? defaultTerminalFontSize;
  }

  #selectedProfileName(): string {
    return this.#profile.selectedOptions[0]?.textContent || "Terminal";
  }

  #setBoardHidden(hidden: boolean): void {
    this.#boardHidden = hidden && this.#dockInteractionEligible();
    if (this.#boardHidden) this.#terminalHidden = false;
    this.#renderPanelVisibility();
  }

  #setTerminalHidden(hidden: boolean): void {
    this.#terminalHidden = hidden;
    if (hidden) this.#flushScratchpad();
    if (this.#terminalHidden) this.#boardHidden = false;
    this.#renderPanelVisibility();
  }

  #renderPanelVisibility(focusTerminal = true): void {
    const revision = ++this.#panelVisibilityRevision;
    this.#workArea.dataset.boardHidden = String(this.#boardHidden);
    this.#workArea.dataset.terminalHidden = String(this.#terminalHidden);
    this.#boardToggle.setAttribute("aria-pressed", String(this.#boardHidden));
    this.#terminalToggle.setAttribute("aria-pressed", String(this.#terminalHidden));
    const boardLabel = this.#boardHidden ? "Show board panel" : "Hide board panel";
    const terminalLabel = this.#terminalHidden
      ? "Show terminal panel"
      : "Hide terminal panel";
    this.#boardToggle.setAttribute("aria-label", boardLabel);
    this.#boardToggle.title = boardLabel;
    this.#terminalToggle.setAttribute("aria-label", terminalLabel);
    this.#terminalToggle.title = terminalLabel;
    this.#separator.tabIndex = this.#boardHidden || this.#terminalHidden ? -1 : 0;
    if (this.#boardHidden || this.#terminalHidden) this.#dragCleanup?.();
    for (const paneRuntime of this.#runtimes.values()) {
      if (paneRuntime.resources) {
        paneRuntime.resources.host.hidden = !this.#isPaneVisible(paneRuntime.paneId);
      }
    }
    this.#updateWebglPolicy();
    const runtime = this.#activeRuntime();
    const resources = runtime.resources;
    if (
      this.#terminalHidden || !this.#workspaceViewVisible || this.#applicationOverlayOpen
    ) return;
    this.#acknowledgeIfForeground(runtime);
    requestAnimationFrame(() => {
      if (
        this.#disposed || this.#terminalHidden || !this.#workspaceViewVisible ||
        this.#applicationOverlayOpen || revision !== this.#panelVisibilityRevision
      ) return;
      this.#fitPanes(this.#activeTabPaneIds());
      this.#updateWebglPolicy();
      if (
        focusTerminal && resources && !resources.disposed && this.#isActive(runtime)
      ) {
        resources.terminal.focus();
      }
    });
  }

  // A pane's palette is its profile's, mapped through the app theme.
  #applyAppTheme(): void {
    if (this.#disposed) return;
    const appTheme = document.documentElement.dataset.theme;
    for (const runtime of this.#runtimes.values()) {
      const resources = runtime.resources;
      if (!resources || resources.disposed) continue;
      const profileId = this.#descriptorFor(runtime.paneId)?.pane.profileId ||
        this.#defaultProfileId;
      const profileTheme = this.#profileSettings.get(profileId)?.theme ?? "default";
      resources.terminal.options.theme = terminalProfileTheme(
        terminalThemeName(profileTheme, appTheme),
      );
    }
  }

  #setModernUnicode(enabled: boolean): void {
    try {
      for (const runtime of this.#runtimes.values()) {
        const resources = runtime.resources;
        if (resources && enabled && !resources.unicode) {
          const unicode = new UnicodeGraphemesAddon();
          resources.terminal.loadAddon(unicode);
          resources.unicode = unicode;
        } else if (resources && !enabled && resources.unicode) {
          resources.unicode.dispose();
          resources.unicode = null;
        }
      }
    } catch (error) {
      this.#showError(error);
      return;
    }

    this.#modernUnicodeEnabled = enabled;
    for (const runtime of this.#runtimes.values()) {
      const resources = runtime.resources;
      if (resources && !resources.disposed) {
        resources.terminal.refresh(0, resources.terminal.rows - 1);
        if (this.#isActive(runtime)) resources.terminal.focus();
      }
    }
  }

  #maximumDockHeight(): number {
    return Math.max(minimumDockHeight, Math.floor(this.#workAreaHeight() * 0.75));
  }

  #workAreaHeight(): number {
    return Math.max(
      1,
      this.#dock.parentElement?.clientHeight || window.innerHeight || 800,
    );
  }

  #heightForDockRatio(ratio: number): number {
    return Math.round(this.#workAreaHeight() * normalizeDockRatio(ratio));
  }

  #setDockHeight(height: number, persist = true): void {
    this.#dockHeight = Math.max(minimumDockHeight, Math.min(height, this.#maximumDockHeight()));
    this.#dockRatio = normalizeDockRatio(this.#dockHeight / this.#workAreaHeight());
    this.#dock.style.setProperty("--terminal-dock-height", `${this.#dockHeight}px`);
    this.#separator.setAttribute("aria-valuemax", String(this.#maximumDockHeight()));
    this.#separator.setAttribute("aria-valuenow", String(Math.round(this.#dockHeight)));
    requestAnimationFrame(() => {
      if (!this.#disposed) this.#fitPanes(this.#activeTabPaneIds());
    });
    if (persist) this.#markPersistenceDirty();
  }

  #beginDockResize(event: PointerEvent): void {
    if (!this.#dockInteractionEligible()) return;
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
      const runtime = this.#activeRuntime();
      if (runtime.resources) this.#fit(runtime, runtime.resources, true);
      this.#flushPersistence();
    };
    this.#dragCleanup = cleanup;
    this.#separator.setPointerCapture(pointerID);
    this.#separator.addEventListener("pointermove", move);
    this.#separator.addEventListener("pointerup", finish);
    this.#separator.addEventListener("pointercancel", finish);
    this.#separator.addEventListener("lostpointercapture", finish);
  }

  #resizeDockFromKeyboard(event: KeyboardEvent): void {
    if (!this.#dockInteractionEligible()) return;
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
    this.#flushPersistence();
  }

  #listen(
    target: EventTarget,
    type: string,
    listener: (event: Event) => void,
    options?: AddEventListenerOptions | boolean,
  ): void {
    const eventListener = listener as EventListener;
    target.addEventListener(type, eventListener, options);
    this.#dockDisposers.push(() =>
      target.removeEventListener(type, eventListener, options),
    );
  }

  agentProfiles(): InstalledAgentProfile[] {
    return installedAgentProfiles(this.#profiles);
  }

  async launchLinked(request: LinkedLaunchRequest): Promise<void> {
    if (this.#disposed) throw new Error("Terminal workspace is unavailable");
    await this.#pendingSessionCloses.retryPending();
    if (this.#disposed) throw new Error("Terminal workspace is unavailable");
    const profile = selectedInstalledAgentProfile(
      this.agentProfiles(),
      request.profileId,
    );
    if (request.association.version !== 1) {
      throw new Error("Unsupported linked launch association version");
    }
    const association = linkedAssociationPointer(
      request.association.planId ?? 0,
      request.association.taskId,
    );
    const activeResources = this.#activeRuntime().resources;
    const rows = activeResources?.terminal.rows ?? 24;
    const columns = activeResources?.terminal.cols ?? 80;

    // Set from inside the stage callback, so it lives on an object the
    // compiler does not narrow back to its initial null.
    const persistenceStage: { release: (() => void) | null } = { release: null };
    try {
      await completeLinkedLaunchTransaction<TerminalSession, LinkedTabStage>({
        launch: () => this.#backend.LaunchLinkedAgent(
          profile.id,
          request.cwd ?? "",
          rows,
          columns,
          association,
        ),
        createTab: (session) => {
          if (this.#disposed || session.profileId !== profile.id) return null;
          // Persist the last committed workspace before staging. While the
          // backend session is unattached, neither timers nor project teardown
          // may serialize the tentative linked descriptor.
          persistenceStage.release = this.#linkedPersistenceStage.begin(
            () => this.#flushPersistence(),
          );
          const priorTabIDs = new Set(
            this.#tabController.workspace.tabs.map((tab) => tab.id),
          );
          const workspace = this.#tabController.dispatch({
            type: "create-tab",
            title: request.title,
            profileId: profile.id,
            cwd: session.cwd,
            association,
          });
          if (!workspace) return null;
          const tab = workspace.tabs.find(
            (candidate) => candidate.id === workspace.activeTabId,
          );
          if (!tab || tab.association === undefined) {
            const staged = workspace.tabs.find((candidate) =>
              !priorTabIDs.has(candidate.id)
            );
            if (staged) {
              this.#authorizedRuntimeRemoval.add(staged.activePaneId);
              this.#tabController.dispatch({
                type: "close-tab",
                tabId: staged.id,
              });
            }
            return null;
          }
          return { tab, paneId: tab.activePaneId };
        },
        attach: async (stage, session) => {
          const descriptor = activeTerminalDescriptor(
            this.#tabController.workspace,
          );
          if (
            this.#disposed ||
            !descriptor ||
            descriptor.tabId !== stage.tab.id ||
            descriptor.pane.paneId !== stage.paneId
          ) throw new Error("Linked terminal tab is no longer active");
          const runtime = this.#runtimes.ensure(stage.paneId);
          this.#lifecycle.prepareOpen(stage.paneId);
          this.#teardownRuntime(runtime);
          runtime.activity = resetPaneActivity("agent");
          const ticket = this.#runtimes.begin(stage.paneId);
          runtime.session = null;
          runtime.closing = false;
          runtime.title = "";
          this.#body.hidden = false;
          this.#message.hidden = true;
          this.#setState(runtime, "opening");

          await new Promise<void>((resolve) =>
            requestAnimationFrame(() => resolve())
          );
          if (
            !this.#accepts(runtime, ticket) ||
            !this.#descriptorFor(stage.paneId)
          ) throw new Error("Linked terminal tab closed before attachment");
          const resources = this.#createRenderer(runtime, ticket);
          runtime.resources = resources;
          this.#updateWebglPolicy();
          if (this.#isActive(runtime)) this.#renderState();
          this.#fit(runtime, resources, false);
          if (!(await this.#attachCreatedSession(
            runtime,
            descriptor,
            ticket,
            resources,
            session,
          ))) throw new Error("Linked terminal session could not be attached");
        },
        closeSession: (session) =>
          this.#backend.RollbackLinkedAgent(session.sessionId),
        rollbackTab: (stage) => {
          const exists = this.#tabController.workspace.tabs.some(
            (tab) => tab.id === stage.tab.id,
          );
          if (!exists) return;
          this.#authorizedRuntimeRemoval.add(stage.paneId);
          const removed = this.#tabController.dispatch({
            type: "close-tab",
            tabId: stage.tab.id,
          });
          if (!removed && !this.#disposed) {
            this.#authorizedRuntimeRemoval.delete(stage.paneId);
            throw new Error("Could not roll back linked terminal tab");
          }
        },
      });
    } finally {
      const releasePersistenceStage = persistenceStage.release;
      if (releasePersistenceStage !== null) {
        releasePersistenceStage();
        if (!this.#disposed) this.#markPersistenceDirty();
      }
    }
    if (this.#disposed) return;
    this.#terminalHidden = false;
    this.#renderPanelVisibility();
    requestAnimationFrame(() => {
      if (!this.#disposed) this.#activeRuntime().resources?.terminal.focus();
    });
  }

  associationState(): ActiveTerminalAssociation | null {
    if (this.#disposed) return null;
    const active = activeTerminalDescriptor(this.#tabController.workspace);
    if (!active) return null;
    const tab = this.#tabController.workspace.tabs.find(
      (candidate) => candidate.id === active.tabId,
    );
    const runtime = this.#runtimes.get(active.pane.paneId);
    const session = runtime?.session;
    const revision = session?.associationRevision ?? 0;
    if (
      !tab || !runtime || runtime.state !== "running" || runtime.busy ||
      runtime.closing || !session || paneIds(tab.root).length !== 1 ||
      !Number.isSafeInteger(revision) || revision < 0
    ) return null;
    return {
      generation: this.#workspaceGeneration,
      tabId: tab.id,
      paneId: active.pane.paneId,
      sessionId: session.sessionId,
      revision,
      ...(tab.association === undefined
        ? {}
        : { pointer: tab.association }),
    };
  }

  async mutateAssociation(
    expected: ActiveTerminalAssociation,
    association?: AssociationPointerV1,
    accepts: () => boolean = () => true,
  ): Promise<ActiveTerminalAssociation> {
    return commitTerminalAssociationMutation({
      expected,
      pointer: association,
      current: () => accepts() ? this.associationState() : null,
      mutate: (sessionID, revision, pointer) =>
        this.#backend.MutateTerminalAssociation(sessionID, revision, pointer),
      commit: (next) => {
        const current = this.associationState();
        if (!current || current.sessionId !== next.sessionId ||
          current.tabId !== next.tabId || current.paneId !== next.paneId) {
          throw new Error("Terminal association target changed before commit");
        }
        const runtime = this.#runtimes.get(next.paneId);
        if (!runtime?.session || runtime.session.sessionId !== next.sessionId) {
          throw new Error("Terminal session changed before association commit");
        }
        runtime.session.associationRevision = next.revision;
        const before = this.#tabController.workspace;
        const after = this.#tabController.dispatch({
          type: "set-tab-association",
          tabId: next.tabId,
          association: next.pointer,
        });
        if (!after && !associationPointersMatch(
          before.tabs.find((tab) => tab.id === next.tabId)?.association,
          next.pointer,
        )) {
          throw new Error("Terminal association pointer could not be committed");
        }
        this.#markPersistenceDirty();
        this.#flushPersistence();
        this.#renderState();
      },
    });
  }

  async previewWriteback(
    expected: ActiveTerminalAssociation,
    kind: TerminalWritebackKind,
    content: string,
    accepts: () => boolean = () => true,
  ): Promise<TerminalWritebackPreview> {
    if (!accepts() || !terminalWritebackStateMatches(expected, this.associationState())) {
      throw new Error("Terminal write-back target changed");
    }
    const result = await this.#backend.PreviewTerminalWriteback(
      expected.sessionId, expected.revision, kind, content,
    );
    if (!accepts() || !terminalWritebackStateMatches(expected, this.associationState()) ||
      result.generation !== expected.generation ||
      result.sessionId !== expected.sessionId || result.revision !== expected.revision ||
      result.kind !== kind) {
      throw new Error("Stale terminal write-back preview ignored");
    }
    return result;
  }

  async writeback(
    expected: ActiveTerminalAssociation,
    requestID: string,
    kind: TerminalWritebackKind,
    content: string,
    confirmSummary: boolean,
    accepts: () => boolean = () => true,
  ): Promise<TerminalWritebackResult> {
    if (!accepts() || !terminalWritebackStateMatches(expected, this.associationState())) {
      throw new Error("Terminal write-back target changed");
    }
    const result = await this.#backend.WriteTerminalMemory(
      expected.sessionId, expected.revision, requestID, kind, content,
      confirmSummary,
    );
    if (!accepts() || !terminalWritebackStateMatches(expected, this.associationState()) ||
      result.generation !== expected.generation ||
      result.sessionId !== expected.sessionId || result.revision !== expected.revision ||
      result.requestId !== requestID || result.kind !== kind) {
      throw new Error("Stale terminal write-back result ignored");
    }
    return result;
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#persistenceScheduler.dispose();
    this.#scratchpadSaver.dispose();
    this.#scratchpadDragCleanup?.();
    this.#disposed = true;
    this.#pendingSessionCloses.releaseToProjectShutdown();
    this.#tabBar.dispose();
    this.#splitView.dispose();
    this.#boardHidden = false;
    this.#terminalHidden = false;
    this.#renderPanelVisibility();
    const runtimes = this.#runtimes.values();
    this.#lifecycle.releaseManyLocal(runtimes.map((runtime) => runtime.paneId));
    for (const runtime of runtimes) this.#runtimes.remove(runtime.paneId);
    this.#tabController.dispose();
    this.#boardToggle.disabled = true;
    this.#terminalToggle.disabled = true;
    this.#association.disabled = true;
    this.#writeback.disabled = true;
    this.#searchOpen.disabled = true;
    this.#clear.disabled = true;
    this.#dragCleanup?.();
    this.#diagnosticsObserver?.disconnect();
    this.#diagnosticsObserver = null;
    this.#finishPasteConfirmation(false);
    this.#finishTerminationConfirmation(false);
    this.#hideContextMenu();
    this.#earlyExit.clear();
    for (const dispose of this.#dockDisposers.splice(0)) dispose();
  }

  setVisible(visible: boolean): void {
    if (this.#disposed || this.#workspaceViewVisible === visible) return;
    this.#workspaceViewVisible = visible;
    this.#renderPanelVisibility();
  }

  setModernUnicode(enabled: boolean): void {
    if (this.#disposed || this.#modernUnicodeEnabled === enabled) return;
    this.#setModernUnicode(enabled);
  }

  startSession(): void {
    if (this.#disposed || this.#open.disabled || this.#open.hidden) return;
    void this.#runOperation((runtime) => this.#openTerminal(runtime));
  }

  setLayoutLocked(locked: boolean): void {
    if (this.#disposed) return;
    this.#layoutLocked = locked;
    this.#terminalToggle.disabled = locked;
    this.#boardToggle.disabled = locked || !this.#dockInteractionEligible();
  }

  setApplicationOverlayOpen(open: boolean, focusTerminal: false): void {
    if (this.#disposed || this.#applicationOverlayOpen === open) return;
    this.#applicationOverlayOpen = open;
    this.#renderPanelVisibility(focusTerminal);
  }
}

export function mountTerminalDock(
  options: MountOptions,
): TerminalDockHandle {
  const dock = new TerminalDock(options);
  const ready = dock.initialize();
  return {
    ready,
    agentProfiles: async () => {
      await ready;
      return dock.agentProfiles();
    },
    launchLinked: async (request) => {
      await ready;
      return dock.launchLinked(request);
    },
    associationState: () => dock.associationState(),
    mutateAssociation: async (expected, association, accepts) => {
      await ready;
      return dock.mutateAssociation(expected, association, accepts);
    },
    previewWriteback: async (expected, kind, content, accepts) => {
      await ready;
      return dock.previewWriteback(expected, kind, content, accepts);
    },
    writeback: async (expected, requestID, kind, content, confirmSummary, accepts) => {
      await ready;
      return dock.writeback(
        expected, requestID, kind, content, confirmSummary, accepts,
      );
    },
    setVisible: (visible) => dock.setVisible(visible),
    setLayoutLocked: (locked) => dock.setLayoutLocked(locked),
    setModernUnicode: (enabled) => dock.setModernUnicode(enabled),
    startSession: () => dock.startSession(),
    setApplicationOverlayOpen: (open, focusTerminal) =>
      dock.setApplicationOverlayOpen(open, focusTerminal),
    flushPending: () => dock.flushPending(),
    dispose: () => dock.dispose(),
  };
}

function associationPointersMatch(
  left: AssociationPointerV1 | undefined,
  right: AssociationPointerV1 | undefined,
): boolean {
  return left?.version === right?.version && left?.planId === right?.planId &&
    left?.taskId === right?.taskId;
}
