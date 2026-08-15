import { describe, expect, it, vi } from "vitest";

import { COMMANDS, installTauriBridge } from "./tauri-bridge";

describe("Tauri compatibility bridge", () => {
  it("pins the exact current command allowlist", () => {
    expect(COMMANDS).toEqual([
      "AcknowledgeAgentHandoffV2", "AddTask", "AddTaskNote", "AddTaskNoteV2",
      "AddTaskV2", "ApplyUpdate", "ApproveAgentWorkflowV2", "AssociateAgentRunV2",
      "AssociateTerminalV2", "CancelUpdateOperation", "CancelWorkspaceChange",
      "CheckForUpdates", "CloseProject", "CloseTerminal", "CloseTerminalV2",
      "CreateFirstPlanV1", "CreateFirstTaskV1", "CreateTerminal", "CreateTerminalV2", "DisableCapabilityV2",
      "DismissAgentWorkflowV2", "DownloadUpdate", "EnableCapabilityV2",
      "ExpireCapabilityV2", "ForgetRecentProjectV1", "GetActivityHeatmapV2", "GetAgentIntelligenceV2",
      "GetAgentRunsV2", "GetBoard", "GetBoardV2", "GetCapabilitiesV2",
      "GetCapabilityAuditsV2", "GetDiagnosticsReport", "GetInitializationStatusV1",
      "GetLayoutState", "GetPendingInitializationV1",
      "GetPreferences", "GetRecentProjects", "GetRecentProjectsV1", "GetTaskDetailV2",
      "GetTerminalProfiles", "GetTerminalProfilesV2", "GetUpdateState",
      "GetWorkspaceSnapshot", "GetWorkspaceState", "InitializeProjectV1", "InstallShellCommand",
      "LaunchLinkedAgentV2", "MoveTask", "MoveTaskV2", "MoveTaskV3",
      "MutateTerminalAssociationV2", "OpenHelpDestination", "OpenProject", "OpenRecentProjectV1",
      "PickProjectDirectory", "PrepareAgentWorkflowV2", "PreviewAgentHandoffV2",
      "PreviewCapabilityV2", "PreviewProjectGuideV1", "PreviewTerminalWritebackV2", "RemoveCapabilityV2",
      "RenameTask", "RenameTaskV2", "ResetApplicationState", "ResetPreferences", "ResetWindowLayout",
      "ResizeTerminal", "ResizeTerminalV2", "ResolveRecentProjectV1",
      "RollbackLinkedAgentLaunchV2", "SaveCapabilityV2", "SearchV2",
      "SendAgentHandoffV2", "SetAgentTaskOwnershipV2", "SetAgentWorktreeV2",
      "SetAutomaticUpdateChecks", "SetLayoutState", "SetPreferences", "StartFirstTaskV1",
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
