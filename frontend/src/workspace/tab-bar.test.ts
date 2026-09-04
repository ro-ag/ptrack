import { describe, expect, it } from "vitest";

import { maximumWorkspaceTabs, maximumTabTitleLength } from "./model";
import {
  activeTabActionState,
  canCreateWorkspaceTab,
  normalizedTabRename,
  tabControlPolicy,
  tabFocusIndex,
  tabIndicatorPresentation,
  tabMoveIndex,
  tabRenameKeyIntent,
  restoreConnectedFocus,
  structuralCloseFocusTarget,
  workspaceTabElementIds,
} from "./tab-bar";

const actionTabs = [
  {
    id: "tab-a",
    title: "A",
    activePaneId: "pane-a",
    root: { kind: "terminal" as const, paneId: "pane-a", profileId: "shell", cwd: "" },
  },
  {
    id: "tab-b",
    title: "B",
    activePaneId: "pane-b",
    root: { kind: "terminal" as const, paneId: "pane-b", profileId: "shell", cwd: "" },
  },
];

describe("tab-bar keyboard and focus policy", () => {
  it("pairs stable encoded tab and panel ids", () => {
    expect(workspaceTabElementIds("tab / one")).toEqual({
      tabButtonId: "terminal-tab-0074006100620020002f0020006f006e0065",
      panelId: "terminal-tab-panel-0074006100620020002f0020006f006e0065",
    });
    expect(workspaceTabElementIds("tab-b")).toEqual(workspaceTabElementIds("tab-b"));
    expect(() => workspaceTabElementIds("bad-\ud800-id")).not.toThrow();
    expect(workspaceTabElementIds("bad-\ud800-id").panelId).toContain("d800");
    expect(workspaceTabElementIds("\ud800")).not.toEqual(workspaceTabElementIds("\ud801"));
  });

  it("restores focus only while the invoker remains connected", () => {
    let calls = 0;
    expect(restoreConnectedFocus({ isConnected: true, focus: () => { calls += 1; } }))
      .toBe(true);
    expect(restoreConnectedFocus({ isConnected: false, focus: () => { calls += 1; } }))
      .toBe(false);
    expect(restoreConnectedFocus(null)).toBe(false);
    expect(calls).toBe(1);
  });

  it("focuses the surviving tab after tab close and pane content after pane close", () => {
    expect(structuralCloseFocusTarget("close-tab")).toBe("active-tab");
    expect(structuralCloseFocusTarget("close-pane")).toBe("active-pane");
  });

  it("wraps arrow focus and supports Home and End", () => {
    expect(tabFocusIndex("ArrowLeft", 0, 3)).toBe(2);
    expect(tabFocusIndex("ArrowRight", 2, 3)).toBe(0);
    expect(tabFocusIndex("ArrowLeft", 2, 3)).toBe(1);
    expect(tabFocusIndex("ArrowRight", 0, 3)).toBe(1);
    expect(tabFocusIndex("Home", 2, 3)).toBe(0);
    expect(tabFocusIndex("End", 0, 3)).toBe(2);
    expect(tabFocusIndex("PageDown", 0, 3)).toBeNull();
    expect(tabFocusIndex("ArrowRight", 0, 0)).toBeNull();
  });

  it("moves only within explicit reorder boundaries", () => {
    expect(tabMoveIndex(1, "left", 3)).toBe(0);
    expect(tabMoveIndex(1, "right", 3)).toBe(2);
    expect(tabMoveIndex(0, "left", 3)).toBeNull();
    expect(tabMoveIndex(2, "right", 3)).toBeNull();
    expect(tabMoveIndex(-1, "right", 3)).toBeNull();
  });
});

describe("tab-bar rename and control policy", () => {
  it("derives toolbar actions from only the active tab", () => {
    expect(activeTabActionState(actionTabs, "tab-b")).toEqual({
      tab: actionTabs[1],
      index: 1,
      controls: {
        moveLeftDisabled: false,
        moveRightDisabled: true,
        duplicateDisabled: false,
        closeDisabled: false,
      },
    });
    expect(activeTabActionState(actionTabs, "missing")).toBeNull();
  });

  it("maps rename keys and accepts only bounded nonblank titles", () => {
    expect(tabRenameKeyIntent("F2")).toBe("begin");
    expect(tabRenameKeyIntent("Enter")).toBe("commit");
    expect(tabRenameKeyIntent("Escape")).toBe("cancel");
    expect(tabRenameKeyIntent("Tab")).toBeNull();
    expect(normalizedTabRename("   ")).toBeNull();
    expect(normalizedTabRename("  Logs  ")).toBe("Logs");
    expect(normalizedTabRename("x".repeat(maximumTabTitleLength + 10))).toHaveLength(
      maximumTabTitleLength,
    );
  });

  it("uses distinct glyphs and accurate labels for every terminal indicator", () => {
    const presentations = [
      "failed", "completed", "exited", "activity",
      "opening", "running", "waiting", "closed",
    ].map((kind) => tabIndicatorPresentation(kind as Parameters<
      typeof tabIndicatorPresentation
    >[0]));
    expect(new Set(presentations.map(({ glyph }) => glyph)).size).toBe(8);
    expect(presentations.map(({ label }) => label)).toEqual([
      "failed",
      "completed",
      "exited",
      "new terminal activity",
      "opening",
      "running",
      "waiting for output",
      "not started",
    ]);
  });

  it("disables boundary, final-close, and maximum-tab actions", () => {
    expect(tabControlPolicy(0, 1)).toEqual({
      moveLeftDisabled: true,
      moveRightDisabled: true,
      duplicateDisabled: false,
      closeDisabled: true,
    });
    expect(tabControlPolicy(1, 3)).toMatchObject({
      moveLeftDisabled: false,
      moveRightDisabled: false,
      closeDisabled: false,
    });
    expect(tabControlPolicy(maximumWorkspaceTabs - 1, maximumWorkspaceTabs)).toEqual({
      moveLeftDisabled: false,
      moveRightDisabled: true,
      duplicateDisabled: true,
      closeDisabled: false,
    });
    expect(canCreateWorkspaceTab(maximumWorkspaceTabs - 1)).toBe(true);
    expect(canCreateWorkspaceTab(maximumWorkspaceTabs)).toBe(false);
    expect(canCreateWorkspaceTab(-1)).toBe(false);
  });
});
