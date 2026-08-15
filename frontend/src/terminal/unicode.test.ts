import { describe, expect, it } from "vitest";

import {
  modernUnicodeEnabled,
  modernUnicodeStorageKey,
  readModernUnicodeSetting,
} from "./unicode";

describe("modern Unicode terminal setting", () => {
  it("defaults on and only disables for an explicit false value", () => {
    expect(modernUnicodeEnabled(null)).toBe(true);
    expect(modernUnicodeEnabled("true")).toBe(true);
    expect(modernUnicodeEnabled("invalid")).toBe(true);
    expect(modernUnicodeEnabled("false")).toBe(false);
  });

  it("reads the mirror the stored preferences record writes", () => {
    const entries = new Map<string, string>([[modernUnicodeStorageKey, "false"]]);
    const storage = { getItem: (key: string) => entries.get(key) ?? null };

    expect(readModernUnicodeSetting(storage)).toBe(false);
    entries.set(modernUnicodeStorageKey, "true");
    expect(readModernUnicodeSetting(storage)).toBe(true);
  });

  it("keeps the default when storage is unavailable", () => {
    const storage = {
      getItem: () => {
        throw new Error("blocked");
      },
    };

    expect(readModernUnicodeSetting(storage)).toBe(true);
  });
});
