import type { WorkspaceStatus } from "./controller";

export type NativeMenuEventSubscriber = (
  name: string,
  callback: () => void,
) => () => void;

export interface NativeMenuActions {
  openProject(): void;
  switchProject(): void;
  closeProject(): void;
  showSettings(): void;
  showCapabilities(): void;
  showBoard(): void;
  showIntelligence(): void;
  toggleTerminalPanel(): void;
  toggleCommandPalette(): void;
  installShellCommand(): void;
  checkForUpdates(): void;
}

export type NativeMenuCommand = keyof NativeMenuActions;

export type NativeMenuView = "board" | "overview" | "settings";

export function nativeMenuViewTarget(
  command: NativeMenuCommand,
): NativeMenuView | null {
  if (command === "showBoard") return "board";
  if (command === "showIntelligence") return "overview";
  if (command === "showSettings" || command === "showCapabilities") {
    return "settings";
  }
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
    command === "checkForUpdates"
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
  ["workspace:capabilities-requested", "showCapabilities"],
  ["workspace:board-requested", "showBoard"],
  ["workspace:intelligence-requested", "showIntelligence"],
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
