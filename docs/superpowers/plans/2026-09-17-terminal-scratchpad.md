# Terminal Scratchpad Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A per-project scratchpad panel beside the terminal panes: a persisted markdown note plus a clipboard strip of user-copied snippets that paste back into a pane.

**Architecture:** One new singleton store collection (`ProjectScratchpad`, payload schema 7, length-framed) carried through ptrack-core model/codec/validation, ptrack-store schema/typed/project, and two generation-fenced GUI commands in ptrack-app. The frontend adds a pure `terminal/scratchpad.ts` module (snippet rules, limits, layout clamp) and a panel inside `#terminal-body` wired from `TerminalDock` in `pane.ts`, saving through `WorkspacePersistenceScheduler`.

**Tech Stack:** Rust (ptrack-core, ptrack-store, ptrack-app, Tauri 2 host tests), TypeScript + vitest, xterm.js.

**Spec:** `docs/superpowers/specs/2026-09-17-terminal-scratchpad-design.md`

## Global Constraints

- Limits, verbatim from the spec: note text ≤ 65 536 bytes; ≤ 50 snippets; snippet text ≤ 4 096 bytes and non-empty after trim; ids unique and monotonic.
- Payload schema stays `NATIVE_PAYLOAD_SCHEMA = 7`; the scratchpad body is length-framed (`frame`/`unframe` in `crates/ptrack-core/src/codec.rs`) from the first byte written. Never redefine a schema number (MEMENTO: schema-number-is-immutable-once-written).
- Command names: `GetScratchpadV1`, `SetScratchpadV1`; both take `generation` as argument 0 and use `require_generation`.
- Frozen allowlists are updated deliberately in every fixture: `crates/ptrack-app/src/desktop_runtime.rs` `COMMANDS`, `desktop_runtime_test.rs` exact list, `src-tauri/src/main_test.rs` if it lists commands, `frontend/src/tauri-bridge.js` `COMMANDS`, `frontend/src/tauri-bridge.test.js`.
- Rust unit tests live in Go-style sibling `*_test.rs` files, never `#[cfg(test)] mod tests` inside a source file.
- Snippets come only from explicit user copies; nothing scrapes terminal output.
- localStorage keys: `ptrack-terminal-scratchpad-open`, `ptrack-terminal-scratchpad-width`.
- No AI attribution in commits or PR. Conventional commit prefixes.
- Any edit to a file listed in `docs/help/assets/screenshots/manifest.json` → `uiSources` requires refreshing `uiSourceSha256` (compute with `source_digest()` from `tools/help_check.py`) and appending a dated line to `sourceReview`.

---

### Task 1: Scratchpad record in ptrack-core

**Files:**
- Modify: `crates/ptrack-core/src/model.rs` (near `Meta` at :272; `RecordKind` and `NativeRecord` at :692-745)
- Modify: `crates/ptrack-core/src/codec.rs` (decode arm :188, encode arm :224, `encode_meta`/`decode_meta` :517-560 as the pattern, `frame` :652)
- Modify: `crates/ptrack-core/src/validation.rs` (`impl Validate for Meta` :363 as the pattern)
- Test: `crates/ptrack-core/src/codec_test.rs`, `crates/ptrack-core/src/validation_test.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct ScratchpadSnippet { pub id: u64, pub text: String, pub pinned: bool, pub created_at: Timestamp }
  pub struct Scratchpad { pub text: String, pub snippets: Vec<ScratchpadSnippet>, pub revision: u64, pub updated_at: Timestamp }
  pub const SCRATCHPAD_TEXT_MAX_BYTES: usize = 65_536;
  pub const SCRATCHPAD_SNIPPET_MAX_BYTES: usize = 4_096;
  pub const SCRATCHPAD_MAX_SNIPPETS: usize = 50;
  RecordKind::Scratchpad = 14 => "scratchpad"
  NativeRecord::Scratchpad(Scratchpad)
  impl Validate for Scratchpad
  impl Default for Scratchpad  // text "", no snippets, revision 0, updated_at = Timestamp default used elsewhere for "unset"
  ```

- [ ] **Step 1: Write failing codec tests** in `codec_test.rs`: (a) round trip of a scratchpad with two snippets (one pinned) at schema 7; (b) a golden byte pin — build the record, encode, assert the exact byte vector (write the vector after the first passing run, then keep it frozen, matching how the file pins other kinds around :94-131); (c) decoding at any schema below 7 is rejected for this kind (the kind did not exist before 7).
- [ ] **Step 2: Write failing validation tests** in `validation_test.rs`: text of 65 537 bytes rejected; 51 snippets rejected; snippet of 4 097 bytes rejected; snippet `"  "` rejected; duplicate ids rejected; the empty default validates.
- [ ] **Step 3: Run** `cargo test -p ptrack-core scratchpad` → FAIL (compile).
- [ ] **Step 4: Implement** the structs, `RecordKind::Scratchpad = 14`, `NativeRecord::Scratchpad`, `kind()` arm, `encode_scratchpad`/`decode_scratchpad` (body: `frame` a sub-writer holding `text`, `revision`, `updated_at`, snippet count, then each snippet as its own framed body `id, text, pinned, created_at` — use the same primitive writer/reader helpers `encode_meta` uses), the decode arm returning `CodecError` for `payload_schema < 7`, and `Validate` with the six rules above (error paths named `scratchpad.text`, `scratchpad.snippets`, `scratchpad.snippets[i].text`, `scratchpad.snippets[i].id`).
- [ ] **Step 5: Run** `cargo test -p ptrack-core` → PASS. Run `cargo clippy -p ptrack-core --all-targets -- -D warnings`.
- [ ] **Step 6: Commit** `feat(core): scratchpad record with framed schema 7 codec`.

### Task 2: Store collection and project API

**Files:**
- Modify: `crates/ptrack-store/src/schema.rs` (`Collection` :126, `ALL_COLLECTIONS` :140 → 14, `name()` :176 → `"scratchpad"`, `store_kind()` :202, `legacy_codec()` :221, `accepted_codec()` :240, `accepted_payload_schemas()` :276 → exactly `7..=NATIVE_PAYLOAD_SCHEMA`, `is_sequenced()` :290 → false, `key_kind()` :305 → Singleton, table constant near :340 named `PROJECT_SCRATCHPAD_TABLE = "ptrack.project.scratchpad"`)
- Modify: `crates/ptrack-store/src/validation.rs:70` (collection → `RecordKind::Scratchpad`)
- Modify: `crates/ptrack-store/src/typed.rs` (add `impl StoredRecord for Scratchpad` mirroring `Meta` at :111)
- Modify: `crates/ptrack-store/src/project.rs` (add `pub fn scratchpad(&self) -> StoreResult<Scratchpad>` next to `meta()` :367 and `pub fn set_scratchpad(&self, expected_revision: u64, mut value: Scratchpad, now: Timestamp) -> StoreResult<Scratchpad>`)
- Test: `crates/ptrack-store/src/schema_test.rs:9` (14), `crates/ptrack-store/src/project_test.rs`

**Interfaces:**
- Consumes: Task 1 types.
- Produces:
  ```rust
  impl ProjectStore {
      /// Missing record reads as `Scratchpad::default()`.
      pub fn scratchpad(&self) -> StoreResult<Scratchpad>;
      /// Fails with `StoreError::ScratchpadConflict { stored: Scratchpad }` when
      /// `expected_revision != stored.revision`; otherwise validates `value`,
      /// sets `revision = stored.revision + 1`, `updated_at = now`, writes, returns it.
      pub fn set_scratchpad(&self, expected_revision: u64, value: Scratchpad, now: Timestamp) -> StoreResult<Scratchpad>;
  }
  ```
  Add the `ScratchpadConflict { stored: Box<Scratchpad> }` variant to `crates/ptrack-store/src/error.rs` with a Display message `scratchpad revision conflict`.

- [ ] **Step 1: Write failing tests** in `project_test.rs` using the existing project test support: fresh store reads default (revision 0); `set_scratchpad(0, …)` returns revision 1 and `scratchpad()` reads it back byte-equal; `set_scratchpad(0, …)` again fails with `ScratchpadConflict` whose `stored.revision == 1`; oversized text fails validation and leaves the stored record untouched. Update `schema_test.rs` expected length to 14 and the exact collection set.
- [ ] **Step 2: Run** `cargo test -p ptrack-store scratchpad schema` → FAIL.
- [ ] **Step 3: Implement** the schema arms, table, `StoredRecord`, the two methods (use `typed::get_write`/`typed::put` inside a write transaction like `update_meta` :505), and the error variant.
- [ ] **Step 4: Run** `cargo test -p ptrack-store` and `cargo clippy -p ptrack-store --all-targets -- -D warnings` → PASS.
- [ ] **Step 5: Commit** `feat(store): scratchpad singleton collection with revision fencing`.

### Task 3: Application port and GUI commands

**Files:**
- Modify: `crates/ptrack-app/src/service.rs` (`ApplicationPort` trait :556-590: add two methods with fail-closed defaults; `UnavailableApplication` needs nothing extra if defaults are used)
- Modify: `crates/ptrack-app/src/production.rs` (implement both by delegating to the project store; find where `snapshot()` obtains the store)
- Modify: `crates/ptrack-app/src/desktop_runtime.rs` (`COMMANDS` :74 add both names in sorted position, bump the array length; handlers in `BoundDesktopWorkspace::invoke` next to `"AddTaskNote" | "AddTaskNoteV2"` :4004)
- Modify: `crates/ptrack-app/src/error.rs` or wherever `AppError` lives (a structured conflict error the bridge can serialize: message `scratchpad revision conflict` plus the stored record as JSON under `stored`)
- Test: `crates/ptrack-app/src/desktop_runtime_test.rs` (exact allowlist :940 and arity test :1102), `crates/ptrack-app/src/production_test.rs` or the service test file that exercises mutations
- Modify: `docs/rust-parity-matrix.md` (two `GUI-…` rows after the highest existing id, same column format as :616)
- Check: `src-tauri/src/main_test.rs`, `src-tauri/tests/security_contract.rs` — update any frozen command list.

**Interfaces:**
- Consumes: Task 2 `scratchpad()` / `set_scratchpad()`.
- Produces (bridge contract):
  - `GetScratchpadV1(generation)` → `{ "generation": u64, "scratchpad": { "text", "snippets": [{ "id", "text", "pinned", "createdAt" }], "revision", "updatedAt" } }`; timestamps are unix milliseconds as numbers.
  - `SetScratchpadV1(generation, revision, scratchpad)` → `{ "generation", "revision" }` where `scratchpad` is the same JSON shape (`revision`/`updatedAt` inside it ignored). On conflict the error message is exactly `scratchpad revision conflict` and the error JSON carries `stored` in the shape above.
  - Port methods: `fn scratchpad(&mut self) -> AppResult<Scratchpad>` and `fn set_scratchpad(&mut self, expected_revision: u64, value: Scratchpad) -> AppResult<Scratchpad>` (the port stamps `now`).

- [ ] **Step 1: Write failing tests**: allowlist fixtures include both names (sorted); `GetScratchpadV1` on a fresh workspace returns revision 0 and empty text; `SetScratchpadV1(gen, 0, {...})` then `GetScratchpadV1` returns the text and revision 1; stale `revision` → error message `scratchpad revision conflict` with `stored.revision == 1` in the error payload; wrong generation is rejected like `AddTaskNoteV2`; over-limit text is rejected with a validation message.
- [ ] **Step 2: Run** `cargo test -p ptrack-app scratchpad allowlist` → FAIL.
- [ ] **Step 3: Implement** port defaults (`Err(unavailable())`), production delegation (`Timestamp::now()` or the crate's clock helper), the two handlers (`u64_arg` for generation and revision, `serde_json::from_value::<ScratchpadV1Json>` for the payload, camelCase JSON via a small serde struct pair), the conflict error mapping, and the docs rows.
- [ ] **Step 4: Run** `cargo test --workspace --all-targets --no-fail-fast`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check` → PASS.
- [ ] **Step 5: Commit** `feat(app): GetScratchpadV1 and SetScratchpadV1 commands`.

### Task 4: Frontend pure module

**Files:**
- Create: `frontend/src/terminal/scratchpad.ts`
- Test: `frontend/src/terminal/scratchpad.test.ts`

**Interfaces:**
- Produces:
  ```ts
  export interface ScratchpadSnippet { id: number; text: string; pinned: boolean; createdAt: number }
  export interface Scratchpad { text: string; snippets: ScratchpadSnippet[]; revision: number; updatedAt: number }
  export const scratchpadTextMaxBytes = 65_536;
  export const scratchpadSnippetMaxBytes = 4_096;
  export const scratchpadMaxSnippets = 50;
  export const emptyScratchpad = (): Scratchpad
  export type SnippetAddResult =
    | { ok: true; snippets: ScratchpadSnippet[] }
    | { ok: false; reason: "empty" | "too-large" | "all-pinned" };
  /** Trim-empty → "empty"; > 4096 bytes → "too-large"; equal text moves to top keeping id/pinned/createdAt; over 50 evicts the oldest unpinned; all pinned → "all-pinned". */
  export function addSnippet(snippets: readonly ScratchpadSnippet[], text: string, now: number): SnippetAddResult
  export function togglePinned(snippets: readonly ScratchpadSnippet[], id: number): ScratchpadSnippet[]
  export function removeSnippet(snippets: readonly ScratchpadSnippet[], id: number): ScratchpadSnippet[]
  /** Pinned first, then by createdAt descending, stable. */
  export function orderedSnippets(snippets: readonly ScratchpadSnippet[]): ScratchpadSnippet[]
  /** First non-empty line, whitespace collapsed, at most 80 chars with "…". */
  export function snippetPreview(text: string): string
  export function clampScratchpadWidth(width: number, bodyWidth: number): number  // 240..max(240, bodyWidth/2), NaN → 320
  export function readScratchpadOpen(storage: Pick<Storage, "getItem">): boolean
  export function writeScratchpadOpen(storage: Pick<Storage, "setItem">, open: boolean): void
  export function readScratchpadWidth(storage: Pick<Storage, "getItem">): number
  export function writeScratchpadWidth(storage: Pick<Storage, "setItem">, width: number): void
  export function utf8ByteLength(text: string): number
  export const scratchpadNotices = { tooLarge: "Selection is larger than 4 KB; not added to the scratchpad.", allPinned: "All 50 snippets are pinned; unpin one to add more.", reloaded: "Scratchpad changed elsewhere and was reloaded.", saveFailed: "Save failed" } as const;
  ```
  Ids: `addSnippet` assigns `max(existing ids) + 1` (1 when empty).

- [ ] **Step 1: Write failing tests** covering every rule in the interface comment, including: byte length counts UTF-8 (a 3-byte character × 1 366 = 4 098 bytes → too-large), dedupe keeps id and pinned, eviction skips pinned, ordering, preview ellipsis, width clamp bounds, storage read/write with a throwing storage (must not throw; defaults apply).
- [ ] **Step 2: Run** `cd frontend && npx vitest run src/terminal/scratchpad.test.ts` → FAIL.
- [ ] **Step 3: Implement** `scratchpad.ts` (use `new TextEncoder().encode(text).length` for bytes; wrap storage in try/catch).
- [ ] **Step 4: Run** the test → PASS.
- [ ] **Step 5: Commit** `feat(terminal): scratchpad snippet rules and layout helpers`.

### Task 5: Bridge, markup, styles, and dock wiring

**Files:**
- Modify: `frontend/src/tauri-bridge.js` `COMMANDS` (insert `"GetScratchpadV1"` after `"GetRecentProjectsV1"`, `"SetScratchpadV1"` after `"SetPreferences"`), `frontend/src/tauri-bridge.test.js` exact list
- Modify: `frontend/index.html` (`#terminal-body` :876-945: keep `#terminal-search`, wrap `#terminal-host` + `#terminal-message` in `<div id="terminal-stage" class="terminal-stage">`, then add the splitter and panel; toolbar utilities group :760: add the toggle button before `#terminal-diagnostics-toggle`)
- Modify: `frontend/src/style.css` (`.terminal-body` :5142 becomes `display: flex; flex-direction: row;` with `.terminal-stage { position: relative; flex: 1 1 auto; min-width: 0; }`; new `.terminal-scratchpad-splitter`, `.terminal-scratchpad`, header, textarea, strip, row, action styles; hidden state via `[hidden]`)
- Modify: `frontend/src/terminal/pane.ts` (`TerminalDock`: element fields, listeners, `#scratchpad` state, capture hook in `#copySelection` :1663 success path, refit on toggle/resize, generation change reload, dispose flush)
- Modify: `frontend/src/build.test.js` (markup + CSS assertions), `docs/help/terminals/index.html` (one paragraph "Scratchpad"), `docs/help/assets/screenshots/manifest.json` digest + review line

**Markup (inside `#terminal-body`, after `#terminal-search`):**
```html
<div id="terminal-stage" class="terminal-stage">
  <div id="terminal-host" class="terminal-host" aria-label="Terminal session"></div>
  <div id="terminal-message" class="terminal-message" data-terminal-overlay hidden></div>
</div>
<div id="terminal-scratchpad-splitter" class="terminal-scratchpad-splitter" role="separator" tabindex="0"
     aria-label="Resize scratchpad" aria-orientation="vertical" aria-valuemin="240" aria-valuenow="320" hidden></div>
<aside id="terminal-scratchpad" class="terminal-scratchpad" aria-label="Scratchpad" data-terminal-overlay hidden>
  <div class="terminal-scratchpad-header">
    <p class="eyebrow">Scratchpad</p>
    <span id="terminal-scratchpad-state" class="terminal-scratchpad-state" role="status" aria-live="polite">Saved</span>
    <button id="terminal-scratchpad-close" class="terminal-scratchpad-close" type="button" aria-label="Hide scratchpad" title="Hide scratchpad"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 4 8 8M12 4l-8 8"></path></svg></button>
  </div>
  <textarea id="terminal-scratchpad-text" class="terminal-scratchpad-text" aria-label="Project scratchpad" maxlength="65536" spellcheck="false" placeholder="Notes for this project…"></textarea>
  <div class="terminal-scratchpad-strip-header">
    <p class="eyebrow">Clipboard</p>
    <button id="terminal-scratchpad-add" class="terminal-scratchpad-add" type="button" disabled>Add selection</button>
  </div>
  <ul id="terminal-scratchpad-snippets" class="terminal-scratchpad-snippets" aria-label="Copied snippets"></ul>
  <p id="terminal-scratchpad-empty" class="terminal-scratchpad-empty">Copy from a pane, or add a selection.</p>
</aside>
```
Toolbar toggle (utilities group, before diagnostics):
```html
<button id="terminal-scratchpad-toggle" class="terminal-action-button" type="button" aria-label="Show scratchpad" title="Show scratchpad" aria-pressed="false" aria-controls="terminal-scratchpad">
  <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 2.5h8.5v11H4z"></path><path d="M2.5 5h1.5M2.5 8h1.5M2.5 11h1.5M6.5 6h4M6.5 8.5h4"></path></svg>
</button>
```
Snippet row (built in TS): `<li class="terminal-scratchpad-snippet" data-pinned>` with `<span class="terminal-scratchpad-preview">` and four `button.terminal-scratchpad-action` (Copy / Paste / Pin|Unpin / Delete) using `terminalControlIcon` where a glyph exists (`close` for Delete) and short text labels otherwise.

**Dock wiring (`pane.ts`), in order:**
1. Fields: `#scratchpadToggle`, `#scratchpad`, `#scratchpadSplitter`, `#scratchpadState`, `#scratchpadClose`, `#scratchpadText`, `#scratchpadAdd`, `#scratchpadList`, `#scratchpadEmpty`, `#stage`; state `#scratchpadOpen`, `#scratchpadWidth`, `#scratchpadRecord: Scratchpad`, `#scratchpadLoaded = false`, `#scratchpadSaving = false`, `#scratchpadScheduler: WorkspacePersistenceScheduler`.
2. Backend interface: add `GetScratchpadV1(generation: number): Promise<{ generation: number; scratchpad: Scratchpad }>` and `SetScratchpadV1(generation: number, revision: number, scratchpad: Scratchpad): Promise<{ generation: number; revision: number }>` to `TerminalBackend`.
3. `#setScratchpadOpen(open, persist = true)`: sets `hidden` on aside and splitter, `aria-pressed`, labels, writes localStorage, applies width via `this.#scratchpad.style.width`, then `this.#fitPanes(this.#activeTabPaneIds())` on the next frame. On first open with a generation, calls `#loadScratchpad()`.
4. `#loadScratchpad()`: `GetScratchpadV1(this.#workspaceGeneration)`; ignore the result if the generation moved; set `#scratchpadRecord`, textarea value (only if the textarea is not focused mid-edit, else keep local text and mark dirty), `#renderSnippets()`.
5. Textarea `input` → `#scratchpadRecord.text = value; #scratchpadScheduler.markDirty(); state "Saving…"`.
6. `#saveScratchpad()` (the scheduler's write): guard `#scratchpadSaving`; call `SetScratchpadV1(generation, record.revision, record)`; on success set `record.revision = result.revision`, state "Saved"; on error message `scratchpad revision conflict`: put the local text on the clipboard via `nativeClipboard().setText` (ignore failure), replace record with `error.stored` if present else reload, state = `scratchpadNotices.reloaded`; other errors: state "Save failed", `this.#showError(error)`, keep dirty for the next edit.
7. Snippet actions: Copy → `nativeClipboard().setText`; Paste → reuse `prepareClipboardPaste`/`commitClipboardPaste` with the snippet text against the active running runtime (same guard as `#requestNativePaste`, but text comes from the snippet); Pin → `togglePinned`; Delete → `removeSnippet`; each mutation sets the record, renders, marks dirty.
8. Capture: at the end of a successful `#copySelection`, `const result = addSnippet(record.snippets, text, Date.now())`; ok → update, render, markDirty; `too-large`/`all-pinned` → show the notice in `#scratchpadState` (auto-clears on next save). `#scratchpadAdd` does the same from `resources.terminal.getSelection()`; its `disabled` tracks `terminal.hasSelection()` via `onSelectionChange` of the active pane.
9. Splitter: pointer drag and ArrowLeft/ArrowRight (16 px) update `#scratchpadWidth` through `clampScratchpadWidth(width, this.#body.clientWidth)`, write localStorage, set `aria-valuenow`, refit panes.
10. Generation change (`setGeneration`/wherever `#workspaceGeneration` is assigned): flush the scheduler for the old generation, reset record to `emptyScratchpad()`, clear the textarea, then load if open.
11. `dispose()`: `#scratchpadScheduler.dispose()`.
12. `#renderPanelVisibility`: nothing extra — the aside hides with the dock.

- [ ] **Step 1: Write failing assertions** in `build.test.js`: index contains `id="terminal-scratchpad-toggle"` with `aria-pressed="false"`, `id="terminal-scratchpad"` as `<aside` with `hidden`, `id="terminal-scratchpad-text"` with `maxlength="65536"`; styles contain `.terminal-body {` … `display: flex` and `.terminal-scratchpad {`; app bundle contains `"GetScratchpadV1"`, `"SetScratchpadV1"`, and the `Copy from a pane, or add a selection.` copy. Update `tauri-bridge.test.js`.
- [ ] **Step 2: Run** `cd frontend && npm test` → FAIL on the new assertions only.
- [ ] **Step 3: Implement** markup, CSS, bridge, and dock wiring as specified. Keep every new pure decision in `scratchpad.ts`; `pane.ts` only wires DOM and backend.
- [ ] **Step 4: Run** `cd frontend && npm test` and `npm run build` → PASS. Refresh the manifest digest and append a `2026-09-17:` review line. Run `python3 -B tools/help_check.py all` → PASS.
- [ ] **Step 5: Commit** `feat(terminal): scratchpad panel with clipboard strip`.

### Task 6: Packaged verification

**Files:** none (verification only; fixes go into the task that owns the file).

- [ ] **Step 1:** Preflight (no foreign cargo/rustc/make), then `make package CARGO_TARGET_DIR=$PWD/target`; copy the bundle to a fresh scratch directory before launching (never launch or overwrite a recently-run path).
- [ ] **Step 2:** Open a real project → Board → start a terminal → toggle scratchpad: type a note, wait, restart the app, note is back. Copy a selection from the pane: a snippet appears; Paste it back; Pin it; Delete one. Toggle panel closed and open: panes refit (no clipped prompt). Switch project: panel shows that project's note.
- [ ] **Step 3:** Capture one screenshot of the open panel for the PR description. Record any defect and fix it under the owning task before declaring done.
