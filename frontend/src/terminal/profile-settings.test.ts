import { describe, expect, it } from "vitest";

import {
  defaultTerminalProfileSettings,
  normalizeTerminalProfileSettings,
  terminalProfileClosesAfterExit,
  terminalProfileFontFamily,
  terminalProfileTheme,
  terminalRendererOptions,
} from "./profile-settings";

describe("terminal profile settings", () => {
  it("normalizes missing and untrusted renderer settings to bounded defaults", () => {
    expect(normalizeTerminalProfileSettings({})).toEqual(defaultTerminalProfileSettings);
    expect(normalizeTerminalProfileSettings({
      fontSize: 99,
      scrollback: -1,
      cwdPolicy: "unknown" as any,
      exitBehavior: "restart" as any,
    })).toMatchObject({
      fontSize: 24,
      scrollback: 100,
      cwdPolicy: "requested",
      exitBehavior: "keep",
    });
  });

  it("uses named themes and falls back without sharing mutable objects", () => {
    expect(terminalProfileTheme("platinum").background).toBe("#f6f7f9");
    expect(terminalProfileTheme("missing").background).toBe("#090d12");
    const first = terminalProfileTheme("default");
    first.background = "changed";
    expect(terminalProfileTheme("default").background).toBe("#090d12");
  });

  it("maps the portable font sentinel and explicit exit behavior", () => {
    expect(terminalProfileFontFamily("monospace")).toContain("Apple Color Emoji");
    expect(terminalProfileFontFamily("Iosevka")).toBe("Iosevka");
    expect(terminalProfileClosesAfterExit("keep", 0)).toBe(false);
    expect(terminalProfileClosesAfterExit("close-on-success", 1)).toBe(false);
    expect(terminalProfileClosesAfterExit("close-on-success", 0)).toBe(true);
    expect(terminalProfileClosesAfterExit("close", 1)).toBe(true);
  });

  it.each([100, 50_000, 100_000])(
    "preserves normalized scrollback %i in fresh renderer options",
    (scrollback) => {
      const settings = normalizeTerminalProfileSettings({ scrollback });
      expect(terminalRendererOptions(settings, 16)).toMatchObject({
        fontSize: 16,
        minimumContrastRatio: 4.5,
        screenReaderMode: true,
        scrollback,
      });
    },
  );
});
