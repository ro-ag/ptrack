# Terminal scratchpad and clipboard strip

Status: implementation contract, approved 2026-09-17. Adds a per-project scratchpad
panel to the embedded terminal dock. Every terminal contract in `2026-07-25-embedded-
terminal-design.md` and the pop-out contracts stays binding; this document only adds.

**This step must ship a feature the user can operate:** open the panel beside the
terminal panes, type notes that survive tab churn and app restarts, and keep the text
copied out of terminal panes as re-usable snippets that paste back into a pane.

## 1. Content model

One scratchpad per project. It is a project record, stored in the project store beside
plans and notes, never in git and never in the terminal layout descriptor.

```text
Scratchpad
  text        markdown, at most 65 536 bytes (UTF-8)
  snippets    at most 50 entries, newest first
  revision    u64, starts at 0, +1 per accepted write
  updated_at  unix milliseconds of the last accepted write

ScratchpadSnippet
  id          u64, unique within the scratchpad, monotonic
  text        at most 4 096 bytes (UTF-8), never empty after trimming
  pinned      bool
  created_at  unix milliseconds
```

The content-free rule of the terminal contract holds: a snippet is created only by an
explicit user act — a copy from a pane (⌘C / Ctrl+Shift+C, or the context-menu Copy) or
the panel's **Add selection** button. p-track never scrapes terminal output into the
scratchpad and never persists scrollback.

Snippet list rules (pure, tested in the frontend):

- Adding text equal to an existing snippet's text moves that snippet to the top and keeps
  its `id`, `pinned`, and `created_at`; nothing is duplicated.
- Adding beyond 50 evicts the oldest **unpinned** snippet. If all 50 are pinned the add is
  refused with a notice; nothing is evicted.
- Text longer than 4 096 bytes is refused with a notice, never truncated.
- Text that is empty after trimming is ignored silently.

## 2. Persistence

No new store collection. The database validator demands an exact table catalog and
an exact `STORE_SCHEMA_VERSION`, and no in-place upgrade path exists (see
`2026-09-07-stack-discovery-design.md` §Records), so a new collection would refuse to
open every project database written by an earlier build. Instead:

- The scratchpad is an additive field `Meta.scratchpad: Option<Scratchpad>` on the
  project's singleton `Meta` record, written only at a new payload schema 8
  (`NATIVE_PAYLOAD_SCHEMA` 7 → 8). Schema 8 `Meta` is the exact schema 7 layout followed
  by one trailing length-framed body holding the scratchpad (`None` representable), the
  same mechanism that introduced `Meta.stack` at schema 5. The schema 7 layout stays
  byte-identical; decoding `Meta` at any schema ≤ 7 yields `scratchpad: None`; every
  other record kind encodes identically at 7 and 8.
- Validation fails closed on: text over 65 536 bytes, more than 50 snippets, any snippet
  over 4 096 bytes or empty after trimming, duplicate snippet ids. `Meta` validation
  includes the scratchpad when present. Caps violations surface as the store's
  validation error class, never as a manifest error.
- Golden byte pins: the existing schema 7 `Meta` pin unchanged, plus schema 8 pins for
  `Meta` with and without a scratchpad, and decode-at-schema tests for both.
- Store API: `scratchpad()` reads `Scratchpad::default()` (`text ""`, no snippets,
  `revision 0`) when the field is absent; `set_scratchpad(expected_revision, value, now)`
  fails with a conflict carrying the stored scratchpad when revisions differ, otherwise
  validates, sets `revision + 1` and `updated_at = now`, and writes `Meta` in one
  transaction **without** stamping `Meta.updated_at` or `last_write_version` — the
  scratchpad carries its own clock.
- A database last written at schema 7 opens unchanged, reads an empty scratchpad, and
  accepts the first write. An older build cannot read a `Meta` written at schema 8 (the
  standing fail-closed rule for every payload-schema bump).

## 3. Commands

Two project-scoped V2-style commands (generation as argument 0, `require_generation`):

- `GetScratchpadV1(generation)` → `{ generation, scratchpad }`.
- `SetScratchpadV1(generation, revision, scratchpad)` → `{ generation, revision }`.
  `revision` must equal the stored revision, else the command fails with a
  `ScratchpadConflict` error carrying the stored scratchpad so the caller can reload
  without a second round trip. The stored `revision` becomes `revision + 1`; `updated_at`
  is stamped by the runtime, never trusted from the caller.

The command names join the sorted allowlist in `desktop_runtime.rs`, the bridge `COMMANDS`
list, and every frozen-allowlist test. The parity matrix gains one row per command. The
application port gains `scratchpad()` and `set_scratchpad(expected_revision, value)`
(no mutation-enum variant: `MutationResult` cannot carry the stored record the conflict
path returns).

Out of scope, noted as follow-up: `ptrack scratchpad` CLI read for agents.

## 4. Panel

Placement: inside `#terminal-body`, right of the split host, as a column:
`host | splitter | panel`. Panel width defaults to 320 px, clamped to 240 px … 50 % of the
body, persisted in `localStorage` (`ptrack-terminal-scratchpad-width`). Open state is
persisted in `localStorage` (`ptrack-terminal-scratchpad-open`) and applied before first
paint of the dock.

Toggle: a toolbar button in the utilities group beside diagnostics, `aria-pressed`, icon
"notebook", label "Show scratchpad" / "Hide scratchpad". Menu accelerator: none in v1.

Panel content, top to bottom:

1. Header: eyebrow "Scratchpad", a saved-state hint ("Saved", "Saving…", "Save failed"),
   and a close control.
2. Note: a `textarea` (monospace, `maxlength` 65 536, `spellcheck=false`), `aria-label`
   "Project scratchpad". Autosave through `WorkspacePersistenceScheduler` (250 ms debounce,
   2 s max wait); flushed on panel close, terminal hide, page hide, project switch, and
   dispose.
3. Clipboard strip: eyebrow "Clipboard", an **Add selection** button (enabled only while the
   active pane has a selection), then the snippet list newest first. Each row shows a
   one-line preview (first non-empty line, ellipsised) and four icon actions: Copy (system
   clipboard), Paste (into the active pane through the existing paste guard, so multi-line
   pastes still ask), Pin, Delete. Pinned rows sit above unpinned rows and carry a pin
   marker. Empty state: "Copy from a pane, or add a selection."

Capture: every successful `#copySelection` in the dock also adds the copied text to the
snippet list. The pop-out window neither shows the panel nor captures in v1.

Layout: opening, closing, or resizing the panel refits the active tab's panes. When the
terminal panel is hidden or the workspace view is not visible, the scratchpad is simply
hidden with the dock; its open state is kept. Keyboard: the splitter is a focusable
`separator` with arrow-key resizing, as the dock separator already is.

Project switch: on a generation change the panel flushes pending text for the old
generation (writes fenced by generation are rejected by the runtime if stale), clears, and
loads the new project's scratchpad.

## 5. Errors and limits

- Save failure (`SetScratchpadV1` error other than conflict): hint shows "Save failed",
  text stays as typed, the next edit retries; the dock's error surface shows the message.
- Conflict: reload the stored scratchpad, replace text and snippets, show "Scratchpad
  changed elsewhere and was reloaded". The user's unsaved text is placed on the system
  clipboard first so nothing typed is lost.
- Add refused (too large, all pinned): notice in the panel hint, no change.
- Clipboard unavailable (no native runtime): Copy and Paste actions are disabled with a
  title explaining why; capture still works from the dock's own copy path.

## 6. Verification

- Rust: schema 7 `Meta` pin unchanged and schema 8 pins added; validate caps; schema 7
  database opens and accepts a scratchpad; `Meta.updated_at` untouched by a scratchpad
  write; runtime tests for get/set, conflict payload, generation fencing, allowlist and
  arity.
- Frontend: `terminal/scratchpad.ts` pure module + `scratchpad.test.ts` (add/dedupe/evict/
  pin/refuse rules, width clamp, preview line); `build.test.js` markup and CSS assertions;
  bridge command list test; screenshot manifest digest refresh.
- Manual, stated as manual: packaged app on a real project — type, restart app, note is
  back; copy from a pane, snippet appears; paste it back; pin, evict at 51; switch project.
- `make test` green; no AI attribution in commits or PR.
