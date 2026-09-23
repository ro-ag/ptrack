import { describe, expect, it } from "vitest";

import { formatBytes, relativeTime } from "./format";

describe("relative time", () => {
  const now = Date.UTC(2026, 8, 22, 12);

  it("reads the same instant consistently in both styles", () => {
    const cases: [number, string, string][] = [
      [10_000, "now", "just now"],
      [120_000, "2m ago", "2 minutes ago"],
      [3_600_000, "1h ago", "1 hour ago"],
      [5_400_000, "1h ago", "1 hour ago"],
      [86_400_000 * 3, "3d ago", "3 days ago"],
      [86_400_000 * 4, "4d ago", "4 days ago"],
      [86_400_000 * 60, "2mo ago", "2 months ago"],
      [86_400_000 * 400, "1y ago", "1 year ago"],
    ];
    for (const [elapsed, short, long] of cases) {
      expect(relativeTime(now - elapsed, "short", now)).toBe(short);
      expect(relativeTime(now - elapsed, "long", now)).toBe(long);
    }
  });

  it("names future instants instead of clamping them to now", () => {
    expect(relativeTime(now + 120_000, "long", now)).toBe("in 2 minutes");
    expect(relativeTime(now + 300_000, "short", now)).toBe("in 5m");
  });

  it("accepts ISO strings and Dates", () => {
    const iso = new Date(now - 120_000).toISOString();
    expect(relativeTime(iso, "short", now)).toBe("2m ago");
    expect(relativeTime(new Date(now - 120_000), "long", now)).toBe("2 minutes ago");
  });

  it("marks unreadable input without inventing a time", () => {
    expect(relativeTime(Number.NaN, "long", now)).toBe("Date unavailable");
    expect(relativeTime("not a date", "short", now)).toBe("");
    expect(relativeTime(undefined, "short", now)).toBe("");
  });
});

describe("byte sizes", () => {
  it("uses one binary scale everywhere", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1023)).toBe("1023 B");
    expect(formatBytes(1536)).toBe("1.5 KiB");
    expect(formatBytes(2 * 1024 * 1024)).toBe("2.0 MiB");
    expect(formatBytes(3 * 1024 ** 3)).toBe("3.0 GiB");
  });

  it("treats non-finite and negative sizes as zero", () => {
    expect(formatBytes(Number.POSITIVE_INFINITY)).toBe("0 B");
    expect(formatBytes(-4)).toBe("0 B");
    expect(formatBytes("12")).toBe("12 B");
  });
});
