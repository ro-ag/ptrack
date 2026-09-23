// The shared context every window controller is built from: the backend
// bridge, the workspace generation fence, the status line and toast, the
// state more than one controller reads, and a late-bound handle to each
// controller so a view can reach another one without importing it.
//
// Controllers are created in any order and may only call each other once
// every one exists: a handle read before its controller is installed throws
// instead of answering with half a window.

import type { Backend } from "../backend";
import type { Preferences } from "../settings/preferences";
import { defaultPreferences } from "../settings/preferences";
import type { SettingsController } from "../settings/controller";
import type { AgentLaunchController } from "../terminal/agent-launch-controller";
import type { AssociationController } from "../terminal/association-controller";
import type { TerminalBackendController } from "../terminal/backend";
import type { TerminalDockHandle } from "../terminal/pane";
import type { TerminalWindow } from "../terminal/window";
import type { WritebackController } from "../terminal/writeback-controller";
import type { UpdatesController } from "../updates/controller";
import type { AgentActivityView } from "./agent-activity-view";
import type { BoardView } from "./board-view";
import { RefreshGate, WorkspaceController } from "./controller";
import { element } from "./dom";
import type { FirstPlanState } from "./first-plan";
import { initialFirstPlanState } from "./first-plan";
import type { FirstPlanView } from "./first-plan-view";
import type { FirstRunState } from "./first-run";
import { initialFirstRunState } from "./first-run";
import type { FirstRunView } from "./first-run-view";
import { messageFrom } from "./format";
import type { IssuesView } from "./issues-view";
import type { LandingController } from "./landing";
import type { LayoutController } from "./layout-controller";
import type { NativeMenuController } from "./native-menu";
import type { OverviewView } from "./overview-view";
import type { Palette } from "./palette";
import type { PlanDialogs } from "./plan-dialogs";
import type { ProjectLifecycle } from "./project-lifecycle";
import type { RecentProjectsState } from "./recent-projects";
import { initialRecentProjectsState } from "./recent-projects";
import type { RecentProjectsController } from "./recent-projects-controller";
import type { Shortcuts } from "./shortcuts";
import type { Board, BoardTask, WorkspaceSnapshot, WorkspaceStateResponse } from "./snapshot-types";
import type { SnapshotController } from "./snapshot-controller";
import type { TaskDrawer } from "./task-drawer";
import type { WorkspaceShell } from "./workspace-shell";

export type WorkspaceView = "board" | "overview" | "issues";

export type NoticeTone = "error" | "warning";

/** State more than one controller reads or writes. */
export interface AppState {
  workspaceState: WorkspaceStateResponse;
  view: WorkspaceView;
  snapshot: WorkspaceSnapshot | null;
  board: Board | null;
  firstRunState: FirstRunState;
  firstPlanState: FirstPlanState;
  detailTask: BoardTask | null;
  terminalHandle: TerminalDockHandle | null;
  recentProjectsState: RecentProjectsState;
  preferences: Preferences;
  expandedLanes: Set<string>;
  foldedLanes: Set<string>;
}

export interface AppCore {
  api(): Backend;
  openHelpDestination(destination: string): void;
  showError(error: unknown): void;
  showNotice(message: string, tone?: NoticeTone): void;
  setStatus(message: string): void;
  readonly workspaceController: WorkspaceController;
  readonly refreshGate: RefreshGate;
  readonly nativeEventDisposers: Array<() => void>;
  readonly state: AppState;
}

export interface AppModules {
  layout: LayoutController;
  overview: OverviewView;
  issues: IssuesView;
  board: BoardView;
  agentActivity: AgentActivityView;
  snapshot: SnapshotController;
  planDialogs: PlanDialogs;
  updates: UpdatesController;
  settings: SettingsController;
  palette: Palette;
  drawer: TaskDrawer;
  agentLaunch: AgentLaunchController;
  association: AssociationController;
  writeback: WritebackController;
  terminalBackend: TerminalBackendController;
  recent: RecentProjectsController;
  firstRun: FirstRunView;
  firstPlan: FirstPlanView;
  shell: WorkspaceShell;
  lifecycle: ProjectLifecycle;
  shortcuts: Shortcuts;
  nativeMenu: NativeMenuController;
  landing: LandingController;
  terminalWindow: TerminalWindow;
}

export type AppContext = AppCore & AppModules;

export function createAppState(): AppState {
  return {
    workspaceState: { status: "welcome", generation: 0 },
    view: "board",
    snapshot: null,
    board: null,
    firstRunState: { ...initialFirstRunState },
    firstPlanState: { ...initialFirstPlanState },
    detailTask: null,
    terminalHandle: null,
    recentProjectsState: { ...initialRecentProjectsState },
    preferences: defaultPreferences,
    expandedLanes: new Set(),
    foldedLanes: new Set(),
  };
}

/** The backend bridge `tauri-bridge.js` installs, or an error until it has. */
export function desktopBackend(): Backend {
  const backend = window.go?.gui?.App;
  if (!backend) throw new Error("The p-track backend is not ready");
  return backend;
}

export function createAppCore(state: AppState = createAppState()): AppCore {
  const toast = element("#toast", HTMLDivElement);
  const status = element("#status", HTMLSpanElement);
  let toastTimer = 0;

  // Errors and cautions share the toast so they look and stack alike; only
  // the tone differs.
  const showNotice = (message: string, tone: NoticeTone = "error") => {
    window.clearTimeout(toastTimer);
    toast.textContent = message;
    toast.dataset.tone = tone;
    toast.hidden = false;
    toastTimer = window.setTimeout(() => {
      toast.hidden = true;
    }, 5000);
  };
  const showError = (error: unknown) => {
    showNotice(messageFrom(error), "error");
  };
  return {
    api: desktopBackend,
    openHelpDestination(destination) {
      void desktopBackend().OpenHelpDestination(destination).catch(() => {
        showError(new Error("Could not open the Help Center."));
      });
    },
    showError,
    showNotice,
    setStatus(message) {
      status.textContent = message;
    },
    workspaceController: new WorkspaceController(),
    refreshGate: new RefreshGate(),
    nativeEventDisposers: [],
    state,
  };
}

export interface AppContextBuilder {
  readonly ctx: AppContext;
  install<K extends keyof AppModules>(key: K, module: AppModules[K]): void;
}

export function createAppContext(core: AppCore): AppContextBuilder {
  const slots: Partial<AppModules> = {};
  const moduleOf = <K extends keyof AppModules>(key: K): AppModules[K] => {
    const module = slots[key];
    if (!module) throw new Error(`The ${key} controller is not installed yet`);
    return module;
  };
  const ctx: AppContext = {
    ...core,
    get layout() { return moduleOf("layout"); },
    get overview() { return moduleOf("overview"); },
    get issues() { return moduleOf("issues"); },
    get board() { return moduleOf("board"); },
    get agentActivity() { return moduleOf("agentActivity"); },
    get snapshot() { return moduleOf("snapshot"); },
    get planDialogs() { return moduleOf("planDialogs"); },
    get updates() { return moduleOf("updates"); },
    get settings() { return moduleOf("settings"); },
    get palette() { return moduleOf("palette"); },
    get drawer() { return moduleOf("drawer"); },
    get agentLaunch() { return moduleOf("agentLaunch"); },
    get association() { return moduleOf("association"); },
    get writeback() { return moduleOf("writeback"); },
    get terminalBackend() { return moduleOf("terminalBackend"); },
    get recent() { return moduleOf("recent"); },
    get firstRun() { return moduleOf("firstRun"); },
    get firstPlan() { return moduleOf("firstPlan"); },
    get shell() { return moduleOf("shell"); },
    get lifecycle() { return moduleOf("lifecycle"); },
    get shortcuts() { return moduleOf("shortcuts"); },
    get nativeMenu() { return moduleOf("nativeMenu"); },
    get landing() { return moduleOf("landing"); },
    get terminalWindow() { return moduleOf("terminalWindow"); },
  };
  return {
    ctx,
    install(key, module) {
      if (slots[key]) throw new Error(`The ${key} controller is already installed`);
      slots[key] = module;
    },
  };
}
