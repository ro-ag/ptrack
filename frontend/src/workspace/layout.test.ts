import { describe, expect, it } from "vitest";

import {
  clampSidebarWidth,
  defaultSidebarWidth,
  minimumSidebarWidth,
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
