# Terminal pop-out window contract (plan #15)

Status: contract for tasks #135–#145. Frozen surface for implementation and review.
Amends `2026-08-15-desktop-state-restoration-contract.md` §5 (see §8 below).

## 0. Naming

**`detach` already means association-detach** in this codebase — `MutateTerminalAssociationV2(..., detach: bool, ...)` unlinks a terminal from a plan or task (parity `GUI-047`). That meaning is unchanged and keeps the word.

The window feature is **pop out** / **pop in**. A terminal is *popped out* to a terminal window and *popped back in* to the main window. No user-facing string, command name, event name, or identifier in this feature may use "detach" or "redock".

## 1. Ownership model

The popped-out window is an **independent top-level window**, not a platform child. It can sit behind the main window, live on another display or Space, and survive the main window being minimized. It is *dependent* only in the logical sense: the app owns it, and it closes when the project closes or the app quits.

`WebviewWindowBuilder::parent()` is deliberately **not** used — on macOS it forces the child above the parent and hides it with the parent, which contradicts the above.

| Concern | Owner |
| --- | --- |
| PTY sessions and the terminal `Manager` | Rust, one manager per project root, unchanged |
| Which renderer may attach to a session | Rust, one lease per session (§3) |
| Window geometry for every window | Rust, `window-state` record keyed by window (§7) |
| `layout-state`, `preferences`, theme, shutdown | Rust, main window is the authority (§7) |
| Workspace descriptor (tabs, splits, panes) | Rust, single fenced command (§8) |

## 2. Window identity and capability

- The main window keeps label `main`. Terminal windows use labels matching **`terminal-*`**, minted as `terminal-<n>` with a monotonic counter, stable for the window's lifetime.
- `src-tauri/capabilities/main-window.json` widens `"windows"` to `["main", "terminal-*"]`. Nothing else in that file changes: the permission array stays exactly `core:event:allow-listen` + `core:event:allow-unlisten`, and the security test's assertions that no `window`, `webview`, `menu`, `tray`, `image`, or `allow-emit` permission appears **stay verbatim**. Widening the label list is the whole change; the guarantee the test protects is untouched.
- `AppHandle::add_capability` is **forbidden** — it would inject permissions the on-disk file does not describe, making the security test stop reflecting reality.
- Terminal windows are created at **runtime** via `WebviewWindowBuilder`, never declared in `tauri.conf.json`, so the assertion that the config declares exactly one window stays true.
- The window is created from a `DesktopRuntime` command through `gui_invoke`, keeping the Tauri command count at exactly **3**. Because `WebviewWindowBuilder::build` deadlocks if called from a synchronous command on Windows, and `gui_invoke` runs on `spawn_blocking`, the build must be dispatched with `run_on_main_thread`.
- Terminal windows load the existing `index.html` with a mode marker in the URL fragment. **No second Vite entry point**, so the fixed `app.js` / `style.css` output names and the build contract test are unaffected.

## 3. Session lease

Today a session has one destructive attachment: a second attach returns 409, and — the load-bearing defect — **the stream ending closes the PTY** (`stream.rs`), so releasing a renderer kills the shell. Task #137 is therefore not "add a lease"; it is "separate releasing a renderer from terminating a session".

- Exactly **one renderer lease** per session at any moment. A second attach while a lease is held is refused, and the refusal must **never** close the session.
- **Release ≠ terminate.** Releasing a lease (pop out, pop in, window closed, page hidden) leaves the PTY running. Only an explicit `CloseTerminal`, the shell exiting, project switch, or app quit terminates it.
- The unattached-session lease (30s force-close today) becomes a **grace window** during which a session may be re-claimed. A session that is unattached and *not* mid-transfer past the grace window is still closed, so an orphaned session cannot leak forever.
- **The stream token rotates on every release.** Today "single use" is what makes a leaked stream URL harmless; a re-claimable session with a long-lived static token is strictly weaker. Each re-attach requires a freshly minted one-shot ticket from a fenced command.
- Sessions carry a monotonic **lease generation**. Writes and resizes from a stale lease are rejected, not applied.

## 4. Replay

- Each session keeps a **bounded, sequenced, in-memory ring buffer** of recent output. It replaces the current one-shot 64 KiB startup buffer, which is unsequenced and drained on read.
- Budget: **256 KiB per session**, retained in process memory only.
- Every chunk carries a monotonic sequence number. A re-attaching renderer presents the last sequence it rendered; the server replays from that point, or from the oldest retained chunk when the buffer has wrapped, and **tells the renderer that a gap occurred**.
- **The ring buffer is never written to disk**, never persisted, and dies with the session. It is not scanned or redacted: scanning raw PTY bytes at throughput is costly and false positives corrupt legitimate output, and the buffer never leaves memory. This is the same treatment live terminal output already gets.
- When the buffer has wrapped, the popped-out terminal shows an explicit, non-decorative marker that earlier output was not carried over. Silence there would be a lie.

## 5. Moving a terminal (task #139)

**The PTY never restarts and no in-flight output is dropped. Scrollback older than the replay window is not carried over, and the UI says so.**

There is no server-side history today and xterm exposes no supported buffer-transfer API, so full-scrollback transfer would mean building megabyte-scale history capture per terminal. That is explicitly out of scope. The sequence is:

1. Target window is created (or focused) and made ready.
2. Source pane releases its lease, recording its last rendered sequence. The PTY keeps running and output keeps accumulating in the ring.
3. The descriptor for the tab or pane moves between windows through the Rust-owned workspace (§8).
4. Target window claims a fresh ticket, attaches, and replays from the recorded sequence.
5. On failure at any step, the terminal **stays where it was** with its lease re-claimed. A failed move must never leave a session with no owner.

## 6. Pop back in and closing

- Closing a terminal window **pops its sessions back into the main window**; it does not terminate them. This is the default and needs no confirmation.
- Explicit termination stays a separate, explicit action (`CloseTerminal`), unchanged.
- If the main window cannot accept them (project closed, shutting down), the sessions are closed cleanly rather than orphaned.

## 7. Window lifecycle, geometry, and per-window scoping

Every window-event handler is currently **label-blind and app-wide**. Each of the following must match on `window.label()`:

- **`CloseRequested` must not call `begin_shutdown` for a terminal window.** Today it would kill the whole app runtime, leaving the main window a dead shell whose every command fails. Only the main window's close begins shutdown.
- **Geometry capture must not overwrite the main window's rect.** The `window-state` record gains per-window entries keyed by label, keeping the versioned / totally-normalized / single-transaction / never-overwrite-unreadable discipline of plan #14. The capture seal becomes per-window.
- **Theme application must cover every window**, not the hard-coded `main`.
- The exit flush must not depend on `main` still being registered — with two windows, which one is destroyed last varies.
- Terminal windows are closed on project switch and on app quit, and their sessions cleaned up. A crash leaves no state that resurrects a window pointing at a dead session.

## 8. Workspace descriptor ownership — amendment to plan #14 §5

Plan #14 §5 stated the terminal workspace descriptor stays in `localStorage` and that plan "adds no new persistence". **That is no longer safe.** Two windows share one origin and one `ptrack.terminal-workspace:<root>` key, each saving on a 250ms debounce — they would last-writer-wins each other's entire workspace continuously, which is silent data loss.

The descriptor moves to **Rust ownership behind a single fenced command**, following the discipline plan #14 established for `layout-state`. The persisted allowlist itself is unchanged and still excludes buffer contents, session ids, environment, and tokens — that guarantee is preserved, only its storage location and arbitration change.

## 9. Cross-window consistency (task #142)

- **Menu commands must not fire twice.** `Builder::menu` applies one menu to all windows and `app.emit` broadcasts to every webview, so "Open Project…" would open two dialogs today. Menu events target the **focused window** via `emit_to`.
- Association, write-back, and agent-link mutations are already revision-fenced, so the fencing holds; the losing window must show a real message, not a raw stale-revision error.
- Terminal profiles are cached per dock with no invalidation event; a profile change must reach both windows.
- Preference changes must be observed by the other window rather than waiting for a reload.
- A pane moved between windows keeps its linked-agent marker, re-derived from the session rather than from per-dock memory.

## 10. Accessibility and keyboard (task #143)

The terminal window carries the same floors the app already holds: focus management and restore, keyboard traversal, forced-colors support, 320 CSS px and 400% zoom reflow, 3:1 control boundaries, 4.5:1 small text. Focus moves to the terminal on pop-out and returns to a sensible target in the main window on pop-in. Pop-out and pop-in are reachable by keyboard and announced, not mouse-only affordances.

## 11. Verification

- Rust: lease exclusivity, release-does-not-terminate, token rotation, replay sequencing and gap reporting, grace-window expiry, per-window geometry isolation, and that a terminal window's close does not begin shutdown.
- Frontend: pop-out and pop-in during output, splits, search, paste, IME, alternate screen, resize, and renderer loss; failed-move rollback.
- The existing tests that pin *current* semantics — single-use attachment, one-shot startup replay, unattached-lease close — will have to change. Each change must be deliberate and stated, never a quiet relaxation.
- Manual cross-platform acceptance (task #145) on macOS, Windows, and Linux, since window parenting, focus, and menus differ per platform and none of it is provable headlessly.

## 12. Delivery

Too large for one change. Sequenced so each step is independently reviewable and shippable:

1. **Lease and replay** (#137, #138, part of #140) — Rust only, no UI. The load-bearing change.
2. **Window lifecycle** (#136, #141, #143) — creation, label scoping, per-window geometry, menus, shutdown.
3. **Transfer and sync** (#139, #140, #142) — descriptor ownership, moving panes, cross-window consistency.
4. **Acceptance** (#144, #145).
