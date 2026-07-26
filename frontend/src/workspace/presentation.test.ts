import { describe, expect, it } from "vitest";

import {
  confirmationCopy,
  focusCycleIndex,
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
});
