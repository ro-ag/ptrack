import { describe, expect, it } from "vitest";

import { createWorkspace, type IdFactory, type WorkspaceIdKind } from "../workspace/model";
import { duplicateTab, splitPane, updatePane } from "../workspace/reducer";
import {
  activeTerminalDescriptor,
  earlyExitCacheLimit,
  ensureStoppedWorkspaceRuntimes,
  paneRuntimeEventAccepted,
  paneRuntimeTransition,
  PaneRuntimeRegistry,
  runtimeBlocksDescriptorClose,
  runtimeDescriptorEditable,
} from "./runtime";

function sequentialIds(): IdFactory {
  let sequence = 0;
  return { next: (kind: WorkspaceIdKind) => `${kind}-${++sequence}` };
}

describe("PaneRuntimeRegistry", () => {
  it("treats transport closure as failure while authoritative exits win races", () => {
    expect(paneRuntimeTransition("running", { kind: "stream-closed" })).toEqual({
      state: "failed",
      detail: "Terminal stream disconnected",
    });
    expect(paneRuntimeTransition("failed", {
      kind: "process-exit",
      failed: false,
      detail: "Process exited with code 0",
    })).toEqual({ state: "exited", detail: "Process exited with code 0" });
    expect(paneRuntimeTransition("exited", { kind: "stream-closed" })).toBeNull();
    expect(paneRuntimeTransition("failed", { kind: "stream-closed" })).toBeNull();
    expect(paneRuntimeTransition("opening", { kind: "stream-open" })).toEqual({
      state: "running",
      detail: "",
    });
    expect(paneRuntimeTransition("closed", { kind: "stream-error" })).toBeNull();
  });

  it("rejects stale, mismatched, closing, restarted, and disposed lifecycle events", () => {
    const current = {
      ticketAccepted: true,
      closing: false,
      sessionId: "session-a",
      eventSessionId: "session-a",
    };
    expect(paneRuntimeEventAccepted(current)).toBe(true);
    expect(paneRuntimeEventAccepted({ ...current, ticketAccepted: false })).toBe(false);
    expect(paneRuntimeEventAccepted({ ...current, closing: true })).toBe(false);
    expect(paneRuntimeEventAccepted({ ...current, eventSessionId: "session-old" })).toBe(false);
    expect(paneRuntimeEventAccepted({
      ticketAccepted: false,
      closing: false,
      sessionId: null,
    })).toBe(false);
  });

  it("bounds early exits to current openings and the workspace pane limit", () => {
    expect(earlyExitCacheLimit(0, 96)).toBe(0);
    expect(earlyExitCacheLimit(3, 96)).toBe(3);
    expect(earlyExitCacheLimit(200, 96)).toBe(96);
    expect(earlyExitCacheLimit(-1, 96)).toBe(0);
    expect(earlyExitCacheLimit(Number.NaN, 96)).toBe(0);
  });

  it("keeps tickets, busy state, and acceptance independent per pane", async () => {
    const registry = new PaneRuntimeRegistry<{ sessionId: string }, { disposed: boolean }>();
    const first = registry.ensure("pane-a");
    const second = registry.ensure("pane-b");
    first.busy = true;
    second.state = "opening";
    const firstTicket = registry.begin(first.paneId);
    const secondTicket = registry.begin(second.paneId);

    await Promise.resolve();
    registry.invalidate(first.paneId);
    expect(registry.accepts(firstTicket)).toBe(false);
    expect(registry.accepts(secondTicket)).toBe(true);
    expect(registry.get("pane-a")?.busy).toBe(true);
    expect(registry.get("pane-b")?.state).toBe("opening");
  });

  it("retains titles independently and exposes descriptor lifecycle policies", () => {
    const registry = new PaneRuntimeRegistry<{ sessionId: string }, object>();
    const first = registry.ensure("pane-a");
    const second = registry.ensure("pane-b");
    first.title = "Build logs";
    first.activity = {
      profileKind: "agent",
      signal: "activity",
      unread: true,
      lastSignalAt: 10,
      exitCode: null,
    };
    second.title = "Tests";
    expect(registry.get("pane-a")?.title).toBe("Build logs");
    expect(registry.get("pane-b")?.title).toBe("Tests");
    expect(registry.get("pane-a")?.activity).toMatchObject({
      profileKind: "agent", signal: "activity", unread: true,
    });
    expect(registry.get("pane-b")?.activity).toMatchObject({
      profileKind: null, signal: "none", unread: false,
    });

    expect(runtimeDescriptorEditable(first)).toBe(true);
    expect(runtimeBlocksDescriptorClose(first)).toBe(false);
    first.state = "opening";
    expect(runtimeDescriptorEditable(first)).toBe(false);
    expect(runtimeBlocksDescriptorClose(first)).toBe(true);
    first.state = "running";
    expect(runtimeDescriptorEditable(first)).toBe(false);
    expect(runtimeBlocksDescriptorClose(first)).toBe(true);
    first.state = "exited";
    first.session = { sessionId: "finished" };
    expect(runtimeDescriptorEditable(first)).toBe(true);
    expect(runtimeBlocksDescriptorClose(first)).toBe(true);
    first.state = "failed";
    expect(runtimeDescriptorEditable(first)).toBe(false);
    first.session = null;
    expect(runtimeDescriptorEditable(first)).toBe(true);
    first.busy = true;
    expect(runtimeDescriptorEditable(first)).toBe(false);
    expect(runtimeBlocksDescriptorClose(first)).toBe(true);
    first.busy = false;
    first.closing = true;
    expect(runtimeDescriptorEditable(first)).toBe(false);
    expect(runtimeBlocksDescriptorClose(first)).toBe(true);
  });

  it("accepts concurrent completions by pane and drops a stale completion", async () => {
    const registry = new PaneRuntimeRegistry<{ sessionId: string }, object>();
    const first = registry.ensure("pane-a");
    const second = registry.ensure("pane-b");
    const firstTicket = registry.begin(first.paneId);
    const secondTicket = registry.begin(second.paneId);
    let finishFirst!: () => void;
    let finishSecond!: () => void;
    const firstGate = new Promise<void>((resolve) => { finishFirst = resolve; });
    const secondGate = new Promise<void>((resolve) => { finishSecond = resolve; });
    const complete = async (
      paneId: string,
      ticket: typeof firstTicket,
      gate: Promise<void>,
    ): Promise<void> => {
      await gate;
      const runtime = registry.runtimeFor(ticket);
      if (runtime) runtime.session = { sessionId: `session-${paneId}` };
    };
    const firstCompletion = complete(first.paneId, firstTicket, firstGate);
    const secondCompletion = complete(second.paneId, secondTicket, secondGate);

    finishSecond();
    await secondCompletion;
    expect(second.session?.sessionId).toBe("session-pane-b");
    expect(first.session).toBeNull();
    registry.invalidate(first.paneId);
    finishFirst();
    await firstCompletion;
    expect(first.session).toBeNull();
  });

  it("looks up sessions and invalidates removed runtimes", () => {
    const registry = new PaneRuntimeRegistry<{ sessionId: string }, object>();
    const runtime = registry.ensure("pane-a");
    const ticket = registry.begin(runtime.paneId);
    runtime.session = { sessionId: "session-a" };

    expect(registry.findBySessionId("session-a")).toBe(runtime);
    expect(registry.findBySessionId("missing")).toBeNull();
    expect(registry.values()).toEqual([runtime]);
    expect(registry.remove(runtime.paneId)).toBe(runtime);
    expect(registry.accepts(ticket)).toBe(false);
    expect(registry.get(runtime.paneId)).toBeNull();
  });

  it("returns the active descriptor without adding runtime data to workspace state", () => {
    const ids = sequentialIds();
    let workspace = createWorkspace(ids, { profileId: "shell", cwd: "/repo" });
    const original = activeTerminalDescriptor(workspace);
    workspace = splitPane(
      workspace,
      ids,
      workspace.activeTabId,
      workspace.tabs[0].activePaneId,
      "vertical",
      { profileId: "agent", cwd: "" },
    );

    expect(original?.pane).toMatchObject({ profileId: "shell", cwd: "/repo" });
    expect(activeTerminalDescriptor(workspace)?.pane).toMatchObject({
      profileId: "agent",
      cwd: "",
    });
    expect(Object.keys(activeTerminalDescriptor(workspace)?.pane ?? {}).sort()).toEqual([
      "cwd",
      "kind",
      "paneId",
      "profileId",
    ]);
  });

  it("keeps duplicated profile and cwd descriptors separate from runtimes", () => {
    const ids = sequentialIds();
    let workspace = createWorkspace(ids, { profileId: "shell", cwd: "/repo" });
    const original = activeTerminalDescriptor(workspace)!;
    const registry = new PaneRuntimeRegistry();
    registry.ensure(original.pane.paneId).state = "running";
    workspace = duplicateTab(workspace, ids, original.tabId);
    const duplicate = activeTerminalDescriptor(workspace)!;

    expect(duplicate.pane.paneId).not.toBe(original.pane.paneId);
    expect(duplicate.pane).toMatchObject({ profileId: "shell", cwd: "/repo" });
    expect(registry.get(duplicate.pane.paneId)).toBeNull();
    workspace = updatePane(workspace, duplicate.tabId, duplicate.pane.paneId, {
      profileId: "agent",
      cwd: "",
    });
    expect(activeTerminalDescriptor(workspace)?.pane).toMatchObject({
      profileId: "agent",
      cwd: "",
    });
    expect(runtimeBlocksDescriptorClose(registry.get(original.pane.paneId))).toBe(true);
  });

  it("restores descriptors only as stopped runtimes without lifecycle callbacks", () => {
    const factory = sequentialIds();
    let workspace = createWorkspace(factory, { profileId: "shell", cwd: "/repo" });
    workspace = splitPane(
      workspace,
      factory,
      workspace.activeTabId,
      workspace.tabs[0].activePaneId,
      "horizontal",
      { profileId: "agent", cwd: "/agent" },
    );
    const registry = new PaneRuntimeRegistry<{ sessionId: string }, object>();
    ensureStoppedWorkspaceRuntimes(registry, workspace);
    expect(registry.values()).toHaveLength(2);
    expect(registry.values().every((runtime) =>
      runtime.state === "closed" && runtime.session === null && runtime.resources === null
    )).toBe(true);
  });
});
