import { describe, expect, it } from "vitest";

import {
  clampTerminalFontSize,
  defaultTerminalFontSize,
  maximumTerminalFontSize,
  minimumTerminalFontSize,
  readTerminalFontSize,
  storedTerminalFontSize,
  terminalFontSizeStorageKey,
  terminalZoomLabel,
  writeTerminalFontSize,
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
});
