# Terminal pop-out, step 2: a working window

Status: implementation contract for plan #15 tasks #136, #140, #141 and the single-pane half
of #139. Extends `2026-08-15-terminal-pop-out-window-contract.md`; that document's decisions
(independent top-level window, replay-window scrollback, in-memory-only replay, `pop out` not
`detach`) are unchanged and binding.

**This step must ship a feature the user can operate.** A pop-out control that opens a window,
moves a running terminal into it without restarting the shell, and returns the terminal to the
main window when that window closes. Anything the step cannot finish honestly is stated, not
silently deferred.

## 1. Ownership

One shared Rust process. Every window is a webview over the same `DesktopRuntime`, the same
terminal `Manager`, and the same loopback stream server. Windows never talk to each other —
Rust is the only place both can agree.

The backend keeps a **window assignment map**: window label → the session it is showing. It is
in-memory, per run, and never persisted; a crashed or restarted app opens with no terminal
windows.

## 2. IPC

Four new allowlisted `DesktopRuntime` commands. The Tauri command surface stays at exactly
three, so window creation happens inside `gui_invoke`.

| Command | Arguments | Returns |
| --- | --- | --- |
| `OpenTerminalWindow` | `[sessionId]` | `{ "label": "terminal-1" }` |
| `GetTerminalWindowSession` | `[label]` | `{ "sessionId": string \| null }` |
| `CloseTerminalWindow` | `[label]` | `{ "sessionId": string \| null }` |
| `ClaimTerminalStream` | `[sessionId, fromSequence]` | `{ "url": string, "fromSequence": number, "gap": bool }` |

- `OpenTerminalWindow` mints the next `terminal-<n>` label, records the assignment, and builds
  the window. `WebviewWindowBuilder::build` deadlocks if called from a synchronous command on
  Windows and `gui_invoke` runs on `spawn_blocking`, so the build is dispatched with
  `run_on_main_thread`. It does **not** use `parent()` — the window is independent.
- `GetTerminalWindowSession` is how a freshly loaded terminal window learns what it owns. It
  returns `null` for an unknown label rather than erroring, so a stale window closes cleanly.
- `CloseTerminalWindow` clears the assignment and returns the session that was freed, so the
  caller knows what to take back.
- `ClaimTerminalStream` is the bridge exposure of the ticket minting that step 1 built but left
  unreachable. It is fenced by the workspace generation.

## 3. The window

Terminal windows load the existing `index.html` with `#terminal-window=<label>` in the URL —
**no second Vite entry point**, so the fixed `app.js` / `style.css` output names and the build
contract test are untouched.

In that mode the frontend renders the terminal surface only: no sidebar, no board, no plans, no
Settings dialog. It reads its label from the fragment, asks `GetTerminalWindowSession`, claims a
stream, and attaches.

`src-tauri/capabilities/main-window.json` widens `"windows"` to `["main", "terminal-*"]`. The
permission array stays exactly `core:event:allow-listen` + `core:event:allow-unlisten`, and the
security test's assertions that no `window`, `webview`, `menu`, `tray`, `image`, or `allow-emit`
permission appears stay **verbatim**. Widening the label list is the entire change.

## 4. Moving a terminal

Pop out, in order:

1. The main window releases its lease on the pane's session, recording the last sequence it
   rendered, and tears down its renderer. The PTY keeps running; output accumulates in the ring.
2. `OpenTerminalWindow(sessionId)` records the assignment and opens the window.
3. The terminal window claims a fresh ticket from its recorded sequence and attaches. Where the
   ring has wrapped, it says so rather than pretending the gap did not happen.

If any step fails, **the terminal stays where it was** and the main window re-claims its lease.
A failed pop-out must never leave a session with no owner.

Pop in is the same in reverse, and happens automatically when the terminal window closes.

## 5. Lifecycle — every handler must match on `window.label()`

The current handlers are label-blind and app-wide. Each of these is a real defect the moment a
second window exists:

- **`CloseRequested` must not call `begin_shutdown` for a terminal window.** Today it would kill
  the whole app runtime, leaving the main window a shell whose every command fails. Only the
  main window's close begins shutdown.
- **Geometry capture must not overwrite the main window's rect.** `window-state` gains
  per-window entries keyed by label, keeping plan #14's versioned, totally-normalized,
  single-transaction, never-overwrite-unreadable discipline. The capture seal becomes per-window.
- **Theme application must cover every window**, not the hard-coded `main`.
- **The exit flush must not assume `main` is still registered** — with two windows, which is
  destroyed last varies.
- Closing a terminal window returns its session to the main window. Closing the main window, or
  switching project, closes the terminal windows and cleans up their sessions.

## 6. Menus

`Builder::menu` applies one menu to every window and `app.emit` broadcasts to every webview, so
today every menu command would fire twice — "Open Project…" would open two dialogs. Menu events
target the **focused** window with `emit_to`. Terminal windows ignore commands that make no
sense there rather than acting on them.

## 7. Honest scope

In this step: one session per terminal window, moved whole. Splits and multi-tab transfer are
task #139's remainder and are **not** claimed here. The pop-out control appears only where a
single pane can be moved; it is absent, not broken, elsewhere.

The gap notice from step 1 gets its first UI here: when the replay ring has wrapped, the
terminal window states that earlier output was not carried over.

## 8. Verification

- Rust: assignment lifecycle, label-scoped shutdown (a terminal window's close must not begin
  shutdown), per-window geometry isolation, and that a failed open leaves the session owned.
- Frontend: pop-out and pop-in against a fake bridge, including the failure path that keeps the
  terminal in place.
- Manual, and stated as manual: pop a terminal out, type in it, close the window, confirm the
  session returns with its shell alive and its output intact.
- `make test` green, and the Windows VM run for the terminal crates.
