import { describe, expect, it } from "vitest";

import {
  clampMenuPosition,
  deleteConfirmationText,
  planMenuItems,
  transferSubmitDisabled,
} from "./plan-lifecycle";

describe("plan lifecycle menu", () => {
  it("offers rename, move, copy, and destructive delete", () => {
    const items = planMenuItems();
    expect(items.map((item) => item.action)).toEqual(["rename", "move", "copy", "delete"]);
    expect(items.filter((item) => item.destructive).map((item) => item.action)).toEqual(["delete"]);
  });
});

describe("context menu position clamp", () => {
  const menu = { width: 140, height: 110 };
  const viewport = { width: 1280, height: 800 };
  it("leaves an in-bounds position alone", () => {
    expect(clampMenuPosition({ x: 300, y: 200 }, menu, viewport)).toEqual({ x: 300, y: 200 });
  });
  it("pulls a bottom-edge menu fully on screen", () => {
    expect(clampMenuPosition({ x: 110, y: 760 }, menu, viewport)).toEqual({ x: 110, y: 682 });
  });
  it("pulls a right-edge menu fully on screen", () => {
    expect(clampMenuPosition({ x: 1250, y: 200 }, menu, viewport)).toEqual({ x: 1132, y: 200 });
  });
  it("never clamps above the top-left margin", () => {
    expect(clampMenuPosition({ x: 0, y: 0 }, { width: 2000, height: 2000 }, viewport)).toEqual({
      x: 8,
      y: 8,
    });
  });
});

describe("delete confirmation text", () => {
  it("names counts, detached issues, and surviving commit records", () => {
    const text = deleteConfirmationText({
      planId: 3,
      title: "Doomed",
      tasks: 2,
      notes: 1,
      commits: 4,
      detachedIssues: [{ id: 7, title: "crash" }],
    });
    expect(text).toContain("2 tasks and 1 note");
    expect(text).toContain("1 linked issue will be detached");
    expect(text).toContain("4 commit records stay");
  });
});

describe("transfer submit gating", () => {
  const projects = [
    { name: "alpha", path: "/a", current: true },
    { name: "beta", path: "/b", current: false },
  ];
  it("move requires a non-current target", () => {
    expect(transferSubmitDisabled({ mode: "move", projects, targetPath: "", title: "" })).toBe(true);
    expect(transferSubmitDisabled({ mode: "move", projects, targetPath: "/a", title: "" })).toBe(true);
    expect(transferSubmitDisabled({ mode: "move", projects, targetPath: "/b", title: "" })).toBe(false);
  });
  it("copy into the current project requires a new title", () => {
    expect(transferSubmitDisabled({ mode: "copy", projects, targetPath: "", title: "" })).toBe(true);
    expect(transferSubmitDisabled({ mode: "copy", projects, targetPath: "/a", title: " " })).toBe(true);
    expect(transferSubmitDisabled({ mode: "copy", projects, targetPath: "", title: "Second" })).toBe(false);
    expect(transferSubmitDisabled({ mode: "copy", projects, targetPath: "/b", title: "" })).toBe(false);
  });
});
