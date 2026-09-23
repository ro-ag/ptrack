import {
  WorkspacePersistenceScheduler,
  type PersistenceTimerClock,
} from "../workspace/persistence";

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
/** Matches `.terminal-scratchpad-splitter` in style.css. */
export const scratchpadSplitterWidth = 5;

export const scratchpadOpenStorageKey = "ptrack-terminal-scratchpad-open";
export const scratchpadWidthStorageKey = "ptrack-terminal-scratchpad-width";

export const scratchpadNotices = {
  tooLarge: "Selection is larger than 4 KB; not added to the scratchpad.",
  allPinned: "All 50 snippets are pinned; unpin one to add more.",
  reloaded: "Scratchpad changed elsewhere and was reloaded.",
  saveFailed: "Save failed",
  unavailable: "Scratchpad is unavailable; the copy was not added.",
} as const;

/** The two transient hints the saver writes over its own notices. */
export const scratchpadStatus = {
  saving: "Saving\u2026",
  saved: "Saved",
} as const;

/** The runtime's Display message for a stale-revision write. */
export const scratchpadConflictMessage = "scratchpad revision conflict";

export type SnippetAddResult =
  | { ok: true; snippets: ScratchpadSnippet[] }
  | { ok: false; reason: "empty" | "too-large" | "all-pinned" };

const encoder = new TextEncoder();

export function utf8ByteLength(text: string): number {
  return encoder.encode(text).length;
}

/** Below this many bytes left, the status line says how many remain. */
export const scratchpadTextWarnBytes = 4_096;

/**
 * The note is capped in UTF-8 bytes by the store, which a textarea `maxlength`
 * (UTF-16 code units) cannot express: a note of CJK text or emoji can be well
 * under the character limit and still be refused. `null` means nothing to say.
 */
export function scratchpadTextLimitNotice(text: string): string | null {
  const bytes = utf8ByteLength(text);
  if (bytes > scratchpadTextMaxBytes) {
    return `Too long by ${(bytes - scratchpadTextMaxBytes).toLocaleString("en-US")} bytes; not saved`;
  }
  const left = scratchpadTextMaxBytes - bytes;
  if (left < scratchpadTextWarnBytes) {
    return `${left.toLocaleString("en-US")} bytes left`;
  }
  return null;
}

export function scratchpadTextFits(text: string): boolean {
  return utf8ByteLength(text) <= scratchpadTextMaxBytes;
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

/**
 * Pinned rows first, then list order. The list is the recency order the user
 * acts on — `addSnippet` puts a new or re-copied snippet at index 0 — so
 * re-sorting by `createdAt` here would hide the "moves to the top" rule and
 * disagree with eviction, which takes the last unpinned entry.
 */
export function orderedSnippets(
  snippets: readonly ScratchpadSnippet[],
): ScratchpadSnippet[] {
  return [
    ...snippets.filter((snippet) => snippet.pinned),
    ...snippets.filter((snippet) => !snippet.pinned),
  ].map(cloneSnippet);
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

/**
 * Whether `#terminal-body` should be visible. Without the scratchpad open, a
 * closed, non-popped-out, single-pane tab collapses the dock: there is no
 * session, no popped-out notice to show, and nothing else in the pane. With
 * the scratchpad open, the note and clipboard strip must stay reachable even
 * in that same state, so the panel overrides the collapse.
 */
export function terminalBodyVisible(input: {
  state: string;
  poppedOut: boolean;
  singlePane: boolean;
  scratchpadOpen: boolean;
}): boolean {
  if (input.scratchpadOpen) return true;
  return !(input.state === "closed" && !input.poppedOut && input.singlePane);
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

// ---------------------------------------------------------------------------
// ScratchpadSaver
//
// The load / dirty / write / conflict state machine, lifted out of the dock so
// it can be tested without a terminal. It owns the record and the debounce
// scheduler; the host supplies the backend, the clipboard, and three callbacks
// that touch the DOM. Nothing here reads `document`.

export interface ScratchpadSaverBackend {
  get(generation: number): Promise<{ generation: number; scratchpad: Scratchpad }>;
  set(
    generation: number,
    revision: number,
    scratchpad: Scratchpad,
  ): Promise<{ generation: number; revision: number }>;
}

export interface ScratchpadSaverHost {
  generation: number;
  backend: ScratchpadSaverBackend;
  clock: PersistenceTimerClock;
  /** Puts text on the system clipboard. Rejection is tolerated. */
  setText(text: string): Promise<void>;
  /** Writes the saved-state hint. */
  status(text: string): void;
  /**
   * Installs a record that arrived from the store. Returns the local text that
   * must win instead (the host is mid-edit and `replaceLocalText` is false), or
   * null to accept the record as it stands.
   */
  applyRecord(record: Scratchpad, replaceLocalText: boolean): string | null;
  reportError(error: unknown): void;
  now?(): number;
}

function conflictPayload(error: unknown): unknown {
  if (error === null || typeof error !== "object") return undefined;
  return (error as { stored?: unknown }).stored;
}

export function isScratchpadConflict(error: unknown): boolean {
  if (typeof error === "string") return error.includes(scratchpadConflictMessage);
  if (error === null || typeof error !== "object") return false;
  const message = (error as { message?: unknown }).message;
  return typeof message === "string" && message.includes(scratchpadConflictMessage);
}

/**
 * The stored record a `ScratchpadConflict` carries, or null when the transport
 * dropped it (then the caller re-reads instead).
 */
export function scratchpadConflictRecord(error: unknown): Scratchpad | null {
  const stored = conflictPayload(error);
  if (stored === null || typeof stored !== "object") return null;
  const candidate = stored as Partial<Scratchpad>;
  if (typeof candidate.text !== "string") return null;
  if (!Array.isArray(candidate.snippets)) return null;
  if (typeof candidate.revision !== "number") return null;
  for (const snippet of candidate.snippets) {
    if (snippet === null || typeof snippet !== "object") return null;
    if (typeof snippet.id !== "number" || typeof snippet.text !== "string") return null;
    if (typeof snippet.pinned !== "boolean") return null;
    if (typeof snippet.createdAt !== "number") return null;
  }
  return cloneScratchpad({
    text: candidate.text,
    snippets: candidate.snippets,
    revision: candidate.revision,
    updatedAt: typeof candidate.updatedAt === "number" ? candidate.updatedAt : 0,
  });
}

export class ScratchpadSaver {
  readonly #host: ScratchpadSaverHost;
  readonly #scheduler: WorkspacePersistenceScheduler;
  #record: Scratchpad = emptyScratchpad();
  #loaded = false;
  #loading: Promise<void> | null = null;
  #saving = false;
  #pending = false;
  #dirty = false;
  #edits = 0;
  #disposed = false;
  #inflight: Promise<void> = Promise.resolve();

  constructor(host: ScratchpadSaverHost) {
    this.#host = host;
    this.#scheduler = new WorkspacePersistenceScheduler(host.clock, () => {
      void this.save();
    });
  }

  get record(): Scratchpad {
    return this.#record;
  }

  get loaded(): boolean {
    return this.#loaded;
  }

  /** True while an edit has not reached the store yet, including after a failure. */
  get dirty(): boolean {
    return this.#dirty;
  }

  get enabled(): boolean {
    return this.#host.generation !== 0;
  }

  markText(text: string): void {
    this.#record.text = text;
    this.#markDirty();
  }

  applySnippets(snippets: ScratchpadSnippet[]): void {
    this.#record.snippets = snippets;
    this.#markDirty();
  }

  #markDirty(): void {
    this.#dirty = true;
    this.#edits += 1;
    this.#status(scratchpadStatus.saving);
    // An oversized note stays dirty and says so; the store would refuse it.
    if (scratchpadTextFits(this.#record.text)) this.#scheduler.markDirty();
  }

  /**
   * Every status line carries the byte budget when it matters: over the cap
   * the refusal replaces the hint, near it the remaining bytes follow it.
   */
  #status(text: string): void {
    const limit = scratchpadTextLimitNotice(this.#record.text);
    if (limit === null) this.#host.status(text);
    else if (!scratchpadTextFits(this.#record.text)) this.#host.status(limit);
    else this.#host.status(`${text} \u00b7 ${limit}`);
  }

  /** One read per instance, shared by every caller that needs the stored record. */
  ensureLoaded(): Promise<void> {
    if (this.#loaded || !this.enabled || this.#disposed) return Promise.resolve();
    if (this.#loading === null) {
      this.#loading = this.load().finally(() => {
        this.#loading = null;
      });
    }
    return this.#loading;
  }

  async load(replaceLocalText = false): Promise<void> {
    if (!this.enabled) return;
    try {
      const result = await this.#host.backend.get(this.#host.generation);
      // A response for another generation belongs to a dock that is already gone.
      if (result.generation !== this.#host.generation) return;
      this.#install(result.scratchpad, replaceLocalText);
    } catch (error) {
      this.#host.reportError(error);
    }
  }

  #install(scratchpad: Scratchpad, replaceLocalText: boolean): void {
    this.#record = cloneScratchpad(scratchpad);
    this.#loaded = true;
    const kept = this.#host.applyRecord(this.#record, replaceLocalText);
    if (kept === null) {
      this.#dirty = false;
      this.#status(scratchpadStatus.saved);
      return;
    }
    this.#record.text = kept;
    this.#markDirty();
  }

  /**
   * Starts the write. When the record has already been read the backend call is
   * issued synchronously, so a flush from `dispose()` reaches the runtime before
   * the caller tears anything down.
   */
  save(): Promise<void> {
    if (!this.enabled) return Promise.resolve();
    if (this.#saving) {
      this.#pending = true;
      return Promise.resolve();
    }
    // Only a first write needs the stored revision, and only while the instance
    // is alive: after disposal there is no time left to wait for a read.
    const wait = this.#loaded || this.#disposed ? null : this.ensureLoaded();
    this.#inflight = this.#run(wait);
    return this.#inflight;
  }

  async #run(wait: Promise<void> | null): Promise<void> {
    this.#saving = true;
    try {
      if (wait !== null) {
        await wait;
        if (!this.#loaded) {
          // The read failed; writing at revision 0 would take the conflict path
          // for what was a transient error. Stay dirty and retry on the next edit.
          this.#status(scratchpadNotices.saveFailed);
          return;
        }
      }
      do {
        this.#pending = false;
        await this.#send();
      } while (this.#pending);
    } finally {
      this.#saving = false;
    }
  }

  async #send(): Promise<void> {
    const record = this.#record;
    const edits = this.#edits;
    if (!scratchpadTextFits(record.text)) {
      this.#status(scratchpadStatus.saving);
      return;
    }
    this.#status(scratchpadStatus.saving);
    try {
      const result = await this.#host.backend.set(
        this.#host.generation,
        record.revision,
        cloneScratchpad(record),
      );
      if (this.#record === record) {
        record.revision = result.revision;
        record.updatedAt = this.#now();
      }
      this.#loaded = true;
      if (this.#edits === edits) this.#dirty = false;
      this.#status(scratchpadStatus.saved);
    } catch (error) {
      if (isScratchpadConflict(error)) {
        await this.#recoverConflict(error);
        return;
      }
      this.#status(scratchpadNotices.saveFailed);
      this.#host.reportError(error);
    }
  }

  async #recoverConflict(error: unknown): Promise<void> {
    // Nothing typed is lost: the local note reaches the clipboard before the
    // stored record replaces it.
    try {
      await this.#host.setText(this.#record.text);
    } catch {
      // A missing native clipboard must not block the reload.
    }
    const stored = scratchpadConflictRecord(error);
    if (stored === null) await this.load(true);
    else this.#install(stored, true);
    this.#status(scratchpadNotices.reloaded);
  }

  /** Writes any pending edit now. Returns true when a write was started. */
  flush(): boolean {
    if (this.#scheduler.flush()) return true;
    if (!this.#dirty || this.#saving) return false;
    void this.save();
    return true;
  }

  /**
   * Writes any pending edit and resolves once every write that was started
   * has settled. A project switch awaits this before the runtime moves to the
   * next generation: a write issued after that is fenced out and lost.
   */
  async flushPending(): Promise<void> {
    this.flush();
    let settled: Promise<void> | null = null;
    while (settled !== this.#inflight) {
      settled = this.#inflight;
      await settled.catch(() => {});
    }
  }

  dispose(): void {
    if (this.#disposed) return;
    // Disposed first, so the final write skips the read it can no longer wait for.
    this.#disposed = true;
    this.flush();
    this.#scheduler.dispose();
  }

  #now(): number {
    return this.#host.now ? this.#host.now() : Date.now();
  }
}
