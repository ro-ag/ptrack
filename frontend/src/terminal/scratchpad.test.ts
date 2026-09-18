import { describe, expect, it } from "vitest";

import {
  addSnippet,
  clampScratchpadWidth,
  defaultScratchpadWidth,
  emptyScratchpad,
  maximumScratchpadWidth,
  minimumScratchpadWidth,
  orderedSnippets,
  readScratchpadOpen,
  readScratchpadWidth,
  removeSnippet,
  scratchpadMaxSnippets,
  scratchpadNotices,
  scratchpadOpenStorageKey,
  scratchpadSnippetMaxBytes,
  scratchpadTextMaxBytes,
  scratchpadWidthStorageKey,
  snippetPreview,
  togglePinned,
  utf8ByteLength,
  writeScratchpadOpen,
  writeScratchpadWidth,
  type ScratchpadSnippet,
} from "./scratchpad";

function snippet(
  id: number,
  text: string,
  createdAt: number,
  pinned = false,
): ScratchpadSnippet {
  return { id, text, pinned, createdAt };
}

/** Newest first, as the list is always kept: `snippet-<count>` … `snippet-1`. */
function listOf(count: number, pinned = false): ScratchpadSnippet[] {
  return Array.from({ length: count }, (_, index) => {
    const ordinal = count - index;
    return snippet(ordinal, `snippet-${ordinal}`, 1_000 + ordinal, pinned);
  });
}

function throwingStorage(): { getItem(): string; setItem(): void } {
  return {
    getItem() {
      throw new Error("storage is unavailable");
    },
    setItem() {
      throw new Error("storage is unavailable");
    },
  };
}

function memoryStorage(seed: Record<string, string> = {}) {
  const values = new Map(Object.entries(seed));
  return {
    values,
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
  };
}

describe("scratchpad limits", () => {
  it("pins the caps the store validates against", () => {
    expect(scratchpadTextMaxBytes).toBe(65_536);
    expect(scratchpadSnippetMaxBytes).toBe(4_096);
    expect(scratchpadMaxSnippets).toBe(50);
  });

  it("counts UTF-8 bytes rather than UTF-16 code units", () => {
    expect(utf8ByteLength("")).toBe(0);
    expect(utf8ByteLength("abc")).toBe(3);
    // A three-byte character is one UTF-16 unit but three stored bytes.
    expect(utf8ByteLength("あ")).toBe(3);
    expect(utf8ByteLength("あ".repeat(1_366))).toBe(4_098);
    // Astral characters are four bytes across two UTF-16 units.
    expect(utf8ByteLength("😀")).toBe(4);
  });

  it("starts empty at revision zero", () => {
    expect(emptyScratchpad()).toEqual({
      text: "",
      snippets: [],
      revision: 0,
      updatedAt: 0,
    });
    // Each call is an independent record: mutating one never leaks.
    const first = emptyScratchpad();
    first.snippets.push(snippet(1, "x", 1));
    expect(emptyScratchpad().snippets).toEqual([]);
  });

  it("states the notices the panel shows", () => {
    expect(scratchpadNotices).toEqual({
      tooLarge: "Selection is larger than 4 KB; not added to the scratchpad.",
      allPinned: "All 50 snippets are pinned; unpin one to add more.",
      reloaded: "Scratchpad changed elsewhere and was reloaded.",
      saveFailed: "Save failed",
    });
  });
});

describe("addSnippet", () => {
  it("refuses text that is empty after trimming", () => {
    for (const text of ["", "   ", "\n\t \r\n"]) {
      expect(addSnippet([], text, 10)).toEqual({ ok: false, reason: "empty" });
    }
  });

  it("refuses text over 4 096 UTF-8 bytes instead of truncating", () => {
    const tooLarge = "あ".repeat(1_366);
    expect(utf8ByteLength(tooLarge)).toBe(4_098);
    expect(addSnippet([], tooLarge, 10)).toEqual({ ok: false, reason: "too-large" });
    const exactlyAtTheCap = "a".repeat(4_096);
    const result = addSnippet([], exactlyAtTheCap, 10);
    expect(result.ok).toBe(true);
  });

  it("assigns max(existing id) + 1, starting at 1", () => {
    const first = addSnippet([], "alpha", 10);
    expect(first).toEqual({
      ok: true,
      snippets: [snippet(1, "alpha", 10)],
    });
    const second = addSnippet(
      [snippet(4, "alpha", 10), snippet(2, "beta", 11)],
      "gamma",
      12,
    );
    expect(second.ok && second.snippets[0]).toEqual(snippet(5, "gamma", 12));
  });

  it("puts a new snippet at the head, newest first", () => {
    const result = addSnippet([snippet(1, "alpha", 10)], "beta", 11);
    expect(result.ok && result.snippets.map((item) => item.text)).toEqual([
      "beta",
      "alpha",
    ]);
  });

  it("moves equal text to the top keeping its id, pinned flag and createdAt", () => {
    const existing = [
      snippet(1, "alpha", 10),
      snippet(2, "beta", 11, true),
      snippet(3, "gamma", 12),
    ];
    const result = addSnippet(existing, "beta", 99);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.snippets).toEqual([
      snippet(2, "beta", 11, true),
      snippet(1, "alpha", 10),
      snippet(3, "gamma", 12),
    ]);
    expect(result.snippets).toHaveLength(3);
    // The input is never mutated.
    expect(existing.map((item) => item.text)).toEqual(["alpha", "beta", "gamma"]);
  });

  it("evicts the oldest unpinned snippet past fifty", () => {
    const existing = listOf(scratchpadMaxSnippets);
    // snippet-1 sits last: the oldest entry, pinned, so it must survive.
    existing[existing.length - 1].pinned = true;
    const result = addSnippet(existing, "fresh", 9_999);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.snippets).toHaveLength(scratchpadMaxSnippets);
    expect(result.snippets[0]).toEqual(snippet(51, "fresh", 9_999));
    // snippet-2 was the oldest unpinned entry and is the one that left.
    expect(result.snippets.some((item) => item.text === "snippet-2")).toBe(false);
    expect(result.snippets.some((item) => item.text === "snippet-1")).toBe(true);
  });

  it("refuses the add when all fifty snippets are pinned", () => {
    const existing = listOf(scratchpadMaxSnippets, true);
    expect(addSnippet(existing, "fresh", 9_999)).toEqual({
      ok: false,
      reason: "all-pinned",
    });
  });

  it("still moves equal text to the top when every snippet is pinned", () => {
    const existing = listOf(scratchpadMaxSnippets, true);
    const result = addSnippet(existing, "snippet-7", 9_999);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.snippets[0]).toEqual(snippet(7, "snippet-7", 1_007, true));
    expect(result.snippets).toHaveLength(scratchpadMaxSnippets);
  });
});

describe("togglePinned and removeSnippet", () => {
  it("flips only the matching snippet", () => {
    const existing = [snippet(1, "alpha", 10), snippet(2, "beta", 11, true)];
    expect(togglePinned(existing, 1)).toEqual([
      snippet(1, "alpha", 10, true),
      snippet(2, "beta", 11, true),
    ]);
    expect(togglePinned(existing, 2)).toEqual([
      snippet(1, "alpha", 10),
      snippet(2, "beta", 11),
    ]);
    expect(existing[0].pinned).toBe(false);
  });

  it("leaves an unknown id alone", () => {
    const existing = [snippet(1, "alpha", 10)];
    expect(togglePinned(existing, 42)).toEqual(existing);
    expect(removeSnippet(existing, 42)).toEqual(existing);
  });

  it("removes by id without mutating the input", () => {
    const existing = [snippet(1, "alpha", 10), snippet(2, "beta", 11)];
    expect(removeSnippet(existing, 1)).toEqual([snippet(2, "beta", 11)]);
    expect(existing).toHaveLength(2);
  });
});

describe("orderedSnippets", () => {
  it("puts pinned rows first, then newest first", () => {
    const existing = [
      snippet(1, "alpha", 10),
      snippet(2, "beta", 30, true),
      snippet(3, "gamma", 20),
      snippet(4, "delta", 15, true),
    ];
    expect(orderedSnippets(existing).map((item) => item.text)).toEqual([
      "beta",
      "delta",
      "gamma",
      "alpha",
    ]);
  });

  it("is stable for equal createdAt and does not mutate the input", () => {
    const existing = [
      snippet(1, "alpha", 10),
      snippet(2, "beta", 10),
      snippet(3, "gamma", 10),
    ];
    expect(orderedSnippets(existing).map((item) => item.id)).toEqual([1, 2, 3]);
    expect(existing.map((item) => item.id)).toEqual([1, 2, 3]);
  });
});

describe("snippetPreview", () => {
  it("uses the first non-empty line with whitespace collapsed", () => {
    expect(snippetPreview("\n\n  git   status \n second line")).toBe("git status");
    expect(snippetPreview("first\tline\there")).toBe("first line here");
    expect(snippetPreview("\r\n  alpha\r\nbeta")).toBe("alpha");
  });

  it("returns an empty string when there is no non-empty line", () => {
    expect(snippetPreview("")).toBe("");
    expect(snippetPreview("  \n\t\n ")).toBe("");
  });

  it("ellipsises at eighty characters including the ellipsis", () => {
    const short = "a".repeat(80);
    expect(snippetPreview(short)).toBe(short);
    const long = "a".repeat(120);
    const preview = snippetPreview(long);
    expect(preview).toHaveLength(80);
    expect(preview.endsWith("…")).toBe(true);
    expect(preview).toBe(`${"a".repeat(79)}…`);
  });
});

describe("clampScratchpadWidth", () => {
  it("states its bounds", () => {
    expect(minimumScratchpadWidth).toBe(240);
    expect(defaultScratchpadWidth).toBe(320);
  });

  it("clamps to half the body width, never below 240", () => {
    expect(clampScratchpadWidth(320, 1_000)).toBe(320);
    expect(clampScratchpadWidth(900, 1_000)).toBe(500);
    expect(clampScratchpadWidth(10, 1_000)).toBe(240);
    // A body too narrow to halve still leaves the 240 floor standing.
    expect(clampScratchpadWidth(400, 300)).toBe(240);
    expect(maximumScratchpadWidth(1_000)).toBe(500);
    expect(maximumScratchpadWidth(300)).toBe(240);
  });

  it("rounds to whole pixels", () => {
    expect(clampScratchpadWidth(320.6, 1_000)).toBe(321);
    expect(clampScratchpadWidth(1_000, 777)).toBe(389);
  });

  it("falls back to 320 for a width that is not a finite number", () => {
    expect(clampScratchpadWidth(Number.NaN, 1_000)).toBe(320);
    expect(clampScratchpadWidth(Number.POSITIVE_INFINITY, 1_000)).toBe(320);
  });

  it("keeps only the lower bound while the body is unmeasured", () => {
    // The dock is hidden at mount, so clientWidth reads 0; a stored width must
    // survive that instead of collapsing to the minimum.
    expect(clampScratchpadWidth(420, 0)).toBe(420);
    expect(clampScratchpadWidth(420, Number.NaN)).toBe(420);
    expect(clampScratchpadWidth(100, 0)).toBe(240);
  });
});

describe("scratchpad localStorage mirrors", () => {
  it("names its keys", () => {
    expect(scratchpadOpenStorageKey).toBe("ptrack-terminal-scratchpad-open");
    expect(scratchpadWidthStorageKey).toBe("ptrack-terminal-scratchpad-width");
  });

  it("reads and writes the open flag", () => {
    const storage = memoryStorage();
    expect(readScratchpadOpen(storage)).toBe(false);
    writeScratchpadOpen(storage, true);
    expect(storage.values.get(scratchpadOpenStorageKey)).toBe("true");
    expect(readScratchpadOpen(storage)).toBe(true);
    writeScratchpadOpen(storage, false);
    expect(storage.values.get(scratchpadOpenStorageKey)).toBe("false");
    expect(readScratchpadOpen(storage)).toBe(false);
    expect(readScratchpadOpen(memoryStorage({
      [scratchpadOpenStorageKey]: "yes",
    }))).toBe(false);
  });

  it("reads and writes the width, clamped and rounded", () => {
    const storage = memoryStorage();
    expect(readScratchpadWidth(storage)).toBe(320);
    writeScratchpadWidth(storage, 412.4);
    expect(storage.values.get(scratchpadWidthStorageKey)).toBe("412");
    expect(readScratchpadWidth(storage)).toBe(412);
    expect(readScratchpadWidth(memoryStorage({
      [scratchpadWidthStorageKey]: "12",
    }))).toBe(240);
    expect(readScratchpadWidth(memoryStorage({
      [scratchpadWidthStorageKey]: "not a number",
    }))).toBe(320);
  });

  it("never throws when storage is unavailable", () => {
    const storage = throwingStorage();
    expect(() => readScratchpadOpen(storage)).not.toThrow();
    expect(readScratchpadOpen(storage)).toBe(false);
    expect(() => readScratchpadWidth(storage)).not.toThrow();
    expect(readScratchpadWidth(storage)).toBe(320);
    expect(() => writeScratchpadOpen(storage, true)).not.toThrow();
    expect(() => writeScratchpadWidth(storage, 400)).not.toThrow();
  });
});
