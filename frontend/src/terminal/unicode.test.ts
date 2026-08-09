import { describe, expect, it } from "vitest";

import {
  modernUnicodeEnabled,
  modernUnicodeStorageKey,
  readModernUnicodeSetting,
  writeModernUnicodeSetting,
} from "./unicode";

describe("modern Unicode terminal setting", () => {
  it("defaults on and only disables for an explicit false value", () => {
    expect(modernUnicodeEnabled(null)).toBe(true);
    expect(modernUnicodeEnabled("true")).toBe(true);
    expect(modernUnicodeEnabled("invalid")).toBe(true);
    expect(modernUnicodeEnabled("false")).toBe(false);
  });

  it("reads and writes the persisted setting", () => {
    const entries = new Map<string, string>();
    const storage = {
      getItem: (key: string) => entries.get(key) ?? null,
      setItem: (key: string, value: string) => entries.set(key, value),
    };

    expect(readModernUnicodeSetting(storage)).toBe(true);
    writeModernUnicodeSetting(storage, false);
    expect(entries.get(modernUnicodeStorageKey)).toBe("false");
    expect(readModernUnicodeSetting(storage)).toBe(false);
  });

  it("keeps the default when storage is unavailable", () => {
    const storage = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    };

    expect(readModernUnicodeSetting(storage)).toBe(true);
    expect(() => writeModernUnicodeSetting(storage, false)).not.toThrow();
  });
});
