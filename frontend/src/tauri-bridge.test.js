import { describe, expect, it, vi } from "vitest";

import { COMMANDS, installTauriBridge } from "./tauri-bridge";

describe("Tauri compatibility bridge", () => {
  it("pins the exact current command allowlist", () => {
    expect(COMMANDS).toEqual([
      "AcknowledgeAgentHandoffV2", "AddTask", "AddTaskNote", "AddTaskNoteV2",
      "AddTaskV2", "ApplyUpdate", "ApproveAgentWorkflowV2", "AssociateAgentRunV2",
      "AssociateTerminalV2", "CancelUpdateOperation", "CancelWorkspaceChange",
      "CheckForUpdates", "ClaimTerminalStream", "CloseProject", "CloseTerminal",
      "CloseTerminalV2", "CompletePlanV1", "CopyPlanV1",
      "CreateFirstPlanV1", "CreateFirstTaskV1", "CreateTerminal", "CreateTerminalV2", "DeletePlanV1", "DisableCapabilityV2",
      "DismissAgentWorkflowV2", "DownloadUpdate", "EnableCapabilityV2",
      "ExpireCapabilityV2", "ForgetRecentProjectV1", "GetActivityHeatmapV2", "GetAgentIntelligenceV2",
      "GetAgentRunsV2", "GetBoard", "GetBoardV2", "GetCapabilitiesV2",
      "GetCapabilityAuditsV2", "GetDiagnosticsReport", "GetInitializationStatusV1",
      "GetLayoutState", "GetPendingInitializationV1",
      "GetPreferences", "GetRecentProjects", "GetRecentProjectsV1", "GetRepoStatsV1", "GetTaskDetailV2",
      "GetTerminalProfiles", "GetTerminalProfilesV2", "GetTerminalWindowTab",
      "GetUpdateState",
      "GetWorkspaceSnapshot", "GetWorkspaceState", "InitializeProjectV1", "InstallShellCommand",
      "LaunchLinkedAgentV2", "ListProjectsV1", "MoveTask", "MoveTaskV2", "MoveTaskV3", "MovePlanV1",
      "MutateTerminalAssociationV2", "OpenHelpDestination", "OpenProject", "OpenRecentProjectV1",
      "OpenTerminalWindow",
      "PickProjectDirectory", "PrepareAgentWorkflowV2", "PreviewAgentHandoffV2",
      "PreviewCapabilityV2", "PreviewProjectGuideV1", "PreviewTerminalWritebackV2", "RemoveCapabilityV2",
      "RenamePlanV1", "RenameTask", "RenameTaskV2", "ResetApplicationState", "ResetPreferences", "ResetWindowLayout",
      "ResizeTerminal", "ResizeTerminalV2", "ResolveRecentProjectV1",
      "RollbackLinkedAgentLaunchV2", "SaveCapabilityV2", "SearchV2",
      "SendAgentHandoffV2", "SetAgentTaskOwnershipV2", "SetAgentWorktreeV2",
      "SetAutomaticUpdateChecks", "SetLayoutState", "SetPlanHoldV1", "SetPreferences", "SetTerminalWindowTab",
      "StartFirstTaskV1",
      "TestCapabilityV2", "ValidateProjectTargetV1",
      "ValidateTerminalCWDsV2",
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
        if (command === "gui_invoke" && payload.request.method === "ValidateProjectTargetV1") {
          return {
            kind: "new",
            canonicalRoot: "/project",
            operationId: "operation-1",
            initialization: {
              operationId: "operation-1",
              canonicalRoot: "/project",
              outcome: "in-progress",
              checkpoint: "guide-applied",
              errorKind: "",
            },
            goal: "Ship safely",
            guideChoice: "install",
          };
        }
        return { ok: true };
      },
      listen: vi.fn(),
      clipboard: { readText: vi.fn(), writeText: vi.fn() },
    });

    await target.go.gui.App.MoveTaskV3(7, 9, "done", "token");
    await target.go.gui.App.PickProjectDirectory("initialize");
    const validation = await target.go.gui.App.ValidateProjectTargetV1("/project");
    expect(validation).toMatchObject({
      operationId: "operation-1",
      goal: "Ship safely",
      guideChoice: "install",
      initialization: { checkpoint: "guide-applied" },
    });
    await target.go.gui.App.PreviewProjectGuideV1({
      operationId: "operation-1",
      root: "/project",
    });
    await target.go.gui.App.InitializeProjectV1({
      operationId: "operation-1",
      root: "/project",
      goal: "Ship safely",
      guideChoice: "install",
      guidePreviewToken: "preview-token",
    });
    await target.go.gui.App.GetInitializationStatusV1("operation-1");
    await target.go.gui.App.GetPendingInitializationV1();
    await target.go.gui.App.CreateFirstPlanV1(7, "First plan");
    await target.go.gui.App.CreateFirstTaskV1(7, 11, "First task");
    await target.go.gui.App.StartFirstTaskV1(7, 21, "2026-08-14T12:00:00Z");
    await target.go.gui.App.GetRecentProjectsV1();
    await target.go.gui.App.ResolveRecentProjectV1("entry-a", "base-a", "/project");
    await target.go.gui.App.OpenRecentProjectV1(
      "entry-a", "base-a", "/project", "relocation-token", "workspace-token",
    );
    await target.go.gui.App.ForgetRecentProjectV1("entry-a", "base-a");
    expect(await target.go.gui.App.InstallShellCommand()).toBeUndefined();
    expect(await target.go.gui.App.OpenHelpDestination("terminals")).toBeUndefined();
    expect(calls).toEqual([
      ["gui_invoke", { request: { method: "MoveTaskV3", arguments: [7, 9, "done", "token"] } }],
      ["pick_project_directory", { purpose: "initialize" }],
      ["gui_invoke", {
        request: { method: "ValidateProjectTargetV1", arguments: ["/project"] },
      }],
      ["gui_invoke", {
        request: {
          method: "PreviewProjectGuideV1",
          arguments: [{ operationId: "operation-1", root: "/project" }],
        },
      }],
      ["gui_invoke", {
        request: {
          method: "InitializeProjectV1",
          arguments: [{
            operationId: "operation-1",
            root: "/project",
            goal: "Ship safely",
            guideChoice: "install",
            guidePreviewToken: "preview-token",
          }],
        },
      }],
      ["gui_invoke", {
        request: { method: "GetInitializationStatusV1", arguments: ["operation-1"] },
      }],
      ["gui_invoke", {
        request: { method: "GetPendingInitializationV1", arguments: [] },
      }],
      ["gui_invoke", {
        request: { method: "CreateFirstPlanV1", arguments: [7, "First plan"] },
      }],
      ["gui_invoke", {
        request: { method: "CreateFirstTaskV1", arguments: [7, 11, "First task"] },
      }],
      ["gui_invoke", {
        request: {
          method: "StartFirstTaskV1",
          arguments: [7, 21, "2026-08-14T12:00:00Z"],
        },
      }],
      ["gui_invoke", {
        request: { method: "GetRecentProjectsV1", arguments: [] },
      }],
      ["gui_invoke", {
        request: {
          method: "ResolveRecentProjectV1",
          arguments: ["entry-a", "base-a", "/project"],
        },
      }],
      ["gui_invoke", {
        request: {
          method: "OpenRecentProjectV1",
          arguments: [
            "entry-a", "base-a", "/project", "relocation-token", "workspace-token",
          ],
        },
      }],
      ["gui_invoke", {
        request: {
          method: "ForgetRecentProjectV1",
          arguments: ["entry-a", "base-a"],
        },
      }],
      ["gui_invoke", { request: { method: "InstallShellCommand", arguments: [] } }],
      ["gui_invoke", { request: { method: "OpenHelpDestination", arguments: ["terminals"] } }],
      ["open_external_url", { url: "https://docs.example/help" }],
    ]);
  });

  it("routes the terminal window commands with their contract shapes", async () => {
    const calls = [];
    const results = {
      OpenTerminalWindow: { label: "terminal-1" },
      GetTerminalWindowTab: { sessions: ["session-a"], shape: { id: "tab-1" } },
      SetTerminalWindowTab: {},
      ClaimTerminalStream: { url: "ws://127.0.0.1:1/s", fromSequence: 42, gap: true },
    };
    const target = { __TAURI_INTERNALS__: {}, navigator: { clipboard: {} } };
    installTauriBridge(target, {
      invoke: async (command, payload) => {
        calls.push([command, payload]);
        return results[payload.request.method];
      },
      listen: vi.fn(),
      clipboard: { readText: vi.fn(), writeText: vi.fn() },
    });

    expect(await target.go.gui.App.OpenTerminalWindow(["session-a"], { id: "tab-1" })).toEqual({
      label: "terminal-1",
    });
    expect(await target.go.gui.App.GetTerminalWindowTab("terminal-1")).toEqual({
      sessions: ["session-a"],
      shape: { id: "tab-1" },
    });
    expect(
      await target.go.gui.App.SetTerminalWindowTab("terminal-1", ["session-a"], { id: "tab-1" }),
    ).toEqual({});
    expect(await target.go.gui.App.ClaimTerminalStream("session-a", 40)).toEqual({
      url: "ws://127.0.0.1:1/s",
      fromSequence: 42,
      gap: true,
    });
    // Pop-in is the window closing: there is no command for it, so nothing in
    // the bridge can leave a terminal window on screen with its session gone.
    expect(target.go.gui.App.CloseTerminalWindow).toBeUndefined();
    expect(calls).toEqual([
      ["gui_invoke", {
        request: { method: "OpenTerminalWindow", arguments: [["session-a"], { id: "tab-1" }] },
      }],
      ["gui_invoke", {
        request: { method: "GetTerminalWindowTab", arguments: ["terminal-1"] },
      }],
      ["gui_invoke", {
        request: {
          method: "SetTerminalWindowTab",
          arguments: ["terminal-1", ["session-a"], { id: "tab-1" }],
        },
      }],
      ["gui_invoke", {
        request: { method: "ClaimTerminalStream", arguments: ["session-a", 40] },
      }],
    ]);
  });

  it("unwraps event payloads and closes a late listener exactly once", async () => {
    let listener;
    let options;
    let resolveListen;
    const dispose = vi.fn();
    const listenPromise = new Promise((resolve) => { resolveListen = resolve; });
    const target = {
      __TAURI_INTERNALS__: { metadata: { currentWindow: { label: "terminal-1" } } },
      navigator: { clipboard: {} },
    };
    installTauriBridge(target, {
      invoke: vi.fn(),
      listen: (_name, callback, listenOptions) => {
        listener = callback;
        options = listenOptions;
        return listenPromise;
      },
      clipboard: { readText: vi.fn(), writeText: vi.fn() },
    });
    const callback = vi.fn();
    const unlisten = target.runtime.EventsOnMultiple("terminal:exit", callback, -1);
    // Scoped to this window's label, so a command targeted at the main window
    // does not also fire here. A default `Any` target would match every emit.
    expect(options).toEqual({ target: { kind: "AnyLabel", label: "terminal-1" } });
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

  /// Without metadata the label comes from the fragment the window was opened
  /// with. Defaulting to "main" subscribes a terminal window to the main
  /// window's events and to none of its own — its shell could exit and it would
  /// never hear about it.
  it("falls back to the window label in the fragment, never to main", () => {
    for (const [hash, label] of [
      ["#terminal-window=terminal-2", "terminal-2"],
      ["", "main"],
      ["#terminal-window=terminal-x", "main"],
    ]) {
      let options;
      const target = {
        __TAURI_INTERNALS__: {},
        location: { hash },
        navigator: { clipboard: {} },
      };
      installTauriBridge(target, {
        invoke: vi.fn(),
        listen: (_name, _callback, listenOptions) => {
          options = listenOptions;
          return new Promise(() => {});
        },
        clipboard: { readText: vi.fn(), writeText: vi.fn() },
      });
      target.runtime.EventsOnMultiple("terminal:exit", vi.fn(), -1);
      expect(options).toEqual({ target: { kind: "AnyLabel", label } });
    }
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
