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

New singleton store collection `ProjectScratchpad`:

- redb table `ptrack.project.scratchpad`, key kind Singleton, not sequenced.
- Encoded with the native codec at payload schema 7, body length-framed field by field
  from the first byte ever written (per MEMENTO: a schema number is immutable once any
  build writes it; a future layout change takes the next number).
- Validation fails closed on: text over 65 536 bytes, more than 50 snippets, any snippet
  over 4 096 bytes or empty, duplicate snippet ids, invalid UTF-8.
- Golden byte pin and decode-at-schema tests exactly as the other collections have.
- A project store without the record reads as the empty scratchpad (`text ""`, no
  snippets, `revision 0`); the first write creates it.

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
mutation enum gains `SetScratchpad`.

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

- Rust: store schema set test updated (14 collections); golden byte pin; validate caps;
  runtime tests for get/set, conflict payload, generation fencing, allowlist and arity.
- Frontend: `terminal/scratchpad.ts` pure module + `scratchpad.test.ts` (add/dedupe/evict/
  pin/refuse rules, width clamp, preview line); `build.test.js` markup and CSS assertions;
  bridge command list test; screenshot manifest digest refresh.
- Manual, stated as manual: packaged app on a real project — type, restart app, note is
  back; copy from a pane, snippet appears; paste it back; pin, evict at 51; switch project.
- `make test` green; no AI attribution in commits or PR.
