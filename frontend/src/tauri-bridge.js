import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { terminalWindowLabel } from "./terminal/pop-out";

const COMMANDS = Object.freeze([
  "AcknowledgeAgentHandoffV2",
  "AddIssueV1",
  "AddPlanV1",
  "AddTaskNoteV2",
  "AddTaskV2",
  "ApplyUpdate",
  "ApproveAgentWorkflowV2",
  "CancelUpdateOperation",
  "CancelWorkspaceChange",
  "CheckForUpdates",
  "ClaimTerminalStream",
  "CloseProject",
  "CloseTerminalV2",
  "CompletePlanV1",
  "CopyPlanV1",
  "CreateFirstPlanV1",
  "CreateFirstTaskV1",
  "CreateTerminalV2",
  "DeletePlanV1",
  "DismissAgentWorkflowV2",
  "DownloadUpdate",
  "ForgetRecentProjectV1",
  "GetActivityHeatmapV2",
  "GetDiagnosticsReport",
  "GetGlobalOverviewV1",
  "GetInitializationStatusV1",
  "GetIssueDetailV1",
  "GetIssuesV1",
  "GetLayoutState",
  "GetPendingInitializationV1",
  "GetPreferences",
  "GetProjectTimelineV1",
  "GetRecentProjectsV1",
  "GetScratchpadV1",
  "GetStackProfileV1",
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
  "PreviewProjectGuideV1",
  "PreviewTerminalWritebackV2",
  "RefreshGlobalOverviewV1",
  "RenamePlanV1",
  "RenameTaskV2",
  "ReopenPlanV1",
  "ResetApplicationState",
  "ResetPreferences",
  "ResetWindowLayout",
  "ResizeTerminalV2",
  "ResolveRecentProjectV1",
  "ResumePlanV1",
  "RollbackLinkedAgentLaunchV2",
  "ScheduleIssueV1",
  "SearchV2",
  "SendAgentHandoffV2",
  "SetAgentTaskOwnershipV2",
  "SetAgentWorktreeV2",
  "SetAutomaticUpdateChecks",
  "SetIssueTaskV1",
  "SetLayoutState",
  "SetPreferences",
  "SetScratchpadV1",
  "SetTerminalWindowTab",
  "StartFirstTaskV1",
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
  // A structured runtime error may carry a recovery payload beside its message
  // (SetScratchpadV1 sends the stored record with `scratchpad revision
  // conflict`). Rebuilding the Error must not drop it, or the caller is forced
  // into a second round trip to learn what it was already told.
  const withStored = (error, value) => {
    if (value && typeof value === "object" && "stored" in value) {
      error.stored = value.stored;
    }
    return error;
  };
  const normalizeError = (value) => {
    if (value instanceof Error) return value;
    if (typeof value === "string") return new Error(value);
    if (value && typeof value.message === "string") {
      return withStored(new Error(value.message), value);
    }
    return withStored(new Error(String(value)), value);
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
  // A refused subscription must not vanish as an unhandled rejection: the
  // window would silently never hear the event. It is logged and announced as
  // `ptrack:event-subscription-failed` on the window, so the page can say so.
  const subscriptionFailed = (name, error) => {
    const message = normalizeError(error).message;
    target.console?.error?.(`p-track could not subscribe to ${name}: ${message}`);
    if (typeof target.dispatchEvent === "function" && typeof target.CustomEvent === "function") {
      target.dispatchEvent(new target.CustomEvent("ptrack:event-subscription-failed", {
        detail: { name, message },
      }));
    }
  };
  const eventsOnMultiple = (name, callback) => {
    let disposed = false;
    let unlisten = null;
    let subscription;
    try {
      subscription = Promise.resolve(listenEvent(name, (event) => {
        if (!disposed) callback(event.payload);
      }, { target: eventTarget }));
    } catch (error) {
      subscription = Promise.reject(error);
    }
    void subscription
      .then((dispose) => {
        if (disposed) dispose();
        else unlisten = dispose;
      })
      .catch((error) => subscriptionFailed(name, error));
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
