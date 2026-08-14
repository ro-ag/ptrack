import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const COMMANDS = Object.freeze([
  "AcknowledgeAgentHandoffV2",
  "AddTask",
  "AddTaskNote",
  "AddTaskNoteV2",
  "AddTaskV2",
  "ApplyUpdate",
  "ApproveAgentWorkflowV2",
  "AssociateAgentRunV2",
  "AssociateTerminalV2",
  "CancelUpdateOperation",
  "CancelWorkspaceChange",
  "CheckForUpdates",
  "CloseProject",
  "CloseTerminal",
  "CloseTerminalV2",
  "CreateTerminal",
  "CreateTerminalV2",
  "DisableCapabilityV2",
  "DismissAgentWorkflowV2",
  "DownloadUpdate",
  "EnableCapabilityV2",
  "ExpireCapabilityV2",
  "GetActivityHeatmapV2",
  "GetAgentIntelligenceV2",
  "GetAgentRunsV2",
  "GetBoard",
  "GetBoardV2",
  "GetCapabilitiesV2",
  "GetCapabilityAuditsV2",
  "GetRecentProjects",
  "GetTaskDetailV2",
  "GetTerminalProfiles",
  "GetTerminalProfilesV2",
  "GetUpdateState",
  "GetWorkspaceSnapshot",
  "GetWorkspaceState",
  "InstallShellCommand",
  "LaunchLinkedAgentV2",
  "MoveTask",
  "MoveTaskV2",
  "MoveTaskV3",
  "MutateTerminalAssociationV2",
  "OpenHelpDestination",
  "OpenProject",
  "PickProjectDirectory",
  "PrepareAgentWorkflowV2",
  "PreviewAgentHandoffV2",
  "PreviewCapabilityV2",
  "PreviewTerminalWritebackV2",
  "RemoveCapabilityV2",
  "RenameTask",
  "RenameTaskV2",
  "ResizeTerminal",
  "ResizeTerminalV2",
  "RollbackLinkedAgentLaunchV2",
  "SaveCapabilityV2",
  "SearchV2",
  "SendAgentHandoffV2",
  "SetAgentTaskOwnershipV2",
  "SetAgentWorktreeV2",
  "SetAutomaticUpdateChecks",
  "TestCapabilityV2",
  "ValidateTerminalCWDsV2",
  "WriteTerminalMemoryV2",
]);

function installTauriBridge(target = globalThis, dependencies = {}) {
  if (!target.__TAURI_INTERNALS__) return false;
  const invokeCommand = dependencies.invoke || invoke;
  const listenEvent = dependencies.listen || listen;
  const clipboard = dependencies.clipboard || target.navigator?.clipboard;
  const normalizeError = (value) => {
    if (value instanceof Error) return value;
    if (typeof value === "string") return new Error(value);
    if (value && typeof value.message === "string") return new Error(value.message);
    return new Error(String(value));
  };
  const normalized = async (operation) => {
    try {
      return await operation();
    } catch (error) {
      throw normalizeError(error);
    }
  };

  const app = Object.fromEntries(
    COMMANDS.map((method) => [
      method,
      (...arguments_) => normalized(async () => {
        if (method === "PickProjectDirectory") {
          return invokeCommand("pick_project_directory");
        }
        const result = await invokeCommand("gui_invoke", {
          request: { method, arguments: arguments_ },
        });
        if (method === "InstallShellCommand") return undefined;
        if (method === "OpenHelpDestination" && typeof result === "string") {
          await invokeCommand("open_external_url", { url: result });
          return undefined;
        }
        return result;
      }),
    ]),
  );

  const eventsOnMultiple = (name, callback) => {
    let disposed = false;
    let unlisten = null;
    void listenEvent(name, (event) => {
      if (!disposed) callback(event.payload);
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  };

  target.go = { ...(target.go || {}), gui: { App: app } };
  target.runtime = {
    ...(target.runtime || {}),
    EventsOnMultiple: eventsOnMultiple,
    BrowserOpenURL: (url) => normalized(() => invokeCommand("open_external_url", { url })),
    ClipboardGetText: () => normalized(() => clipboard.readText()),
    ClipboardSetText: (text) => normalized(async () => {
      await clipboard.writeText(text);
      return true;
    }),
  };
  return true;
}

installTauriBridge();

export { COMMANDS, installTauriBridge };
