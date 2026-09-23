import { describe, expect, it } from "vitest";

import {
  clampTerminalFontSize,
  defaultTerminalFontSize,
  maximumTerminalFontSize,
  minimumTerminalFontSize,
  readTerminalFontSize,
  readTerminalProfileFontSize,
  storedTerminalFontSize,
  terminalFontSizeStorageKey,
  terminalProfileFontSizeStorageKey,
  terminalZoomFontSize,
  terminalZoomLabel,
  writeTerminalFontSize,
  writeTerminalProfileFontSize,
} from "./preferences";

describe("terminal font preferences", () => {
  it("clamps stored and live sizes to readable bounds", () => {
    expect(clampTerminalFontSize(2)).toBe(minimumTerminalFontSize);
    expect(clampTerminalFontSize(17.4)).toBe(17);
    expect(clampTerminalFontSize(99)).toBe(maximumTerminalFontSize);
    expect(clampTerminalFontSize(Number.NaN)).toBe(defaultTerminalFontSize);
    expect(storedTerminalFontSize(null)).toBe(defaultTerminalFontSize);
    expect(storedTerminalFontSize("invalid")).toBe(defaultTerminalFontSize);
  });

  it("reads, writes, and labels persisted zoom", () => {
    const entries = new Map<string, string>();
    const storage = {
      getItem: (key: string) => entries.get(key) ?? null,
      setItem: (key: string, value: string) => entries.set(key, value),
    };

    expect(readTerminalFontSize(storage)).toBe(defaultTerminalFontSize);
    writeTerminalFontSize(storage, 18);
    expect(entries.get(terminalFontSizeStorageKey)).toBe("18");
    expect(readTerminalFontSize(storage)).toBe(18);
    expect(terminalZoomLabel(defaultTerminalFontSize)).toBe("100%");
    expect(terminalZoomLabel(21)).toBe("150%");
  });

  it("keeps defaults when storage is unavailable", () => {
    const storage = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    };

    expect(readTerminalFontSize(storage)).toBe(defaultTerminalFontSize);
    expect(() => writeTerminalFontSize(storage, 18)).not.toThrow();
  });

  it("persists zoom per stable profile and migrates the legacy preference", () => {
    const entries = new Map<string, string>([[terminalFontSizeStorageKey, "16"]]);
    const storage = {
      getItem: (key: string) => entries.get(key) ?? null,
      setItem: (key: string, value: string) => entries.set(key, value),
    };

    expect(readTerminalProfileFontSize(storage, "shell/default", 12)).toBe(16);
    writeTerminalProfileFontSize(storage, "shell/default", 19);
    expect(entries.get(terminalProfileFontSizeStorageKey("shell/default"))).toBe("19");
    expect(readTerminalProfileFontSize(storage, "shell/default", 12)).toBe(19);
    expect(readTerminalProfileFontSize(storage, "new-profile", 12)).toBe(16);
  });
});

describe("terminalZoomFontSize", () => {
  it("steps by one, resets to the profile default, and stays in bounds", () => {
    expect(terminalZoomFontSize("zoom-in", 14, 13)).toBe(15);
    expect(terminalZoomFontSize("zoom-out", 14, 13)).toBe(13);
    expect(terminalZoomFontSize("zoom-reset", 20, 13)).toBe(13);
    expect(terminalZoomFontSize("zoom-in", maximumTerminalFontSize, 13))
      .toBe(maximumTerminalFontSize);
    expect(terminalZoomFontSize("zoom-out", minimumTerminalFontSize, 13))
      .toBe(minimumTerminalFontSize);
  });
});
