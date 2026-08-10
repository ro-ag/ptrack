import { describe, expect, it } from "vitest";

import {
  collectWorkspaceIds,
  createWorkspace,
  findSplitPane,
  findTerminalPane,
  isWorkspace,
  maximumPanesPerTab,
  maximumWorkspaceTabs,
  paneCount,
  type IdFactory,
  type PaneNode,
  type Workspace,
  type WorkspaceIdKind,
} from "./model";
import { reduceWorkspace } from "./reducer";

function sequentialIds(): IdFactory {
  let sequence = 0;
  return { next: (kind: WorkspaceIdKind) => `${kind}-${++sequence}` };
}

function reduce(
  workspace: Workspace,
  action: Parameters<typeof reduceWorkspace>[1],
  ids: IdFactory,
): Workspace {
  const next = reduceWorkspace(workspace, action, ids);
  expect(isWorkspace(next)).toBe(true);
  return next;
}

function shallowestPaneId(root: PaneNode): string {
  const queue = [root];
  while (queue.length > 0) {
    const node = queue.shift() as PaneNode;
    if (node.kind === "terminal") return node.paneId;
    queue.push(node.first, node.second);
  }
  throw new Error("pane tree is empty");
}

describe("workspace tab reducer", () => {
  it("creates, selects, renames, reorders, and closes tabs", () => {
    const ids = sequentialIds();
    let workspace = createWorkspace(ids, { title: "First" });
    const firstId = workspace.activeTabId;
    workspace = reduce(workspace, { type: "create-tab", title: "Second" }, ids);
    const secondId = workspace.activeTabId;
    expect(workspace.tabs.map((tab) => tab.title)).toEqual(["First", "Second"]);

    workspace = reduce(workspace, { type: "select-tab", tabId: firstId }, ids);
    workspace = reduce(workspace, {
      type: "rename-tab",
      tabId: firstId,
      title: "  Primary  ",
    }, ids);
    workspace = reduce(workspace, {
      type: "reorder-tab",
      tabId: firstId,
      toIndex: 99,
    }, ids);
    expect(workspace.tabs.map((tab) => tab.id)).toEqual([secondId, firstId]);
    expect(workspace.tabs[1].title).toBe("Primary");

    workspace = reduce(workspace, { type: "close-tab", tabId: firstId }, ids);
    expect(workspace.tabs).toHaveLength(1);
    expect(workspace.activeTabId).toBe(secondId);
    expect(reduceWorkspace(workspace, { type: "close-tab", tabId: secondId }, ids)).toBe(workspace);
  });

  it("creates a linked tab with only its authority-free association pointer", () => {
    const ids = sequentialIds();
    let workspace = createWorkspace(ids, { title: "First" });
    workspace = reduce(workspace, {
      type: "create-tab",
      title: "Task #9 · Codex",
      profileId: "agent-codex",
      cwd: "/repo",
      association: { version: 1, planId: 2, taskId: 9 },
    }, ids);
    expect(workspace.tabs[1]).toMatchObject({
      title: "Task #9 · Codex",
      association: { version: 1, planId: 2, taskId: 9 },
      root: { profileId: "agent-codex", cwd: "/repo" },
    });
    expect(Object.keys(workspace.tabs[1].association ?? {}).sort()).toEqual([
      "planId",
      "taskId",
      "version",
    ]);
  });

  it("relinks and detaches only the selected tab pointer", () => {
    const ids = sequentialIds();
    let workspace = createWorkspace(ids, {
      association: { version: 1, planId: 2, taskId: 9 },
    });
    const tabId = workspace.activeTabId;
    workspace = reduce(workspace, {
      type: "set-tab-association",
      tabId,
      association: { version: 1, planId: 2 },
    }, ids);
    expect(workspace.tabs[0].association).toEqual({ version: 1, planId: 2 });
    const relinked = workspace;
    workspace = reduce(workspace, {
      type: "set-tab-association",
      tabId,
    }, ids);
    expect(workspace.tabs[0].association).toBeUndefined();
    expect(workspace.tabs[0].root).toBe(relinked.tabs[0].root);
    expect(reduceWorkspace(workspace, {
      type: "set-tab-association",
      tabId: "missing",
      association: { version: 1, planId: 2 },
    }, ids)).toBe(workspace);
  });

  it("duplicates descriptors with fresh ids and preserves active-pane mapping", () => {
    const ids = sequentialIds();
    let workspace = createWorkspace(ids, { title: "Work", profileId: "shell", cwd: "/repo" });
    const tabId = workspace.activeTabId;
    const firstPaneId = workspace.tabs[0].activePaneId;
    workspace = reduce(workspace, {
      type: "split-pane",
      tabId,
      paneId: firstPaneId,
      direction: "vertical",
    }, ids);
    const source = workspace.tabs[0];
    source.association = { version: 1, planId: 2, taskId: 9 };
    const sourceIds = collectWorkspaceIds(workspace);

    workspace = reduce(workspace, { type: "duplicate-tab", tabId }, ids);
    const duplicate = workspace.tabs[1];
    const duplicateIds = collectWorkspaceIds({
      version: 1,
      activeTabId: duplicate.id,
      tabs: [duplicate],
    });
    expect([...duplicateIds].every((id) => !sourceIds.has(id))).toBe(true);
    expect(duplicate.title).toBe("Work copy");
    expect(findTerminalPane(duplicate.root, duplicate.activePaneId)).not.toBeNull();
    expect(paneCount(duplicate.root)).toBe(paneCount(source.root));
    expect(duplicate.association).toBeUndefined();
  });

  it("keeps invalid tab actions as safe no-ops and caps tab count", () => {
    const ids = sequentialIds();
    let workspace = createWorkspace(ids);
    expect(reduceWorkspace(workspace, { type: "select-tab", tabId: "missing" }, ids)).toBe(workspace);
    expect(reduceWorkspace(workspace, {
      type: "rename-tab",
      tabId: workspace.activeTabId,
      title: "   ",
    }, ids)).toBe(workspace);
    expect(reduceWorkspace(workspace, {
      type: "reorder-tab",
      tabId: workspace.activeTabId,
      toIndex: Number.NaN,
    }, ids)).toBe(workspace);

    while (workspace.tabs.length < maximumWorkspaceTabs) {
      workspace = reduce(workspace, { type: "create-tab" }, ids);
    }
    expect(reduceWorkspace(workspace, { type: "create-tab" }, ids)).toBe(workspace);
  });
});

describe("workspace pane reducer", () => {
  it("does not split a linked tab", () => {
    const ids = sequentialIds();
    const workspace = createWorkspace(ids, {
      profileId: "agent",
      cwd: "/repo",
      association: { version: 1, planId: 3, taskId: 7 },
    });
    const tab = workspace.tabs[0];
    const next = reduce(workspace, {
      type: "split-pane",
      tabId: tab.id,
      paneId: tab.activePaneId,
      direction: "horizontal",
    }, ids);

    expect(next).toBe(workspace);
    expect(next.tabs[0].root.kind).toBe("terminal");
    expect(next.tabs[0].association).toEqual({
      version: 1,
      planId: 3,
      taskId: 7,
    });
  });

  it("splits, focuses, updates, resizes, and closes panes", () => {
    const ids = sequentialIds();
    let workspace = createWorkspace(ids, { profileId: "shell", cwd: "/repo" });
    const tabId = workspace.activeTabId;
    const originalPaneId = workspace.tabs[0].activePaneId;
    workspace = reduce(workspace, {
      type: "split-pane",
      tabId,
      paneId: originalPaneId,
      direction: "horizontal",
      ratio: 4,
      profileId: "agent",
    }, ids);
    const split = workspace.tabs[0].root;
    expect(split.kind).toBe("split");
    if (split.kind !== "split") throw new Error("expected split");
    const newPaneId = workspace.tabs[0].activePaneId;
    expect(split.ratio).toBe(0.9);
    expect(findTerminalPane(split, newPaneId)).toMatchObject({
      profileId: "agent",
      cwd: "/repo",
    });

    workspace = reduce(workspace, {
      type: "update-pane",
      tabId,
      paneId: originalPaneId,
      changes: { cwd: "/other" },
    }, ids);
    expect(findTerminalPane(workspace.tabs[0].root, originalPaneId)?.cwd).toBe("/other");
    workspace = reduce(workspace, {
      type: "focus-pane",
      tabId,
      paneId: originalPaneId,
    }, ids);
    expect(workspace.tabs[0].activePaneId).toBe(originalPaneId);
    workspace = reduce(workspace, {
      type: "resize-split",
      tabId,
      splitId: split.splitId,
      ratio: Number.NaN,
    }, ids);
    expect(findSplitPane(workspace.tabs[0].root, split.splitId)?.ratio).toBe(0.5);

    workspace = reduce(workspace, { type: "close-pane", tabId, paneId: originalPaneId }, ids);
    expect(paneCount(workspace.tabs[0].root)).toBe(1);
    expect(workspace.tabs[0].activePaneId).toBe(newPaneId);
    expect(reduceWorkspace(workspace, {
      type: "close-pane",
      tabId,
      paneId: newPaneId,
    }, ids)).toBe(workspace);
  });

  it("caps pane growth and leaves unknown pane and split ids unchanged", () => {
    const ids = sequentialIds();
    let workspace = createWorkspace(ids);
    const tabId = workspace.activeTabId;
    while (paneCount(workspace.tabs[0].root) < maximumPanesPerTab) {
      workspace = reduce(workspace, {
        type: "split-pane",
        tabId,
        paneId: shallowestPaneId(workspace.tabs[0].root),
        direction: "vertical",
      }, ids);
    }
    const full = workspace;
    workspace = reduceWorkspace(workspace, {
      type: "split-pane",
      tabId,
      paneId: workspace.tabs[0].activePaneId,
      direction: "vertical",
    }, ids);
    expect(workspace).toBe(full);
    expect(reduceWorkspace(workspace, {
      type: "focus-pane",
      tabId,
      paneId: "missing",
    }, ids)).toBe(workspace);
    expect(reduceWorkspace(workspace, {
      type: "resize-split",
      tabId,
      splitId: "missing",
      ratio: 0.4,
    }, ids)).toBe(workspace);
  });

  it("uses the nearest surviving sibling as nested active-pane fallback", () => {
    const ids = sequentialIds();
    let workspace = createWorkspace(ids, { profileId: "shell" });
    const tabId = workspace.activeTabId;
    const first = workspace.tabs[0].activePaneId;
    workspace = reduce(workspace, {
      type: "split-pane",
      tabId,
      paneId: first,
      direction: "horizontal",
    }, ids);
    const second = workspace.tabs[0].activePaneId;
    workspace = reduce(workspace, {
      type: "split-pane",
      tabId,
      paneId: second,
      direction: "vertical",
    }, ids);
    const third = workspace.tabs[0].activePaneId;
    workspace = reduce(workspace, { type: "focus-pane", tabId, paneId: second }, ids);
    workspace = reduce(workspace, { type: "close-pane", tabId, paneId: second }, ids);
    expect(workspace.tabs[0].activePaneId).toBe(third);
    workspace = reduce(workspace, { type: "close-pane", tabId, paneId: third }, ids);
    expect(workspace.tabs[0].activePaneId).toBe(first);
    expect(workspace.tabs[0].root).toMatchObject({ kind: "terminal", paneId: first });
  });

  it("rejects duplicate or empty ids from the factory without corrupting state", () => {
    const initialIds = sequentialIds();
    const workspace = createWorkspace(initialIds);
    const brokenIds: IdFactory = { next: () => workspace.activeTabId };
    const next = reduceWorkspace(workspace, { type: "create-tab" }, brokenIds);
    expect(next).toBe(workspace);
    expect(isWorkspace(next)).toBe(true);
  });

  it("normalizes malformed input before safely handling an unknown action", () => {
    const ids = sequentialIds();
    const malformed = { version: 1, activeTabId: "", tabs: [] } as unknown as Workspace;
    const next = reduceWorkspace(malformed, { type: "unknown" } as never, ids);
    expect(isWorkspace(next)).toBe(true);
    expect(next.tabs).toHaveLength(1);
    expect(paneCount(next.tabs[0].root)).toBe(1);
  });
});
