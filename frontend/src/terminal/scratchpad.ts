// Pure rules for the terminal scratchpad: snippet bookkeeping, the one-line
// preview, panel geometry, and the localStorage mirrors. The dock only wires
// DOM, backend, and the persistence scheduler; every decision lives here so it
// can be tested without a terminal.

export interface ScratchpadSnippet {
  id: number;
  text: string;
  pinned: boolean;
  createdAt: number;
}

export interface Scratchpad {
  text: string;
  snippets: ScratchpadSnippet[];
  revision: number;
  updatedAt: number;
}

export const scratchpadTextMaxBytes = 65_536;
export const scratchpadSnippetMaxBytes = 4_096;
export const scratchpadMaxSnippets = 50;

export const minimumScratchpadWidth = 240;
export const defaultScratchpadWidth = 320;

export const scratchpadOpenStorageKey = "ptrack-terminal-scratchpad-open";
export const scratchpadWidthStorageKey = "ptrack-terminal-scratchpad-width";

export const scratchpadNotices = {
  tooLarge: "Selection is larger than 4 KB; not added to the scratchpad.",
  allPinned: "All 50 snippets are pinned; unpin one to add more.",
  reloaded: "Scratchpad changed elsewhere and was reloaded.",
  saveFailed: "Save failed",
} as const;

export type SnippetAddResult =
  | { ok: true; snippets: ScratchpadSnippet[] }
  | { ok: false; reason: "empty" | "too-large" | "all-pinned" };

const encoder = new TextEncoder();

export function utf8ByteLength(text: string): number {
  return encoder.encode(text).length;
}

export function emptyScratchpad(): Scratchpad {
  return { text: "", snippets: [], revision: 0, updatedAt: 0 };
}

function cloneSnippet(snippet: ScratchpadSnippet): ScratchpadSnippet {
  return {
    id: snippet.id,
    text: snippet.text,
    pinned: snippet.pinned,
    createdAt: snippet.createdAt,
  };
}

export function cloneScratchpad(scratchpad: Scratchpad): Scratchpad {
  return {
    text: scratchpad.text,
    snippets: scratchpad.snippets.map(cloneSnippet),
    revision: scratchpad.revision,
    updatedAt: scratchpad.updatedAt,
  };
}

function nextSnippetId(snippets: readonly ScratchpadSnippet[]): number {
  let highest = 0;
  for (const snippet of snippets) {
    if (snippet.id > highest) highest = snippet.id;
  }
  return highest + 1;
}

/**
 * Trim-empty text is ignored; text over 4 096 UTF-8 bytes is refused rather
 * than truncated; text equal to an existing snippet moves that snippet to the
 * top keeping its id, pinned flag, and createdAt; an add past 50 evicts the
 * oldest unpinned snippet, and is refused when all 50 are pinned.
 */
export function addSnippet(
  snippets: readonly ScratchpadSnippet[],
  text: string,
  now: number,
): SnippetAddResult {
  if (text.trim() === "") return { ok: false, reason: "empty" };
  if (utf8ByteLength(text) > scratchpadSnippetMaxBytes) {
    return { ok: false, reason: "too-large" };
  }
  const existingIndex = snippets.findIndex((snippet) => snippet.text === text);
  if (existingIndex >= 0) {
    const moved = cloneSnippet(snippets[existingIndex]);
    const rest = snippets
      .filter((_, index) => index !== existingIndex)
      .map(cloneSnippet);
    return { ok: true, snippets: [moved, ...rest] };
  }
  const kept = snippets.map(cloneSnippet);
  if (kept.length >= scratchpadMaxSnippets) {
    // Newest first, so the oldest unpinned entry is the last unpinned one.
    let evictIndex = -1;
    for (let index = kept.length - 1; index >= 0; index -= 1) {
      if (!kept[index].pinned) {
        evictIndex = index;
        break;
      }
    }
    if (evictIndex < 0) return { ok: false, reason: "all-pinned" };
    kept.splice(evictIndex, 1);
  }
  const added: ScratchpadSnippet = {
    id: nextSnippetId(snippets),
    text,
    pinned: false,
    createdAt: now,
  };
  return { ok: true, snippets: [added, ...kept] };
}

export function togglePinned(
  snippets: readonly ScratchpadSnippet[],
  id: number,
): ScratchpadSnippet[] {
  return snippets.map((snippet) =>
    snippet.id === id
      ? { ...cloneSnippet(snippet), pinned: !snippet.pinned }
      : cloneSnippet(snippet),
  );
}

export function removeSnippet(
  snippets: readonly ScratchpadSnippet[],
  id: number,
): ScratchpadSnippet[] {
  return snippets.filter((snippet) => snippet.id !== id).map(cloneSnippet);
}

/** Pinned first, then by createdAt descending, stable within equal keys. */
export function orderedSnippets(
  snippets: readonly ScratchpadSnippet[],
): ScratchpadSnippet[] {
  return snippets
    .map((snippet, index) => ({ snippet, index }))
    .sort((left, right) => {
      if (left.snippet.pinned !== right.snippet.pinned) {
        return left.snippet.pinned ? -1 : 1;
      }
      if (left.snippet.createdAt !== right.snippet.createdAt) {
        return right.snippet.createdAt - left.snippet.createdAt;
      }
      return left.index - right.index;
    })
    .map((entry) => cloneSnippet(entry.snippet));
}

const previewMaxLength = 80;

/** First non-empty line, whitespace collapsed, at most 80 characters with "…". */
export function snippetPreview(text: string): string {
  for (const rawLine of text.split("\n")) {
    const line = rawLine.replace(/\s+/g, " ").trim();
    if (line === "") continue;
    return line.length <= previewMaxLength
      ? line
      : `${line.slice(0, previewMaxLength - 1)}…`;
  }
  return "";
}

/**
 * Half the body, never under the 240 px floor. An unmeasured body (0 or a
 * non-finite width, which is what a hidden dock reports) keeps the floor only,
 * so a stored width survives a mount that cannot measure anything yet.
 */
export function maximumScratchpadWidth(bodyWidth: number): number {
  if (!Number.isFinite(bodyWidth) || bodyWidth <= 0) {
    return Number.POSITIVE_INFINITY;
  }
  return Math.max(minimumScratchpadWidth, Math.round(bodyWidth / 2));
}

export function clampScratchpadWidth(width: number, bodyWidth: number): number {
  const requested = Number.isFinite(width) ? Math.round(width) : defaultScratchpadWidth;
  const maximum = maximumScratchpadWidth(bodyWidth);
  return Math.max(minimumScratchpadWidth, Math.min(requested, maximum));
}

export function readScratchpadOpen(storage: Pick<Storage, "getItem">): boolean {
  try {
    return storage.getItem(scratchpadOpenStorageKey) === "true";
  } catch {
    return false;
  }
}

export function writeScratchpadOpen(
  storage: Pick<Storage, "setItem">,
  open: boolean,
): void {
  try {
    storage.setItem(scratchpadOpenStorageKey, String(open));
  } catch {
    // The panel state is a convenience mirror; a refusal is not an error.
  }
}

export function readScratchpadWidth(storage: Pick<Storage, "getItem">): number {
  let raw: string | null;
  try {
    raw = storage.getItem(scratchpadWidthStorageKey);
  } catch {
    return defaultScratchpadWidth;
  }
  if (raw === null) return defaultScratchpadWidth;
  const parsed = Number.parseFloat(raw);
  if (!Number.isFinite(parsed)) return defaultScratchpadWidth;
  return clampScratchpadWidth(parsed, 0);
}

export function writeScratchpadWidth(
  storage: Pick<Storage, "setItem">,
  width: number,
): void {
  try {
    storage.setItem(scratchpadWidthStorageKey, String(Math.round(width)));
  } catch {
    // The panel width is a convenience mirror; a refusal is not an error.
  }
}
