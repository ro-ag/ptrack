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
  scratchpadSplitterWidth,
  scratchpadStatus,
  scratchpadTextMaxBytes,
  scratchpadWidthStorageKey,
  snippetPreview,
  togglePinned,
  utf8ByteLength,
  writeScratchpadOpen,
  writeScratchpadWidth,
  isScratchpadConflict,
  ScratchpadSaver,
  scratchpadConflictRecord,
  type Scratchpad,
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
      unavailable: "Scratchpad is unavailable; the copy was not added.",
    });
    expect(scratchpadStatus).toEqual({ saving: "Saving\u2026", saved: "Saved" });
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
  it("puts pinned rows first and otherwise keeps list order", () => {
    const existing = [
      snippet(1, "alpha", 10),
      snippet(2, "beta", 30, true),
      snippet(3, "gamma", 20),
      snippet(4, "delta", 15, true),
    ];
    expect(orderedSnippets(existing).map((item) => item.text)).toEqual([
      "beta",
      "delta",
      "alpha",
      "gamma",
    ]);
  });

  it("never re-sorts by createdAt, so a re-copied snippet really shows on top", () => {
    // The composition addSnippet → orderedSnippets is the rule the user sees:
    // "adding text equal to an existing snippet moves that snippet to the top".
    const existing = [
      snippet(1, "alpha", 10),
      snippet(2, "beta", 11),
      snippet(3, "gamma", 12),
    ];
    const result = addSnippet(existing, "beta", 99);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(orderedSnippets(result.snippets).map((item) => item.text)).toEqual([
      "beta",
      "alpha",
      "gamma",
    ]);
    // …and the moved snippet still carries its original identity.
    expect(orderedSnippets(result.snippets)[0]).toEqual(snippet(2, "beta", 11));
  });

  it("shows last the entry eviction drops, and does not mutate the input", () => {
    const existing = listOf(scratchpadMaxSnippets);
    const displayed = orderedSnippets(existing);
    expect(displayed.at(-1)).toEqual(snippet(1, "snippet-1", 1_001));
    const evicting = addSnippet(existing, "fresh", 9_999);
    expect(evicting.ok).toBe(true);
    if (!evicting.ok) return;
    // What the panel shows as oldest is exactly what the add evicted.
    expect(evicting.snippets.some((item) => item.text === "snippet-1")).toBe(false);
    expect(existing.map((item) => item.id)).toEqual(displayed.map((item) => item.id));
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
    // The dock adds the splitter to the gutter it reserves for body overlays.
    expect(scratchpadSplitterWidth).toBe(5);
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

// --- ScratchpadSaver -------------------------------------------------------

class ManualClock {
  readonly pending = new Map<number, () => void>();
  #next = 1;

  setTimeout(callback: () => void): unknown {
    const handle = this.#next++;
    this.pending.set(handle, callback);
    return handle;
  }

  clearTimeout(handle: unknown): void {
    this.pending.delete(handle as number);
  }

  runAll(): void {
    for (const [handle, callback] of [...this.pending]) {
      this.pending.delete(handle);
      callback();
    }
  }
}

interface SetCall {
  generation: number;
  revision: number;
  scratchpad: Scratchpad;
}

function stored(
  text: string,
  revision: number,
  snippets: ScratchpadSnippet[] = [],
): Scratchpad {
  return { text, snippets, revision, updatedAt: 1_700 };
}

function conflict(storedRecord?: Scratchpad): Error & { stored?: Scratchpad } {
  const error: Error & { stored?: Scratchpad } = new Error(
    "scratchpad revision conflict",
  );
  if (storedRecord) error.stored = storedRecord;
  return error;
}

function saverHarness(options: {
  generation?: number;
  record?: Scratchpad;
  getError?: unknown;
} = {}) {
  const clock = new ManualClock();
  const sets: SetCall[] = [];
  const gets: number[] = [];
  const statuses: string[] = [];
  const clipboard: string[] = [];
  const errors: unknown[] = [];
  const applied: Array<{ record: Scratchpad; replaceLocalText: boolean }> = [];
  let getResult = options.record ?? stored("", 0);
  let getError = options.getError;
  let setResult: (call: SetCall) => Promise<{ generation: number; revision: number }> =
    (call) =>
      Promise.resolve({ generation: call.generation, revision: call.revision + 1 });
  let localText: string | null = null;
  let clipboardError: unknown = null;

  const saver = new ScratchpadSaver({
    generation: options.generation ?? 7,
    backend: {
      get: (generation) => {
        gets.push(generation);
        return getError === undefined
          ? Promise.resolve({ generation, scratchpad: getResult })
          : Promise.reject(getError);
      },
      set: (generation, revision, scratchpad) => {
        const call = { generation, revision, scratchpad };
        sets.push(call);
        return setResult(call);
      },
    },
    clock,
    setText: (text) => {
      clipboard.push(text);
      return clipboardError === null
        ? Promise.resolve()
        : Promise.reject(clipboardError);
    },
    status: (text) => statuses.push(text),
    applyRecord: (record, replaceLocalText) => {
      applied.push({ record, replaceLocalText });
      return localText;
    },
    reportError: (error) => errors.push(error),
    now: () => 4_242,
  });

  return {
    saver,
    clock,
    sets,
    gets,
    statuses,
    clipboard,
    errors,
    applied,
    setStored: (record: Scratchpad) => {
      getResult = record;
      getError = undefined;
    },
    failGet: (error: unknown) => {
      getError = error;
    },
    failSet: (error: unknown) => {
      setResult = () => Promise.reject(error);
    },
    succeedSet: () => {
      setResult = (call) =>
        Promise.resolve({ generation: call.generation, revision: call.revision + 1 });
    },
    keepLocalText: (text: string | null) => {
      localText = text;
    },
    failClipboard: (error: unknown) => {
      clipboardError = error;
    },
  };
}

const settle = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

describe("ScratchpadSaver", () => {
  it("reads once before the first write and sends the stored revision", async () => {
    const h = saverHarness({ record: stored("from the store", 4) });
    h.saver.markText("typed");
    await settle();
    h.clock.runAll();
    await settle();
    expect(h.gets).toEqual([7]);
    expect(h.sets).toHaveLength(1);
    expect(h.sets[0].revision).toBe(4);
    expect(h.sets[0].generation).toBe(7);
    expect(h.saver.record.revision).toBe(5);
    expect(h.saver.record.updatedAt).toBe(4_242);
    expect(h.saver.dirty).toBe(false);
    expect(h.statuses.at(-1)).toBe(scratchpadStatus.saved);
  });

  it("flushes on dispose and actually issues the write", async () => {
    // The defect this pins: dispose() used to suspend on the pending read and
    // bail once the dock marked itself disposed, so the last note never left.
    const h = saverHarness({ record: stored("stored", 2) });
    await h.saver.ensureLoaded();
    h.saver.markText("typed just before quitting");
    expect(h.sets).toHaveLength(0);
    h.saver.dispose();
    // The backend call is issued synchronously, before any await unwinds.
    expect(h.sets).toHaveLength(1);
    expect(h.sets[0].scratchpad.text).toBe("typed just before quitting");
    expect(h.clock.pending.size).toBe(0);
    await settle();
  });

  it("flushes a pending note on a project switch, before the read resolves", async () => {
    const h = saverHarness({ record: stored("stored", 9) });
    h.saver.markText("half-typed note");
    h.saver.dispose();
    await settle();
    expect(h.sets).toHaveLength(1);
    expect(h.sets[0].scratchpad.text).toBe("half-typed note");
  });

  it("does nothing on dispose when nothing was edited", () => {
    const h = saverHarness();
    h.saver.dispose();
    expect(h.sets).toEqual([]);
  });

  it("flush() writes an edit the debounce has not fired yet", async () => {
    const h = saverHarness({ record: stored("stored", 1) });
    await h.saver.ensureLoaded();
    h.saver.markText("note");
    expect(h.saver.flush()).toBe(true);
    expect(h.sets).toHaveLength(1);
    await settle();
    expect(h.saver.flush()).toBe(false);
  });

  it("recovers from a conflict that carries the stored record", async () => {
    const h = saverHarness({ record: stored("stored", 3) });
    await h.saver.ensureLoaded();
    const remote = stored("what the other window wrote", 8, [
      { id: 1, text: "remote snippet", pinned: false, createdAt: 12 },
    ]);
    h.saver.markText("my unsaved note");
    h.failSet(conflict(remote));
    h.saver.flush();
    await settle();
    // Clipboard first, so nothing typed is lost.
    expect(h.clipboard).toEqual(["my unsaved note"]);
    // The stored record arrives with the error: no second read.
    expect(h.gets).toHaveLength(1);
    expect(h.saver.record.text).toBe("what the other window wrote");
    expect(h.saver.record.revision).toBe(8);
    expect(h.applied.at(-1)?.replaceLocalText).toBe(true);
    expect(h.statuses.at(-1)).toBe(scratchpadNotices.reloaded);
  });

  it("re-reads when the conflict payload did not survive the transport", async () => {
    const h = saverHarness({ record: stored("stored", 3) });
    await h.saver.ensureLoaded();
    h.saver.markText("my unsaved note");
    h.failSet(conflict());
    h.setStored(stored("reloaded text", 8));
    h.saver.flush();
    await settle();
    expect(h.clipboard).toEqual(["my unsaved note"]);
    expect(h.gets).toEqual([7, 7]);
    expect(h.saver.record.text).toBe("reloaded text");
    expect(h.statuses.at(-1)).toBe(scratchpadNotices.reloaded);
  });

  it("reloads even when the clipboard is unavailable", async () => {
    const h = saverHarness({ record: stored("stored", 3) });
    await h.saver.ensureLoaded();
    h.saver.markText("note");
    h.failClipboard(new Error("Native clipboard access is unavailable"));
    h.failSet(conflict(stored("remote", 4)));
    h.saver.flush();
    await settle();
    expect(h.saver.record.text).toBe("remote");
    expect(h.errors).toEqual([]);
  });

  it("keeps the edit dirty when a save fails, and retries on the next edit", async () => {
    const h = saverHarness({ record: stored("stored", 1) });
    await h.saver.ensureLoaded();
    h.saver.markText("first");
    h.failSet(new Error("runtime is unavailable"));
    h.saver.flush();
    await settle();
    expect(h.sets).toHaveLength(1);
    expect(h.saver.dirty).toBe(true);
    expect(h.statuses.at(-1)).toBe(scratchpadNotices.saveFailed);
    expect(h.errors).toHaveLength(1);
    // No retry storm while the backend is down: the timer is not re-armed.
    h.clock.runAll();
    await settle();
    expect(h.sets).toHaveLength(1);
    // The next edit retries.
    h.succeedSet();
    h.saver.markText("second");
    h.saver.flush();
    await settle();
    expect(h.sets).toHaveLength(2);
    expect(h.sets[1].scratchpad.text).toBe("second");
    expect(h.saver.dirty).toBe(false);
  });

  it("does not write at revision 0 when the first read failed", async () => {
    const h = saverHarness();
    h.failGet(new Error("workspace is unavailable"));
    h.saver.markText("typed");
    h.saver.flush();
    await settle();
    expect(h.sets).toEqual([]);
    expect(h.saver.loaded).toBe(false);
    expect(h.saver.dirty).toBe(true);
    expect(h.statuses.at(-1)).toBe(scratchpadNotices.saveFailed);
  });

  it("keeps mid-edit local text when a load lands under the cursor", async () => {
    const h = saverHarness({ record: stored("from the store", 2) });
    h.keepLocalText("what the user is typing");
    await h.saver.ensureLoaded();
    expect(h.saver.record.text).toBe("what the user is typing");
    expect(h.saver.dirty).toBe(true);
    expect(h.statuses.at(-1)).toBe(scratchpadStatus.saving);
  });

  it("coalesces a save that arrives while one is in flight", async () => {
    const h = saverHarness({ record: stored("stored", 1) });
    await h.saver.ensureLoaded();
    h.saver.markText("first");
    void h.saver.save();
    h.saver.markText("second");
    void h.saver.save();
    await settle();
    expect(h.sets).toHaveLength(2);
    expect(h.sets[1].scratchpad.text).toBe("second");
  });

  it("does nothing at all without a workspace generation", async () => {
    const h = saverHarness({ generation: 0 });
    expect(h.saver.enabled).toBe(false);
    h.saver.markText("typed");
    h.saver.flush();
    await h.saver.ensureLoaded();
    await settle();
    expect(h.gets).toEqual([]);
    expect(h.sets).toEqual([]);
  });

  it("sends a defensive copy, so a later edit cannot mutate a request", async () => {
    const h = saverHarness({ record: stored("stored", 1) });
    await h.saver.ensureLoaded();
    h.saver.markText("note");
    h.saver.flush();
    const sent = h.sets[0].scratchpad;
    h.saver.markText("changed while the write was in flight");
    expect(sent.text).toBe("note");
    await settle();
  });
});

describe("scratchpadConflictRecord", () => {
  it("accepts a well-formed payload", () => {
    const record = stored("text", 5, [
      { id: 2, text: "snippet", pinned: true, createdAt: 11 },
    ]);
    expect(scratchpadConflictRecord(conflict(record))).toEqual(record);
  });

  it("rejects anything it cannot trust, so the caller re-reads instead", () => {
    expect(scratchpadConflictRecord(new Error("boom"))).toBeNull();
    expect(scratchpadConflictRecord(null)).toBeNull();
    expect(scratchpadConflictRecord({ stored: "nope" })).toBeNull();
    expect(scratchpadConflictRecord({ stored: { text: 1, snippets: [], revision: 0 } }))
      .toBeNull();
    expect(scratchpadConflictRecord({ stored: { text: "", snippets: {}, revision: 0 } }))
      .toBeNull();
    expect(scratchpadConflictRecord({
      stored: { text: "", snippets: [{ id: "1", text: "x", pinned: false, createdAt: 0 }], revision: 0 },
    })).toBeNull();
  });

  it("recognises the runtime's conflict message wherever it is wrapped", () => {
    expect(isScratchpadConflict(new Error("scratchpad revision conflict"))).toBe(true);
    expect(isScratchpadConflict("SetScratchpadV1: scratchpad revision conflict"))
      .toBe(true);
    expect(isScratchpadConflict(new Error("stale workspace generation"))).toBe(false);
    expect(isScratchpadConflict(undefined)).toBe(false);
  });
});
