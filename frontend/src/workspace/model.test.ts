import { describe, expect, it } from "vitest";

import {
  createWorkspace,
  isWorkspace,
  maximumPaneDepth,
  maximumPanesPerTab,
  maximumCwdLength,
  maximumProfileIdLength,
  maximumWorkspaceIdLength,
  maximumWorkspaceTabs,
  normalizeSplitRatio,
  normalizeWorkspace,
  paneCount,
  paneDepth,
  type IdFactory,
  type WorkspaceIdKind,
} from "./model";

function sequentialIds(): IdFactory {
  let sequence = 0;
  return { next: (kind: WorkspaceIdKind) => `${kind}-${++sequence}` };
}

describe("workspace model", () => {
  it("creates the minimum valid descriptor workspace with injected ids", () => {
    const workspace = createWorkspace(sequentialIds(), {
      title: "Shell",
      profileId: "login",
      cwd: "/project",
    });

    expect(workspace).toEqual({
      version: 1,
      activeTabId: "tab-1",
      tabs: [{
        id: "tab-1",
        title: "Shell",
        activePaneId: "pane-2",
        root: {
          kind: "terminal",
          paneId: "pane-2",
          profileId: "login",
          cwd: "/project",
        },
      }],
    });
    expect(isWorkspace(workspace)).toBe(true);
  });

  it("normalizes malformed persisted input to bounded valid state", () => {
    const workspace = normalizeWorkspace({
      version: 99,
      activeTabId: "missing",
      tabs: [{
        id: "same",
        title: "   ",
        activePaneId: "missing",
        root: {
          kind: "split",
          splitId: "same",
          direction: "diagonal",
          ratio: Number.POSITIVE_INFINITY,
          first: { kind: "terminal", paneId: "same", profileId: 42, cwd: null },
          second: { kind: "terminal", paneId: "", profileId: "shell", cwd: "/tmp" },
        },
      }],
    }, sequentialIds(), { profileId: "fallback", cwd: "/project" });

    expect(isWorkspace(workspace)).toBe(true);
    expect(workspace.version).toBe(1);
    expect(workspace.activeTabId).toBe(workspace.tabs[0].id);
    expect(workspace.tabs[0].title).toBe("Terminal 1");
    expect(workspace.tabs[0].activePaneId).toBe("same");
    expect(workspace.tabs[0].root).toMatchObject({
      kind: "split",
      direction: "horizontal",
      ratio: 0.5,
    });
    expect(new Set([
      workspace.tabs[0].id,
      ...("first" in workspace.tabs[0].root
        ? [
            workspace.tabs[0].root.splitId,
            workspace.tabs[0].root.first.kind === "terminal"
              ? workspace.tabs[0].root.first.paneId
              : "",
            workspace.tabs[0].root.second.kind === "terminal"
              ? workspace.tabs[0].root.second.paneId
              : "",
          ]
        : []),
    ]).size).toBe(4);
  });

  it("clamps ratios and rejects invalid active references and duplicate ids", () => {
    expect(normalizeSplitRatio(-5)).toBe(0.1);
    expect(normalizeSplitRatio(7)).toBe(0.9);
    expect(normalizeSplitRatio(Number.NaN)).toBe(0.5);
    expect(normalizeSplitRatio(0.333_333)).toBe(0.333);

    const invalid = {
      version: 1,
      activeTabId: "tab",
      tabs: [{
        id: "tab",
        title: "Terminal",
        activePaneId: "elsewhere",
        root: { kind: "terminal", paneId: "tab", profileId: "", cwd: "" },
      }],
    };
    expect(isWorkspace(invalid)).toBe(false);
    expect(isWorkspace({
      ...createWorkspace(sequentialIds()),
      temporary: true,
    })).toBe(false);
  });

  it("allows only versioned authority-free tab association pointers", () => {
    const workspace = createWorkspace(sequentialIds());
    workspace.tabs[0].association = { version: 1, planId: 2, taskId: 9 };
    expect(isWorkspace(workspace)).toBe(true);

    for (const association of [
      { version: 2, planId: 2 },
      { version: 1, taskId: 9 },
      { version: 1, planId: 0 },
      { version: 1, planId: 2, taskId: Number.MAX_SAFE_INTEGER + 1 },
      { version: 1, planId: 2, generation: 7 },
      { version: 1, planId: 2, sessionId: "live" },
    ]) {
      expect(isWorkspace({
        ...workspace,
        tabs: [{ ...workspace.tabs[0], association }],
      })).toBe(false);
    }

    const normalized = normalizeWorkspace({
      ...workspace,
      tabs: [{ ...workspace.tabs[0], association: { version: 2, planId: 2 } }],
    }, sequentialIds());
    expect(normalized.tabs[0].association).toBeUndefined();
    expect(isWorkspace(normalized)).toBe(true);
  });

  it("enforces tab, pane, and depth limits during normalization", () => {
    const tabs = Array.from({ length: maximumWorkspaceTabs + 4 }, (_, index) => ({
      id: `tab-input-${index}`,
      title: `Tab ${index}`,
      activePaneId: `pane-input-${index}`,
      root: { kind: "terminal", paneId: `pane-input-${index}`, profileId: "", cwd: "" },
    }));
    const workspace = normalizeWorkspace({ version: 1, activeTabId: tabs[0].id, tabs }, sequentialIds());
    expect(workspace.tabs).toHaveLength(maximumWorkspaceTabs);

    let root: unknown = { kind: "terminal", paneId: "tail", profileId: "", cwd: "" };
    for (let depth = 0; depth < maximumPaneDepth + maximumPanesPerTab; depth += 1) {
      root = {
        kind: "split",
        splitId: `split-input-${depth}`,
        direction: "vertical",
        ratio: 0.5,
        first: { kind: "terminal", paneId: `pane-left-${depth}`, profileId: "", cwd: "" },
        second: root,
      };
    }
    const bounded = normalizeWorkspace({
      version: 1,
      activeTabId: "deep-tab",
      tabs: [{ id: "deep-tab", title: "Deep", activePaneId: "tail", root }],
    }, sequentialIds());
    expect(paneCount(bounded.tabs[0].root)).toBeLessThanOrEqual(maximumPanesPerTab);
    expect(paneDepth(bounded.tabs[0].root)).toBeLessThanOrEqual(maximumPaneDepth);
    expect(isWorkspace(bounded)).toBe(true);
  });

  it("falls back to one tab and pane when input has no usable tree", () => {
    const workspace = normalizeWorkspace({ tabs: [{ root: null }] }, sequentialIds());
    expect(workspace.tabs).toHaveLength(1);
    expect(paneCount(workspace.tabs[0].root)).toBe(1);
    expect(workspace.tabs[0].activePaneId).toBe(workspace.tabs[0].root.kind === "terminal"
      ? workspace.tabs[0].root.paneId
      : "");
    expect(isWorkspace(workspace)).toBe(true);
  });

  it("normalizes descriptor and identifier lengths to persistence bounds", () => {
    const created = createWorkspace(sequentialIds(), {
      profileId: "p".repeat(maximumProfileIdLength + 5),
      cwd: "c".repeat(maximumCwdLength + 5),
    });
    expect(created.tabs[0].root).toMatchObject({
      profileId: "p".repeat(maximumProfileIdLength),
      cwd: "c".repeat(maximumCwdLength),
    });
    expect(isWorkspace(created)).toBe(true);

    const normalized = normalizeWorkspace({
      version: 1,
      activeTabId: "t".repeat(maximumWorkspaceIdLength + 1),
      tabs: [{
        id: "t".repeat(maximumWorkspaceIdLength + 1),
        title: "Terminal",
        activePaneId: "p".repeat(maximumWorkspaceIdLength + 1),
        root: {
          kind: "terminal",
          paneId: "p".repeat(maximumWorkspaceIdLength + 1),
          profileId: "shell",
          cwd: "",
        },
      }],
    }, sequentialIds());
    expect(normalized.tabs[0].id.length).toBeLessThanOrEqual(maximumWorkspaceIdLength);
    expect(normalized.tabs[0].root.kind === "terminal"
      ? normalized.tabs[0].root.paneId.length
      : 0).toBeLessThanOrEqual(maximumWorkspaceIdLength);
    expect(isWorkspace(normalized)).toBe(true);
  });

  it("stops validating and normalizing at the depth boundary", () => {
    let root: unknown = {
      kind: "terminal", paneId: "tail", profileId: "shell", cwd: "",
    };
    for (let depth = 0; depth < 5_000; depth += 1) {
      root = {
        kind: "split",
        splitId: `deep-${depth}`,
        direction: "vertical",
        ratio: 0.5,
        first: root,
        second: {
          kind: "terminal", paneId: `leaf-${depth}`, profileId: "shell", cwd: "",
        },
      };
    }
    const malicious = {
      version: 1,
      activeTabId: "tab",
      tabs: [{ id: "tab", title: "Deep", activePaneId: "tail", root }],
    };
    expect(() => isWorkspace(malicious)).not.toThrow();
    expect(isWorkspace(malicious)).toBe(false);
    const normalized = normalizeWorkspace(malicious, sequentialIds());
    expect(isWorkspace(normalized)).toBe(true);
    expect(paneDepth(normalized.tabs[0].root)).toBeLessThanOrEqual(maximumPaneDepth);
  });
});
