import type { WorkspaceStatus } from "./controller";
import type { AppContext } from "./app-context";
import { element } from "./dom";
import { messageFrom } from "./format";
import { runtimeEventIsCurrent } from "./presentation";
import { updateStateFrom } from "../updates/controller";

export type NativeMenuEventSubscriber = (
  name: string,
  callback: () => void,
) => () => void;

export interface NativeMenuActions {
  openProject(): void;
  switchProject(): void;
  closeProject(): void;
  showSettings(): void;
  showBoard(): void;
  showIntelligence(): void;
  showIssues(): void;
  toggleTerminalPanel(): void;
  toggleCommandPalette(): void;
  installShellCommand(): void;
  checkForUpdates(): void;
}

export type NativeMenuCommand = keyof NativeMenuActions;

export type NativeMenuView = "board" | "overview" | "issues";

// Settings is an application dialog, not a view, so showSettings has no view
// target.
export function nativeMenuViewTarget(
  command: NativeMenuCommand,
): NativeMenuView | null {
  if (command === "showBoard") return "board";
  if (command === "showIntelligence") return "overview";
  if (command === "showIssues") return "issues";
  return null;
}

export interface NativeMenuCommandState {
  workspaceStatus: WorkspaceStatus;
  openOverlayIDs: readonly string[];
  focusTarget: "input" | "terminal" | "other";
}

// Native menu commands are explicit user actions, so retained DOM focus must
// not suppress them. Application overlays and workspace lifecycle state are
// the only gates; keyboard shortcuts apply their separate input/terminal
// guards in app.js.
export function nativeMenuCommandAllowed(
  command: NativeMenuCommand,
  state: NativeMenuCommandState,
): boolean {
  const paletteOpen = state.openOverlayIDs.includes("palette");
  if (command === "toggleCommandPalette" && paletteOpen) return true;
  if (state.openOverlayIDs.length > 0) return false;
  if (
    command === "openProject" ||
    command === "installShellCommand" ||
    command === "checkForUpdates" ||
    command === "showSettings"
  ) {
    return ["welcome", "open", "error", "closed"].includes(state.workspaceStatus);
  }
  return state.workspaceStatus === "open";
}

const nativeMenuBindings: ReadonlyArray<
  readonly [string, keyof NativeMenuActions]
> = [
  ["workspace:open-requested", "openProject"],
  ["workspace:switch-requested", "switchProject"],
  ["workspace:close-requested", "closeProject"],
  ["workspace:settings-requested", "showSettings"],
  ["workspace:board-requested", "showBoard"],
  ["workspace:intelligence-requested", "showIntelligence"],
  ["workspace:issues-requested", "showIssues"],
  ["workspace:terminal-panel-toggle-requested", "toggleTerminalPanel"],
  ["workspace:command-palette-requested", "toggleCommandPalette"],
  ["workspace:install-shell-command-requested", "installShellCommand"],
  ["update:open-requested", "checkForUpdates"],
];

export function registerNativeMenuActions(
  subscribe: NativeMenuEventSubscriber,
  actions: NativeMenuActions,
): Array<() => void> {
  return nativeMenuBindings.map(([eventName, actionName]) =>
    subscribe(eventName, () => actions[actionName]())
  );
}

// ------------------------------------------------ native menu controller

export function createNativeMenuController(ctx: AppContext) {
  const { api, nativeEventDisposers, showError, showNotice, workspaceController } = ctx;
  const elements = {
    appVersion: element("#app-version", HTMLButtonElement),
    palette: element("#palette", HTMLDivElement),
    settingsOpen: element("#settings-open", HTMLButtonElement),
  };

  function nativeMenuOpenOverlayIDs(): string[] {
    return Array.from(
      document.querySelectorAll<HTMLElement>("body > .modal, body > [data-terminal-overlay]"),
    ).filter((overlay) => !overlay.hidden).map((overlay) =>
      overlay.id || (overlay.hasAttribute("data-terminal-overlay")
        ? "terminal-overlay"
        : "application-overlay")
    );
  }

  function nativeMenuFocusTarget(): NativeMenuCommandState["focusTarget"] {
    const active = document.activeElement;
    if (active instanceof Element && active.closest("#terminal-dock")) {
      return "terminal";
    }
    if (
      active instanceof HTMLElement &&
      (["INPUT", "SELECT", "TEXTAREA"].includes(active.tagName) ||
        active.isContentEditable)
    ) return "input";
    return "other";
  }

  function nativeCommandAllowed(command: NativeMenuCommand): boolean {
    if (
      ctx.state.firstRunState.phase !== "idle" ||
      ctx.state.firstPlanState.phase !== "idle" ||
      ctx.recent.recentProjectOperationActive()
    ) return false;
    return nativeMenuCommandAllowed(command, {
      workspaceStatus: workspaceController.state.status,
      openOverlayIDs: nativeMenuOpenOverlayIDs(),
      focusTarget: nativeMenuFocusTarget(),
    });
  }

  function eventsOn(name: string, callback: (payload: unknown) => void): () => void {
    const runtime = window.runtime;
    if (typeof runtime?.EventsOnMultiple !== "function") return () => {};
    return runtime.EventsOnMultiple(name, callback, -1);
  }

  function registerNativeProjectActions(): void {
    const showNativeView = (command: NativeMenuCommand) => {
      if (!nativeCommandAllowed(command)) return;
      const target = nativeMenuViewTarget(command);
      if (target) ctx.shell.setView(target, true);
    };
    nativeEventDisposers.push(
      ...registerNativeMenuActions(eventsOn, {
        openProject: () => {
          if (
            ctx.state.firstRunState.phase === "idle" &&
            nativeCommandAllowed("openProject")
          ) void ctx.lifecycle.requestOpenProject();
        },
        switchProject: () => {
          if (
            ctx.state.firstRunState.phase === "idle" &&
            nativeCommandAllowed("switchProject")
          ) void ctx.lifecycle.requestOpenProject();
        },
        closeProject: () => {
          if (
            ctx.state.firstRunState.phase === "idle" &&
            nativeCommandAllowed("closeProject")
          ) void ctx.lifecycle.requestCloseProject();
        },
        showSettings: () => {
          if (nativeCommandAllowed("showSettings")) ctx.settings.openSettings(elements.settingsOpen);
        },
        showBoard: () => {
          showNativeView("showBoard");
        },
        showIntelligence: () => {
          showNativeView("showIntelligence");
        },
        showIssues: () => {
          showNativeView("showIssues");
        },
        toggleTerminalPanel: () => {
          if (nativeCommandAllowed("toggleTerminalPanel")) {
            document.querySelector<HTMLElement>("#terminal-panel-toggle")?.click();
          }
        },
        toggleCommandPalette: () => {
          if (!nativeCommandAllowed("toggleCommandPalette")) return;
          if (elements.palette.hidden) ctx.palette.openPalette();
          else ctx.palette.closePalette();
        },
        installShellCommand: () => {
          if (nativeCommandAllowed("installShellCommand")) {
            void Promise.resolve().then(() => api().InstallShellCommand()).catch((error: unknown) => {
              showError(new Error(`Could not install the shell command: ${messageFrom(error)}`));
            });
          }
        },
        checkForUpdates: () => {
          if (
            nativeCommandAllowed("checkForUpdates") &&
            ctx.updates.openAboutUpdates(elements.appVersion)
          ) void ctx.updates.runUpdateAction("check");
        },
      }),
      eventsOn("update:state-changed", (state) => ctx.updates.renderUpdateState(updateStateFrom(state))),
      // A window close the runtime refused (a call still running) leaves every
      // service up; say why the window stayed open instead of doing nothing.
      eventsOn("app:close-refused", (message) =>
        showNotice(`p-track could not close yet: ${messageFrom(message)}`, "warning"),
      ),
      eventsOn("workspace:data-changed", () =>
        void ctx.snapshot.loadSnapshot(ctx.state.board?.planId || 0, true),
      ),
      eventsOn("workspace:runtime-changed", (generation) => {
        if (!runtimeEventIsCurrent(
          generation,
          workspaceController.state.generation,
          workspaceController.state.status === "open",
        )) return;
        ctx.snapshot.runtimeRefreshes.request(Number(generation));
      }),
    );
  }

  function bind(): void {
    // Native events are subscribed once startup has a workspace to route them
    // to; see registerNativeProjectActions.
  }

  return {
    bind,
    nativeMenuOpenOverlayIDs,
    registerNativeProjectActions,
  };
}

export type NativeMenuController = ReturnType<typeof createNativeMenuController>;
