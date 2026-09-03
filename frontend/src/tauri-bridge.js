import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { terminalWindowLabel } from "./terminal/pop-out";

const COMMANDS = Object.freeze([
  "AcknowledgeAgentHandoffV2",
  "AddIssueV1",
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
  "ClaimTerminalStream",
  "CloseProject",
  "CloseTerminal",
  "CloseTerminalV2",
  "CompletePlanV1",
  "CopyPlanV1",
  "CreateFirstPlanV1",
  "CreateFirstTaskV1",
  "CreateTerminal",
  "CreateTerminalV2",
  "DeletePlanV1",
  "DisableCapabilityV2",
  "DismissAgentWorkflowV2",
  "DownloadUpdate",
  "EnableCapabilityV2",
  "ExpireCapabilityV2",
  "ForgetRecentProjectV1",
  "GetActivityHeatmapV2",
  "GetAgentIntelligenceV2",
  "GetAgentRunsV2",
  "GetBoard",
  "GetBoardV2",
  "GetCapabilitiesV2",
  "GetCapabilityAuditsV2",
  "GetDiagnosticsReport",
  "GetInitializationStatusV1",
  "GetIssueDetailV1",
  "GetIssuesV1",
  "GetLayoutState",
  "GetPendingInitializationV1",
  "GetPreferences",
  "GetRecentProjects",
  "GetRecentProjectsV1",
  "GetRepoStatsV1",
  "GetTaskDetailV2",
  "GetTerminalProfiles",
  "GetTerminalProfilesV2",
  "GetTerminalWindowTab",
  "GetUpdateState",
  "GetWorkspaceSnapshot",
  "GetWorkspaceState",
  "HoldPlanV1",
  "InitializeProjectV1",
  "InstallShellCommand",
  "LaunchLinkedAgentV2",
  "ListProjectsV1",
  "MoveIssueTaskV1",
  "MoveTask",
  "MoveTaskV2",
  "MoveTaskV3",
  "MovePlanV1",
  "MutateTerminalAssociationV2",
  "OpenHelpDestination",
  "OpenProject",
  "OpenRecentProjectV1",
  "OpenTerminalWindow",
  "PickProjectDirectory",
  "PrepareAgentWorkflowV2",
  "PreviewAgentHandoffV2",
  "PreviewCapabilityV2",
  "PreviewProjectGuideV1",
  "PreviewTerminalWritebackV2",
  "RemoveCapabilityV2",
  "RenamePlanV1",
  "RenameTask",
  "RenameTaskV2",
  "ResetApplicationState",
  "ResetPreferences",
  "ResetWindowLayout",
  "ResizeTerminal",
  "ResizeTerminalV2",
  "ResolveRecentProjectV1",
  "ResumePlanV1",
  "RollbackLinkedAgentLaunchV2",
  "SaveCapabilityV2",
  "ScheduleIssueV1",
  "SearchV2",
  "SendAgentHandoffV2",
  "SetAgentTaskOwnershipV2",
  "SetAgentWorktreeV2",
  "SetAutomaticUpdateChecks",
  "SetIssueTaskV1",
  "SetLayoutState",
  "SetPreferences",
  "SetTerminalWindowTab",
  "StartFirstTaskV1",
  "TestCapabilityV2",
  "UpdateIssueV1",
  "ValidateProjectTargetV1",
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
          return invokeCommand("pick_project_directory", { purpose: arguments_[0] });
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

  // Every listener is scoped to the window it belongs to. A listener left on
  // the default `{ kind: "Any" }` target matches every emit unconditionally —
  // Tauri short-circuits the filter for it — so an event targeted at one window
  // would still fire in both. Broadcasts are unaffected: an unfiltered emit
  // reaches a labelled listener just the same.
  //
  // Missing metadata falls back to the label in the URL fragment, which is how
  // a terminal window is addressed in the first place: defaulting it to `main`
  // would subscribe that window to the main window's events and to none of its
  // own.
  const eventTarget = {
    kind: "AnyLabel",
    label: target.__TAURI_INTERNALS__.metadata?.currentWindow?.label ??
      terminalWindowLabel(target.location?.hash ?? "") ?? "main",
  };
  const eventsOnMultiple = (name, callback) => {
    let disposed = false;
    let unlisten = null;
    void listenEvent(name, (event) => {
      if (!disposed) callback(event.payload);
    }, { target: eventTarget }).then((dispose) => {
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
