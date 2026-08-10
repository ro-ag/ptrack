import { describe, expect, it } from "vitest";
import {
  appendPreservingIdentity,
  activeTabDockInteractionEligible,
  keyboardSplitRatio,
  leafRects,
  paneFocusShortcutIntent,
  paneInDirection,
  pointerSplitRatio,
  preferredWebglPaneIds,
  separatorAria,
  splitControlPolicy,
  splitControlsRestricted,
  splitDragOutcome,
  terminalPanePresentationPolicy,
} from "./split-view";
import type { PaneNode } from "./model";
import {
  acknowledgePaneActivity,
  recordExit,
  recordOutput,
  resetPaneActivity,
} from "../terminal/activity";

const nested: PaneNode = {
  kind: "split",
  splitId: "outer",
  direction: "horizontal",
  ratio: 0.6,
  first: { kind: "terminal", paneId: "a", profileId: "shell", cwd: "" },
  second: {
    kind: "split",
    splitId: "inner",
    direction: "vertical",
    ratio: 0.25,
    first: { kind: "terminal", paneId: "b", profileId: "agent", cwd: "/b" },
    second: { kind: "terminal", paneId: "c", profileId: "shell", cwd: "/c" },
  },
};

describe("split geometry", () => {
  it("computes exact recursive leaf rectangles", () => {
    expect(leafRects(nested, { x: 10, y: 20, width: 1000, height: 800 })).toEqual([
      { paneId: "a", x: 10, y: 20, width: 600, height: 800 },
      { paneId: "b", x: 610, y: 20, width: 400, height: 200 },
      { paneId: "c", x: 610, y: 220, width: 400, height: 600 },
    ]);
  });

  it("selects deterministic directional neighbors by overlap then distance", () => {
    const rects = leafRects(nested, { x: 0, y: 0, width: 1000, height: 800 });
    expect(paneInDirection(rects, "a", "right")).toBe("c");
    expect(paneInDirection(rects, "b", "down")).toBe("c");
    expect(paneInDirection(rects, "c", "left")).toBe("a");
    expect(paneInDirection(rects, "a", "left")).toBeNull();
  });

  it("requires the platform modifier chord for pane focus", () => {
    const event = {
      type: "keydown",
      key: "ArrowRight",
      altKey: true,
      ctrlKey: true,
      metaKey: false,
      repeat: false,
      isComposing: false,
    };
    expect(paneFocusShortcutIntent(event, false)).toEqual({
      direction: "right",
      focus: true,
    });
    expect(paneFocusShortcutIntent({ ...event, type: "keyup" }, false)).toEqual({
      direction: "right",
      focus: false,
    });
    expect(paneFocusShortcutIntent({ ...event, repeat: true }, false)).toEqual({
      direction: "right",
      focus: false,
    });
    expect(paneFocusShortcutIntent({ ...event, altKey: false }, false)).toBeNull();
    expect(paneFocusShortcutIntent({ ...event, isComposing: true }, false)).toBeNull();
    expect(paneFocusShortcutIntent({
      ...event,
      ctrlKey: false,
      metaKey: true,
      key: "ArrowUp",
    }, true)).toEqual({ direction: "up", focus: true });
    expect(paneFocusShortcutIntent({ ...event, key: "a" }, false)).toBeNull();
  });

  it("keeps multi-pane and sibling-runtime dock interactions available", () => {
    expect(activeTabDockInteractionEligible({
      paneCount: 2,
      hasResources: false,
      hasLiveRuntime: false,
    })).toBe(true);
    expect(activeTabDockInteractionEligible({
      paneCount: 1,
      hasResources: true,
      hasLiveRuntime: false,
    })).toBe(true);
    expect(activeTabDockInteractionEligible({
      paneCount: 1,
      hasResources: false,
      hasLiveRuntime: true,
    })).toBe(true);
    expect(activeTabDockInteractionEligible({
      paneCount: 1,
      hasResources: false,
      hasLiveRuntime: false,
    })).toBe(false);
  });
});

describe("split controls", () => {
  it("keeps split controls disabled for a detached linked-launch pane", () => {
    expect(splitControlsRestricted(true, false)).toBe(true);
    expect(splitControlsRestricted(false, true)).toBe(true);
    expect(splitControlsRestricted(false, false)).toBe(false);
  });

  it("enforces pane and depth caps", () => {
    expect(splitControlPolicy(nested, "b")).toEqual({
      canSplitRight: true,
      canSplitDown: true,
      canClose: true,
    });
    let deep: PaneNode = { kind: "terminal", paneId: "deep", profileId: "", cwd: "" };
    for (let depth = 1; depth < 6; depth += 1) {
      deep = {
        kind: "split",
        splitId: `s${depth}`,
        direction: "horizontal",
        ratio: 0.5,
        first: deep,
        second: { kind: "terminal", paneId: `p${depth}`, profileId: "", cwd: "" },
      };
    }
    expect(splitControlPolicy(deep, "deep").canSplitRight).toBe(false);
  });

  it("normalizes pointer and keyboard ratios", () => {
    expect(pointerSplitRatio("horizontal", { x: 100, y: 0, width: 400, height: 200 }, 300, 0))
      .toBe(0.5);
    expect(pointerSplitRatio("vertical", { x: 0, y: 10, width: 400, height: 200 }, 0, 190))
      .toBe(0.9);
    expect(keyboardSplitRatio("horizontal", 0.5, "ArrowRight")).toBe(0.52);
    expect(keyboardSplitRatio("horizontal", 0.5, "ArrowUp")).toBeNull();
    expect(keyboardSplitRatio("vertical", 0.5, "ArrowUp")).toBe(0.48);
    expect(keyboardSplitRatio("vertical", 0.5, "PageDown")).toBe(0.6);
    expect(keyboardSplitRatio("vertical", 0.5, "Home")).toBe(0.1);
    expect(keyboardSplitRatio("vertical", 0.5, "End")).toBe(0.9);
  });

  it("commits pointer up/lost capture and restores on cancellation", () => {
    expect(splitDragOutcome(0.5, 0.7, "pointerup")).toEqual({ ratio: 0.7, commit: true });
    expect(splitDragOutcome(0.5, 0.7, "lostpointercapture")).toEqual({ ratio: 0.7, commit: true });
    expect(splitDragOutcome(0.5, 0.7, "escape")).toEqual({ ratio: 0.5, commit: false });
    expect(splitDragOutcome(0.5, 0.7, "cancel")).toEqual({ ratio: 0.5, commit: false });
  });

  it("provides accurate separator ARIA", () => {
    expect(separatorAria("horizontal", 0.555)).toEqual({
      orientation: "vertical",
      valueMin: 10,
      valueMax: 90,
      valueNow: 56,
    });
    expect(separatorAria("vertical", 0.5).orientation).toBe("horizontal");
  });
});

describe("renderer resource policies", () => {
  it("prioritizes the selected visible pane within a bounded WebGL budget", () => {
    expect(preferredWebglPaneIds(nested, "c", new Set(["a", "b", "c"]), 2))
      .toEqual(["c", "a"]);
    expect(preferredWebglPaneIds(nested, "c", new Set(["a", "b"]), 4))
      .toEqual(["a", "b"]);
    expect(preferredWebglPaneIds(nested, "c", new Set(["a", "b", "c"]), 0))
      .toEqual([]);
  });

  it("reparents the same host object", () => {
    const host = {};
    const appended: object[] = [];
    const result = appendPreservingIdentity({ append: (node) => appended.push(node) }, host);
    expect(result).toBe(host);
    expect(appended).toEqual([host]);
  });

  it("treats a hidden app view as background and reveals only the selected pane", () => {
    const policy = (workspaceViewVisible: boolean, selected: boolean) =>
      terminalPanePresentationPolicy({
        workspaceViewVisible,
        terminalHidden: false,
        documentVisible: true,
        activeTab: true,
        selected,
        hasResources: true,
        hostVisible: workspaceViewVisible,
        bodyVisible: true,
        dockVisible: true,
      });
    const hiddenSelected = policy(false, true);
    expect(hiddenSelected).toMatchObject({
      paneVisible: false,
      foreground: false,
      webglAllowed: false,
    });
    expect(hiddenSelected.webglAllowed
      ? preferredWebglPaneIds(nested, "a", new Set(["a", "b", "c"]), 4)
      : []).toEqual([]);

    let selected = recordOutput(resetPaneActivity("agent"), hiddenSelected.foreground, 1);
    let sibling = recordExit(
      recordOutput(resetPaneActivity("shell"), false, 2),
      "shell",
      "exited",
      0,
      null,
      3,
    );
    expect(selected.unread).toBe(true);
    expect(sibling.unread).toBe(true);

    if (policy(true, true).foreground) selected = acknowledgePaneActivity(selected);
    if (policy(true, false).foreground) sibling = acknowledgePaneActivity(sibling);
    expect(selected.unread).toBe(false);
    expect(sibling).toMatchObject({ signal: "exited", unread: true });
  });
});
