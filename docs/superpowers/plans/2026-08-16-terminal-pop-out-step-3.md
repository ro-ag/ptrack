# Terminal Pop-Out Step 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A tab — split tree, sessions, title, association — moves whole into a terminal window and comes back whole, with state meaning the same thing in every window.

**Architecture:** The in-memory assignment map holds the tab's sessions and serialized shape. The bridge gains one command (`SetTerminalWindowTab`) and renames one (`GetTerminalWindowSession` → `GetTerminalWindowTab`); `OpenTerminalWindow` takes the session list and shape. The main window releases every pane lease, opens the window, and holds the tab; the terminal window renders the split tree with per-pane claims and pushes shape changes back into the assignment. Pop-in on destroy returns the whole tab through the existing `terminal:window-closed` event, now carrying the tab.

**Tech Stack:** Rust (ptrack-app, src-tauri/Tauri 2), TypeScript + vitest (frontend), xterm.js.

## Global Constraints

- Contract: `docs/superpowers/specs/2026-08-16-terminal-pop-out-step-3.md`; step-1/2 contracts stay binding except step 2 §7's single-pane gate.
- Capability file permission array stays exactly `core:event:allow-listen` + `core:event:allow-unlisten`; windows stay `["main", "terminal-*"]`.
- Assignment map stays in-memory, per run, never persisted.
- No failure may leave any session unowned; tab moves are all-or-nothing.
- Rust unit tests live in Go-style sibling `_test.rs` files.
- Frozen command-allowlist fixtures (desktop_runtime.rs list, security contract) are updated deliberately, never loosened.
- `make test` and `cargo test --workspace` green before merge; no Co-Authored-By/AI attribution anywhere.

---

### Task 1: Assignment map holds a tab

**Files:**
- Modify: `crates/ptrack-app/src/terminal_windows.rs`
- Test: `crates/ptrack-app/src/terminal_windows_test.rs`

**Interfaces:**
- Produces:
  - `pub struct TerminalWindowTab { pub sessions: Vec<String>, pub shape: serde_json::Value }`
  - `open(&mut self, fence: Option<u64>, tab: TerminalWindowTab) -> AppResult<String>` — validates: fence present, ≥1 session, no empty session id, no duplicate session ids inside the tab, no session already assigned to any window, window limit.
  - `tab(&self, label: &str) -> Option<&TerminalWindowTab>`
  - `set_tab(&mut self, label: &str, tab: TerminalWindowTab) -> AppResult<()>` — unknown label errors; duplicate-session checks run against *other* windows only.
  - `close(&mut self, label: &str) -> Option<TerminalWindowTab>`
  - `expire`/`drain` unchanged signatures (labels out; assigned sessions are simply dropped with them).

- [ ] Rewrite tests: open with two sessions + shape round-trips through `tab()`; open rejects empty list, duplicate inside tab, session owned by another window; `set_tab` replaces shape and sessions, rejects unknown label and cross-window duplicates; `close` returns the whole tab; limit and fence behavior unchanged.
- [ ] Run: `cargo test -p ptrack-app terminal_windows` — expect FAIL (compile).
- [ ] Implement `TerminalWindowTab`, switch `assigned: BTreeMap<String, TerminalWindowTab>`, adjust validations (helper `fn duplicate(&self, skip: Option<&str>, sessions: &[String]) -> bool`).
- [ ] Run same tests — PASS. Commit `feat(terminal): assignment map carries a whole tab`.

### Task 2: Bridge commands for tab windows

**Files:**
- Modify: `crates/ptrack-app/src/desktop_runtime.rs` (COMMANDS list; `application_state` dispatch; command fns around line 3650)
- Test: `crates/ptrack-app/src/desktop_runtime_test.rs`, `src-tauri/tests/security_contract.rs`, `src-tauri/src/main_test.rs` (frozen allowlist fixtures)

**Interfaces:**
- Consumes: Task 1 types.
- Produces bridge methods:
  - `OpenTerminalWindow(sessions: string[], shape: object)` → `{ "label": string }`
  - `GetTerminalWindowTab(label: string)` → `{ "sessions": string[], "shape": object } | { "sessions": null, "shape": null }` (stale label)
  - `SetTerminalWindowTab(label: string, sessions: string[], shape: object)` → `{}`
  - Rust: `open_terminal_window(&self, sessions: Vec<String>, shape: Value) -> AppResult<String>`, `terminal_window_tab(&self, label) -> Option<TerminalWindowTab>`, `set_terminal_window_tab(&self, label, sessions, shape) -> AppResult<()>`, `close_terminal_window(&self, label) -> Option<TerminalWindowTab>`.
- `GetTerminalWindowSession` is removed from the allowlist; the command count and frozen fixtures change accordingly.

- [ ] Update runtime tests: open with list+shape mints label; get returns both; set replaces; argument-shape errors exact; fence expiry drops tabs. Update frozen allowlist fixtures (count +1: one rename, one addition).
- [ ] Run: `cargo test -p ptrack-app desktop_runtime` and `cargo test -p ptrack-desktop --test security_contract` — FAIL first, then implement, then PASS.
- [ ] Commit `feat(terminal): bridge speaks whole-tab window assignments`.

### Task 3: Shell pop-in returns the tab

**Files:**
- Modify: `src-tauri/src/main.rs` (`pop_in_terminal_window` ~line 225; `OpenTerminalWindow` intercept ~line 65 unchanged in shape)
- Test: `src-tauri/src/main_test.rs`

**Interfaces:**
- Produces event `terminal:window-closed` payload: `{ "label": string, "sessions": string[], "shape": object }`.

- [ ] Update `pop_in_terminal_window` to emit the full tab from `close_terminal_window`. Update main_test expectations.
- [ ] `cargo test -p ptrack-desktop` PASS. Commit `feat(terminal): window close hands the whole tab back`.

### Task 4: Tab-level pop-out helpers

**Files:**
- Modify: `frontend/src/terminal/pop-out.ts`, `frontend/src/terminal/pop-out.test.ts`

**Interfaces:**
- Produces:
  - `terminalPopOutControl(input: { panes: ReadonlyArray<{ state: PaneRuntimeState; hasSession: boolean }>; busy: boolean; closing: boolean })` — present when every pane is `running` with a session (≥1 pane), disabled while busy/closing.
  - `popOutTab(steps: { releaseAll(): Promise<ReadonlyArray<{ paneId: string; sessionId: string }>>; open(released: ReadonlyArray<{ paneId: string; sessionId: string }>): Promise<{ label: string }>; reclaimAll(): Promise<void> }): Promise<TabPopOutResult>` with `outcome: "popped-out" | "kept" | "unowned"` — any failure reclaims everything; reclaim failure is `unowned`.
  - `heldTabCloseRefused` reuses `poppedOutCloseRefusedNotice`.
- Existing single-pane `popOutTerminal` is deleted; `panesHoldPoppedOutTerminal` keeps its signature (holders are now all pane ids of held tabs).

- [ ] Write failing tests (presence gate incl. mixed pane states; all-or-nothing on open failure and on reclaim failure). Run `npm test -- pop-out` — FAIL.
- [ ] Implement; PASS. Commit `feat(terminal): tab-level pop-out orchestration`.

### Task 5: Main window moves and holds tabs

**Files:**
- Modify: `frontend/src/terminal/pane.ts` (`#popOutPane` → `#popOutTab` ~line 1051; `#poppedOut` becomes `Map<sessionId, paneId>` per held tab plus `#heldTabs: Map<tabId, string[]>`; `#popTerminalBackIn` ~line 1216 restores every pane; render gating ~2491-2544; close refusal ~2624)
- Modify: `frontend/src/app.js` (event payload pass-through), `frontend/index.html` if control markup moves
- Test: `frontend/src/terminal/pane.test.ts`

**Interfaces:**
- Consumes Task 2 bridge methods and Task 4 helpers.
- Behavior: pop-out serializes the active tab's `WorkspaceTab` (shape) and per-pane sessions in pane order; every pane of a held tab renders the held notice; close of any held pane/tab refused with existing copy; `terminal:window-closed` restores each session into its recorded pane and re-claims from sequence 0 per pane (ring replays what it kept, gap per pane).

- [ ] Extend fake-bridge pane tests: two-pane split pops out whole (both leases released, one `OpenTerminalWindow([s1,s2], shape)` call); open failure re-claims both; window-closed restores both; held-tab close refusal.
- [ ] Implement; `npm test -- pane` PASS. Commit `feat(terminal): main window moves whole tabs out and back`.

### Task 6: Terminal window renders the split tree

**Files:**
- Modify: `frontend/src/app.js` (`startTerminalWindow` ~line 7741), `frontend/index.html` (terminal-window section hosts a split container)
- Create: `frontend/src/terminal/window-tab.ts` + `frontend/src/terminal/window-tab.test.ts`

**Interfaces:**
- Produces `window-tab.ts`:
  - `renderWindowTab(host: HTMLElement, shape: WorkspaceTab, mount: (paneId: string, container: HTMLElement) => void): void` — walks `PaneNode` (reuse `frontend/src/workspace/model.ts` types), builds nested flex containers with the tab's ratios and directions, calls `mount` per terminal pane.
  - `applySplitResize(shape: WorkspaceTab, splitId: string, ratio: number): WorkspaceTab` (clamped to model min/max).
- `startTerminalWindow` claims per pane (existing single-pane attach/reclaim loop extracted into `attachWindowPane(...)` used once per pane), per-pane gap notices, and pushes shape changes through `SetTerminalWindowTab` after a resize (debounced 300ms). Splitting new panes inside the child window is deferred to a later step if `OpenTerminal` wiring proves nontrivial — deferral is stated in the PR, not silent.

- [ ] Write window-tab tests: two-pane vertical shape renders two mounts with 0.3/0.7 flex; resize clamps; degenerate single-pane tree renders one mount.
- [ ] Implement; `npm test -- window-tab` PASS.
- [ ] Wire `startTerminalWindow`; `make frontend-test` PASS. Commit `feat(terminal): terminal window renders the tab's split tree`.

### Task 7: State sync across windows

**Files:**
- Modify: `frontend/src/app.js` (terminal-window mode subscribes: `terminal:status`, `terminal:exit` per session — already; plus profile-settings/theme refresh on `workspace:data-changed`), `src-tauri/src/main.rs` (verify capability prompts and menu raise main window — no change expected, assert in test)
- Test: `frontend/src/build.test.js` or new assertions in `frontend/src/tauri-bridge.test.js`

**Interfaces:** consumes existing broadcast events only; no new permissions.

- [ ] Add tests: terminal-window mode re-reads normalized profile settings on data-change event; window title set to `Terminal — <tab title>`; association shown read-only from shape.
- [ ] Implement; PASS. Commit `feat(terminal): terminal windows track shared state`.

### Task 8: Full verification

- [ ] `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `make test` — all green.
- [ ] Manual (stated as manual, recorded as a plan-15 note): pop out a two-pane split with running output, type in both windows, resize the child split, close the child, confirm both shells alive and the resized split intact.
- [ ] PR + squash merge; update ptrack tasks #139/#142/#143 notes.
