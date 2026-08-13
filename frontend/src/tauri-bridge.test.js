import { describe, expect, it, vi } from "vitest";

import { COMMANDS, installTauriBridge } from "./tauri-bridge";

describe("Tauri compatibility bridge", () => {
  it("pins the exact current command allowlist", () => {
    expect(COMMANDS).toEqual([
      "AcknowledgeAgentHandoffV2", "AddTask", "AddTaskNote", "AddTaskNoteV2",
      "AddTaskV2", "ApplyUpdate", "ApproveAgentWorkflowV2", "AssociateAgentRunV2",
      "AssociateTerminalV2", "CancelUpdateOperation", "CancelWorkspaceChange",
      "CheckForUpdates", "CloseProject", "CloseTerminal", "CloseTerminalV2",
      "CreateTerminal", "CreateTerminalV2", "DisableCapabilityV2",
      "DismissAgentWorkflowV2", "DownloadUpdate", "EnableCapabilityV2",
      "ExpireCapabilityV2", "GetActivityHeatmapV2", "GetAgentIntelligenceV2",
      "GetAgentRunsV2", "GetBoard", "GetBoardV2", "GetCapabilitiesV2",
      "GetCapabilityAuditsV2", "GetRecentProjects", "GetTaskDetailV2",
      "GetTerminalProfiles", "GetTerminalProfilesV2", "GetUpdateState",
      "GetWorkspaceSnapshot", "GetWorkspaceState", "InstallShellCommand",
      "LaunchLinkedAgentV2", "MoveTask", "MoveTaskV2", "MoveTaskV3",
      "MutateTerminalAssociationV2", "OpenHelpDestination", "OpenProject",
      "PickProjectDirectory", "PrepareAgentWorkflowV2", "PreviewAgentHandoffV2",
      "PreviewCapabilityV2", "PreviewTerminalWritebackV2", "RemoveCapabilityV2",
      "RenameTask", "RenameTaskV2", "ResizeTerminal", "ResizeTerminalV2",
      "RollbackLinkedAgentLaunchV2", "SaveCapabilityV2", "SearchV2",
      "SendAgentHandoffV2", "SetAgentTaskOwnershipV2", "SetAgentWorktreeV2",
      "SetAutomaticUpdateChecks", "TestCapabilityV2", "ValidateTerminalCWDsV2",
      "WriteTerminalMemoryV2",
    ]);
  });

  it("preserves call ordering and routes native-only methods", async () => {
    const calls = [];
    const target = { __TAURI_INTERNALS__: {}, navigator: { clipboard: {} } };
    installTauriBridge(target, {
      invoke: async (command, payload) => {
        calls.push([command, payload]);
        if (command === "gui_invoke" && payload.request.method === "OpenHelpDestination") {
          return "https://docs.example/help";
        }
        return { ok: true };
      },
      listen: vi.fn(),
      clipboard: { readText: vi.fn(), writeText: vi.fn() },
    });

    await target.go.gui.App.MoveTaskV3(7, 9, "done", "token");
    await target.go.gui.App.PickProjectDirectory();
    expect(await target.go.gui.App.InstallShellCommand()).toBeUndefined();
    expect(await target.go.gui.App.OpenHelpDestination("terminals")).toBeUndefined();
    expect(calls).toEqual([
      ["gui_invoke", { request: { method: "MoveTaskV3", arguments: [7, 9, "done", "token"] } }],
      ["pick_project_directory", undefined],
      ["gui_invoke", { request: { method: "InstallShellCommand", arguments: [] } }],
      ["gui_invoke", { request: { method: "OpenHelpDestination", arguments: ["terminals"] } }],
      ["open_external_url", { url: "https://docs.example/help" }],
    ]);
  });

  it("unwraps event payloads and closes a late listener exactly once", async () => {
    let listener;
    let resolveListen;
    const dispose = vi.fn();
    const listenPromise = new Promise((resolve) => { resolveListen = resolve; });
    const target = { __TAURI_INTERNALS__: {}, navigator: { clipboard: {} } };
    installTauriBridge(target, {
      invoke: vi.fn(),
      listen: (_name, callback) => { listener = callback; return listenPromise; },
      clipboard: { readText: vi.fn(), writeText: vi.fn() },
    });
    const callback = vi.fn();
    const unlisten = target.runtime.EventsOnMultiple("terminal:exit", callback, -1);
    listener({ payload: { sessionId: "s1" } });
    expect(callback).toHaveBeenCalledWith({ sessionId: "s1" });
    unlisten();
    listener({ payload: { sessionId: "late" } });
    resolveListen(dispose);
    await listenPromise;
    await Promise.resolve();
    expect(callback).toHaveBeenCalledTimes(1);
    expect(dispose).toHaveBeenCalledTimes(1);
  });

  it("routes browser and clipboard helpers with truthy write success", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    const clipboard = {
      readText: vi.fn().mockResolvedValue("copied"),
      writeText: vi.fn().mockResolvedValue(undefined),
    };
    const target = { __TAURI_INTERNALS__: {}, navigator: { clipboard } };
    installTauriBridge(target, { invoke, listen: vi.fn(), clipboard });
    await target.runtime.BrowserOpenURL("https://example.com");
    expect(invoke).toHaveBeenCalledWith("open_external_url", { url: "https://example.com" });
    await expect(target.runtime.ClipboardGetText()).resolves.toBe("copied");
    await expect(target.runtime.ClipboardSetText("text")).resolves.toBe(true);
    expect(clipboard.writeText).toHaveBeenCalledWith("text");
  });

  it("normalizes backend and native rejection values to Error objects", async () => {
    const invoke = vi.fn(async (command) => {
      if (command === "pick_project_directory") throw "picker failed";
      throw { message: "backend failed" };
    });
    const clipboard = {
      readText: vi.fn().mockRejectedValue("clipboard failed"),
      writeText: vi.fn(),
    };
    const target = { __TAURI_INTERNALS__: {}, navigator: { clipboard } };
    installTauriBridge(target, { invoke, listen: vi.fn(), clipboard });
    await expect(target.go.gui.App.GetBoardV2(7, 1)).rejects.toEqual(
      new Error("backend failed"),
    );
    await expect(target.go.gui.App.PickProjectDirectory()).rejects.toEqual(
      new Error("picker failed"),
    );
    await expect(target.runtime.ClipboardGetText()).rejects.toEqual(
      new Error("clipboard failed"),
    );
  });
});
