import { describe, expect, it } from "vitest";

import {
  clampSidebarWidth,
  defaultLayoutState,
  defaultSidebarWidth,
  layoutProjectState,
  layoutStatePatch,
  minimumSidebarWidth,
  normalizeLayoutState,
  sidebarMaximumWidth,
  sidebarWidthFromKey,
  storedSidebarWidth,
} from "./layout";

describe("sidebar layout policy", () => {
  it("clamps pointer widths to useful and responsive bounds", () => {
    expect(clampSidebarWidth(80, 1_400)).toBe(minimumSidebarWidth);
    expect(clampSidebarWidth(360, 1_400)).toBe(360);
    expect(clampSidebarWidth(900, 1_400)).toBe(420);
    expect(clampSidebarWidth(420, 640)).toBe(288);
    expect(sidebarMaximumWidth(640)).toBe(288);
  });

  it("uses the default for missing or invalid persisted widths", () => {
    expect(storedSidebarWidth(null, 1_400)).toBe(defaultSidebarWidth);
    expect(storedSidebarWidth("not-a-number", 1_400)).toBe(defaultSidebarWidth);
    expect(storedSidebarWidth("350", 1_400)).toBe(350);
  });

  it("supports fine, coarse, and boundary keyboard resizing", () => {
    expect(sidebarWidthFromKey(248, "ArrowLeft", 1_400)).toBe(232);
    expect(sidebarWidthFromKey(248, "ArrowRight", 1_400)).toBe(264);
    expect(sidebarWidthFromKey(248, "PageDown", 1_400)).toBe(184);
    expect(sidebarWidthFromKey(248, "PageUp", 1_400)).toBe(312);
    expect(sidebarWidthFromKey(248, "Home", 1_400)).toBe(180);
    expect(sidebarWidthFromKey(248, "End", 1_400)).toBe(420);
    expect(sidebarWidthFromKey(248, "Escape", 1_400)).toBeNull();
  });
});

describe("stored layout record", () => {
  it("normalizes a full record and drops the backend eviction counter", () => {
    const state = normalizeLayoutState({
      storage: "ok",
      version: 1,
      sidebar: { width: 280.4, hidden: true },
      panels: { boardHidden: false, terminalHidden: true },
      projects: {
        "/work/app": {
          view: "overview",
          planId: 13,
          foldedLanes: ["done", "done", "todo"],
          usedAt: 7,
        },
      },
    });

    expect(state.storage).toBe("ok");
    expect(state.sidebar).toEqual({ width: 280, hidden: true });
    expect(state.panels).toEqual({ boardHidden: false, terminalHidden: true });
    expect(state.projects["/work/app"]).toEqual({
      view: "overview",
      planId: 13,
      foldedLanes: ["done", "todo"],
    });
    expect(state.projects["/work/app"]).not.toHaveProperty("usedAt");
  });

  it("falls back for unknown views, lanes, plans, and statuses", () => {
    const state = normalizeLayoutState({
      sidebar: { width: "wide", hidden: "yes" },
      panels: { boardHidden: 1 },
      projects: {
        "": { view: "board" },
        "/work/app": { view: "settings", planId: -3, foldedLanes: ["done", "nope", 7] },
      },
    });

    expect(state.storage).toBe("unreadable");
    expect(state.sidebar).toEqual({ width: defaultSidebarWidth, hidden: false });
    expect(state.panels).toEqual({ boardHidden: false, terminalHidden: false });
    expect(Object.keys(state.projects)).toEqual(["/work/app"]);
    expect(state.projects["/work/app"]).toEqual({
      view: "board",
      planId: 0,
      foldedLanes: ["done"],
    });
    expect(normalizeLayoutState(null).storage).toBe("unreadable");
    expect(defaultLayoutState().storage).toBe("defaults");
  });

  it("reads an unknown project root as the board default", () => {
    expect(layoutProjectState(defaultLayoutState(), "/missing")).toEqual({
      view: "board",
      planId: 0,
      foldedLanes: [],
    });
  });

  it("patches only the open project and never sends the eviction counter", () => {
    const state = normalizeLayoutState({
      storage: "ok",
      sidebar: { width: 300, hidden: false },
      panels: { boardHidden: true, terminalHidden: false },
      projects: {
        "/work/app": { view: "board", planId: 4, foldedLanes: [], usedAt: 9 },
        "/work/other": { view: "overview", planId: 2, foldedLanes: [], usedAt: 3 },
      },
    });

    const patch = layoutStatePatch(state, "/work/app");
    expect(patch).toEqual({
      sidebar: { width: 300, hidden: false },
      panels: { boardHidden: true, terminalHidden: false },
      projects: { "/work/app": { view: "board", planId: 4, foldedLanes: [] } },
    });
    expect(JSON.stringify(patch)).not.toContain("usedAt");
    expect(layoutStatePatch(state, "")).not.toHaveProperty("projects");
    expect(layoutStatePatch(state, "/unknown")).not.toHaveProperty("projects");
  });
});
