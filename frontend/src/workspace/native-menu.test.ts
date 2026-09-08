import { describe, expect, it, vi } from "vitest";
import {
  type NativeMenuActions,
  type NativeMenuCommand,
  nativeMenuCommandAllowed,
  nativeMenuViewTarget,
  registerNativeMenuActions,
} from "./native-menu";

describe("native menu event routing", () => {
  it("registers every native action and delegates to the supplied behavior", () => {
    const handlers = new Map<string, () => void>();
    const disposers = Array.from({ length: 11 }, () => vi.fn());
    let disposerIndex = 0;
    const subscribe = vi.fn((name: string, callback: () => void) => {
      handlers.set(name, callback);
      return disposers[disposerIndex++];
    });
    const actionNames: Array<keyof NativeMenuActions> = [
      "openProject",
      "switchProject",
      "closeProject",
      "showSettings",
      "showBoard",
      "showIntelligence",
      "showIssues",
      "toggleTerminalPanel",
      "toggleCommandPalette",
      "installShellCommand",
      "checkForUpdates",
    ];
    const actions = Object.fromEntries(
      actionNames.map((name) => [name, vi.fn()]),
    ) as unknown as NativeMenuActions;

    const registered = registerNativeMenuActions(subscribe, actions);

    expect([...handlers.keys()]).toEqual([
      "workspace:open-requested",
      "workspace:switch-requested",
      "workspace:close-requested",
      "workspace:settings-requested",
      "workspace:board-requested",
      "workspace:intelligence-requested",
      "workspace:issues-requested",
      "workspace:terminal-panel-toggle-requested",
      "workspace:command-palette-requested",
      "workspace:install-shell-command-requested",
      "update:open-requested",
    ]);
    [...handlers.values()].forEach((handler) => handler());
    actionNames.forEach((name) => expect(actions[name]).toHaveBeenCalledOnce());
    expect(registered).toEqual(disposers);
  });

  it("allows explicit workspace commands despite retained input or terminal focus", () => {
    const commands: NativeMenuCommand[] = [
      "showSettings",
      "showBoard",
      "showIntelligence",
      "toggleTerminalPanel",
    ];
    for (const focusTarget of ["input", "terminal"] as const) {
      for (const command of commands) {
        expect(nativeMenuCommandAllowed(command, {
          workspaceStatus: "open",
          openOverlayIDs: [],
          focusTarget,
        })).toBe(true);
      }
    }
  });

  it("maps native view commands to headings that should receive focus", () => {
    expect(nativeMenuViewTarget("showSettings")).toBeNull();
    expect(nativeMenuViewTarget("showBoard")).toBe("board");
    expect(nativeMenuViewTarget("showIntelligence")).toBe("overview");
    expect(nativeMenuViewTarget("showIssues")).toBe("issues");
    expect(nativeMenuViewTarget("toggleTerminalPanel")).toBeNull();
  });

  it.each(["confirm-modal", "terminal-paste-modal"])(
    "blocks workspace and lifecycle commands behind %s",
    (overlayID) => {
      for (const command of [
        "openProject",
        "switchProject",
        "closeProject",
        "showBoard",
        "toggleTerminalPanel",
        "installShellCommand",
      ] as NativeMenuCommand[]) {
        expect(nativeMenuCommandAllowed(command, {
          workspaceStatus: "open",
          openOverlayIDs: [overlayID],
          focusTarget: "other",
        })).toBe(false);
      }
    },
  );

  it("allows only stable-state actions from the unobscured welcome state", () => {
    const state = {
      workspaceStatus: "welcome" as const,
      openOverlayIDs: [],
      focusTarget: "other" as const,
    };
    expect(nativeMenuCommandAllowed("openProject", state)).toBe(true);
    expect(nativeMenuCommandAllowed("installShellCommand", state)).toBe(true);
    expect(nativeMenuCommandAllowed("checkForUpdates", state)).toBe(true);
    expect(nativeMenuCommandAllowed("showSettings", state)).toBe(true);
    expect(nativeMenuCommandAllowed("switchProject", state)).toBe(false);
    expect(nativeMenuCommandAllowed("closeProject", state)).toBe(false);
    expect(nativeMenuCommandAllowed("showBoard", state)).toBe(false);
  });

  it("allows open and shell installation only in stable workspace states", () => {
    for (const workspaceStatus of ["welcome", "error", "closed", "open"] as const) {
      const state = {
        workspaceStatus,
        openOverlayIDs: [],
        focusTarget: "other" as const,
      };
      expect(nativeMenuCommandAllowed("openProject", state)).toBe(true);
      expect(nativeMenuCommandAllowed("installShellCommand", state)).toBe(true);
    }
    for (const command of [
      "openProject",
      "installShellCommand",
      "checkForUpdates",
    ] as const) {
      expect(nativeMenuCommandAllowed(command, {
        workspaceStatus: "loading",
        openOverlayIDs: [],
        focusTarget: "other",
      })).toBe(false);
    }
    expect(nativeMenuCommandAllowed("showBoard", {
      workspaceStatus: "loading",
      openOverlayIDs: [],
      focusTarget: "other",
    })).toBe(false);
    expect(nativeMenuCommandAllowed("toggleCommandPalette", {
      workspaceStatus: "loading",
      openOverlayIDs: [],
      focusTarget: "other",
    })).toBe(false);
  });

  it("opens a clean workspace palette and always permits closing an open palette", () => {
    expect(nativeMenuCommandAllowed("toggleCommandPalette", {
      workspaceStatus: "open",
      openOverlayIDs: [],
      focusTarget: "input",
    })).toBe(true);
    expect(nativeMenuCommandAllowed("toggleCommandPalette", {
      workspaceStatus: "welcome",
      openOverlayIDs: [],
      focusTarget: "other",
    })).toBe(false);
    expect(nativeMenuCommandAllowed("toggleCommandPalette", {
      workspaceStatus: "open",
      openOverlayIDs: ["memory-modal"],
      focusTarget: "other",
    })).toBe(false);
    expect(nativeMenuCommandAllowed("toggleCommandPalette", {
      workspaceStatus: "loading",
      openOverlayIDs: ["palette", "terminal-paste-modal"],
      focusTarget: "terminal",
    })).toBe(true);
  });
});
