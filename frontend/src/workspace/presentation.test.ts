import { describe, expect, it } from "vitest";

import {
  collapsedLaneStatuses,
  commandShortcut,
  confirmationCopy,
  focusCycleIndex,
  groupSearchResults,
  heatLevel,
  heatmapWeeks,
  paletteTarget,
  preserveSectionOnError,
  shortcutIntent,
  workspaceStateCopy,
} from "./presentation";

describe("workspace presentation policy", () => {
  it("defines distinct copy for every workspace state", () => {
    for (const state of ["welcome", "loading", "open", "error", "closed"] as const) {
      const copy = workspaceStateCopy(state, state === "error" ? "broken" : "");
      expect(copy.heading.length).toBeGreaterThan(0);
      expect(copy.detail.length).toBeGreaterThan(0);
    }
    expect(workspaceStateCopy("error", "broken").detail).toBe("broken");
  });

  it("describes only explicitly counted active resources", () => {
    expect(confirmationCopy("switch", 1, 2)).toEqual({
      heading: "Switch projects?",
      submit: "Switch project",
      detail: expect.stringContaining("1 active terminal and 2 registered agent runs"),
    });
  });

  it("describes pending resource operations separately from terminals", () => {
    expect(confirmationCopy("switch", 0, 0, 1).detail).toContain(
      "1 resource operation still finishing",
    );
    expect(confirmationCopy("switch", 0, 0, 1).detail).toContain(
      "0 active terminals",
    );
  });

  it("cycles focus in both directions", () => {
    expect(focusCycleIndex(3, 2, false)).toBe(0);
    expect(focusCycleIndex(3, 0, true)).toBe(2);
  });

  it("retains a successful section as stale when a partial refresh fails", () => {
    const previous = { state: "ready", snapshot: { branch: "main" } };
    expect(
      preserveSectionOnError(previous, { state: "error", error: "timed out" }),
    ).toEqual({
      state: "stale",
      snapshot: { branch: "main" },
      error: "timed out",
    });
  });

  it("suppresses shortcuts during composition, modifiers, and repeats", () => {
    expect(shortcutIntent({ key: "r" })).toBe("refresh");
    expect(shortcutIntent({ key: "/" })).toBe("addTask");
    expect(shortcutIntent({ key: "r", composing: true })).toBeNull();
    expect(shortcutIntent({ key: "/", ctrl: true })).toBeNull();
    expect(shortcutIntent({ key: "r", repeat: true })).toBeNull();
  });

  it("routes primary-modifier chords to commands", () => {
    expect(commandShortcut({ key: "k", meta: true })).toBe("palette");
    expect(commandShortcut({ key: "K", ctrl: true })).toBe("palette");
    expect(commandShortcut({ key: "1", meta: true })).toBe("board");
    expect(commandShortcut({ key: "2", meta: true })).toBe("overview");
    expect(commandShortcut({ key: "3", meta: true })).toBe("settings");
    expect(commandShortcut({ key: "n", meta: true })).toBe("addTask");
  });

  it("ignores command chords without a primary modifier or with extras", () => {
    expect(commandShortcut({ key: "k" })).toBeNull();
    expect(commandShortcut({ key: "1", shift: true })).toBeNull();
    expect(commandShortcut({ key: "k", meta: true, alt: true })).toBeNull();
    expect(commandShortcut({ key: "k", meta: true, repeat: true })).toBeNull();
    expect(commandShortcut({ key: "k", meta: true, prevented: true })).toBeNull();
    expect(commandShortcut({ key: "x", meta: true })).toBeNull();
  });

  it("groups palette results in plans, tasks, notes order", () => {
    const groups = groupSearchResults([
      { kind: "note", id: 9, planId: 0, title: "Task note", snippet: "…" },
      { kind: "task", id: 4, planId: 2, title: "Card", snippet: "" },
      { kind: "plan", id: 2, planId: 2, title: "Board", snippet: "" },
    ]);
    expect(groups.map((group) => group.label)).toEqual(["Plans", "Tasks", "Notes"]);
    expect(groups[0].items[0].title).toBe("Board");
    expect(groupSearchResults([])).toEqual([]);
  });

  it("maps palette results to their activation targets", () => {
    expect(
      paletteTarget({ kind: "plan", id: 3, planId: 3, title: "P", snippet: "" }),
    ).toEqual({ view: "board", planId: 3, taskId: 0 });
    expect(
      paletteTarget({ kind: "task", id: 7, planId: 3, title: "T", snippet: "" }),
    ).toEqual({ view: "board", planId: 3, taskId: 7 });
    expect(
      paletteTarget({ kind: "note", id: 1, planId: 3, title: "N", snippet: "" }),
    ).toEqual({ view: "overview", planId: 0, taskId: 0 });
  });

  it("collapses empty lanes unless re-expanded or all lanes are empty", () => {
    const lanes = [
      { status: "todo", taskCount: 2 },
      { status: "doing", taskCount: 0 },
      { status: "blocked", taskCount: 0 },
      { status: "done", taskCount: 5 },
    ];
    expect(collapsedLaneStatuses(lanes, new Set())).toEqual(["doing", "blocked"]);
    expect(collapsedLaneStatuses(lanes, new Set(["doing"]))).toEqual(["blocked"]);
    const allEmpty = lanes.map((lane) => ({ ...lane, taskCount: 0 }));
    expect(collapsedLaneStatuses(allEmpty, new Set())).toEqual([]);
    const allBusy = lanes.map((lane) => ({ ...lane, taskCount: 1 }));
    expect(collapsedLaneStatuses(allBusy, new Set())).toEqual([]);
    // Populated lanes fold only when the user collapsed them manually.
    expect(collapsedLaneStatuses(lanes, new Set(), new Set(["done"]))).toEqual([
      "doing",
      "blocked",
      "done",
    ]);
    expect(collapsedLaneStatuses(lanes, new Set(), new Set(["doing"]))).toEqual([
      "doing",
      "blocked",
    ]);
  });

  it("scales heat levels against the series maximum", () => {
    expect(heatLevel(0, 10)).toBe(0);
    expect(heatLevel(1, 10)).toBe(1);
    expect(heatLevel(5, 10)).toBe(2);
    expect(heatLevel(10, 10)).toBe(4);
    expect(heatLevel(3, 0)).toBe(0);
  });

  it("buckets days into Sunday-first week columns with padding", () => {
    // 2026-07-21 is a Tuesday, so the first column gets two padding cells.
    const days = [
      { date: "2026-07-21", count: 3 },
      { date: "2026-07-22", count: 0 },
      { date: "2026-07-23", count: 9 },
    ];
    const columns = heatmapWeeks(days);
    expect(columns).toHaveLength(1);
    expect(columns[0].map((cell) => cell.date)).toEqual([
      "",
      "",
      "2026-07-21",
      "2026-07-22",
      "2026-07-23",
    ]);
    expect(columns[0][2].level).toBe(2);
    expect(columns[0][4].level).toBe(4);
    expect(heatmapWeeks([])).toEqual([]);
  });
});
