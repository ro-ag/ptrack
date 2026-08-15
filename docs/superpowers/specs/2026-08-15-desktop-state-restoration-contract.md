# Desktop state persistence and restoration contract (plan #14)

Status: contract for tasks #127–#134. Frozen surface for implementation and review.
Builds on the plan #13 contract, `2026-08-14-settings-about-contract.md`, and reuses its
persistence discipline rather than introducing a second one.

## 1. Ownership and layering

| State | Authority | Written by |
| --- | --- | --- |
| Window geometry and display placement | Rust, global store key `window-state` | `src-tauri` window event handler |
| Layout sizes and visibility | Rust, global store key `layout-state` | frontend through one command |
| Selected view, plan, navigation context | Rust, global store key `layout-state` | frontend through one command |
| Startup opt-in and last project | Rust, `preferences` record, new `startup` section | Settings UI |
| Terminal workspace descriptors | unchanged: `localStorage`, per project root | `workspace/persistence.ts` |

**The window is owned entirely by Rust.** `src-tauri/capabilities/main-window.json` grants exactly
`core:event:allow-listen` and `core:event:allow-unlisten`, and `src-tauri/tests/security_contract.rs`
asserts that list is exact and contains no `window` or `webview` permission. The frontend therefore
never reads or writes window geometry, and **no new IPC command is added for task #127**. Restore
happens in `run_desktop`'s setup before the window is shown; capture happens in the existing
`.on_window_event` closure.

**Three keys, not one.** Window geometry changes on every drag; the `preferences` record is
user intent and changes rarely. They stay in separate global-store config keys so a resize
storm never rewrites user preferences, and a corrupt layout record can never cost the user
their settings. All three keys share the same discipline: versioned envelope, total
normalization, clamping, no rewrite of an unreadable or newer record, and the tri-state
`Ok` / `Defaults` / `Unreadable` status.

**`localStorage` stays a cache.** The plan #13 rule holds. Theme keeps its pre-paint key
because IPC is too late for first paint; sidebar width joins it for the same reason, mirrored
from the stored record on load. The stored record is authoritative whenever both disagree.

## 2. Window state (#127)

Stored under `window-state`:

```json
{
  "version": 1,
  "logical": { "x": 120.0, "y": 80.0, "width": 1440.0, "height": 900.0 },
  "scaleFactor": 2.0,
  "maximized": false,
  "fullscreen": false,
  "display": { "workArea": { "x": 0, "y": 0, "width": 3456, "height": 2160 }, "scaleFactor": 2.0 }
}
```

- **Logical coordinates only.** `outer_position` and `inner_size` are physical; a physical rect
  replayed at a different scale factor lands in the wrong place and the wrong size. Divide by the
  scale factor on capture, multiply on restore.
- **Display fingerprint is the work-area rectangle plus its scale factor**, not `Monitor::name()`,
  which is `Option<String>` and not stable across replug on every platform.
- **Restore is clamped, never blind.** On startup, enumerate `available_monitors()`. If no monitor's
  work area intersects the stored rect by at least 64 logical pixels in both axes, discard the
  position and fall back to the configured default centered on the primary monitor — the size is
  still restored, clamped to the target work area and to the existing 880×560 minimum.
- **Fullscreen is never restored.** A window that was quit in fullscreen reopens windowed at its
  pre-fullscreen rect; restoring fullscreen strands users whose display is gone. Maximized **is**
  restored when the target work area still admits it.
- **Capture is debounced.** `Resized` and `Moved` fire continuously during a drag; coalesce and
  write at most once per second, and flush once on `CloseRequested` before the window closes.
  `ScaleFactorChanged` rewrites the stored scale factor and re-derives the logical rect.

## 3. Layout and navigation state (#129, #130)

Stored under `layout-state`, project-scoped by root path where the state is project-specific:

```json
{
  "version": 1,
  "sidebar": { "width": 280, "hidden": false },
  "panels": { "boardHidden": false, "terminalHidden": false },
  "projects": { "<project root>": { "view": "board", "planId": 13, "foldedLanes": ["done"] } }
}
```

- `view` is validated against the existing allowlist (`board`, `overview`, `capabilities`); anything
  else falls back to `board`.
- `planId` is a hint, never an authority. On restore the backend still resolves it; a plan that no
  longer exists, or belongs to another project, silently falls back to the active plan (`0`).
- The per-project map is bounded to 32 entries, evicting least-recently-used, so it cannot grow
  without limit.
- Sidebar width is clamped by the existing `clampSidebarWidth` rules. The stored record wins over
  the `localStorage` mirror on load.
- Task drawer selection is **not** persisted: reopening onto a task the user has since finished is
  worse than reopening onto the board.

## 4. Startup and last project (#128)

New `startup` section in the existing `preferences` record:

```json
{ "restoreLastProject": false, "lastProjectRoot": null }
```

- **Default off.** The task says "only when the user has opted in", so the first launch after
  upgrade must not silently change behavior.
- **An explicit context always wins.** A CLI-supplied path is an explicit instruction and beats the
  opt-in. So does a working directory that is itself a bound project: launching from inside a
  project must open that project, which is both the pre-existing behavior and the only reading a
  terminal user expects. Auto-open applies **only** when neither is present — the Finder and Dock
  launch, where the working directory is `/` and no project is named.
- **Auto-open requires proof.** Reuse `resolve_recent_project`: open only when availability is
  `Available` and resolution is `Ready`. A `confirmation-required` result — the relocated-project
  case — must **never** auto-open; it opens Welcome with that entry preselected so the user
  confirms the relocation themselves.
- `lastProjectRoot` is recorded on successful open and cleared on explicit project close.

## 5. Terminal workspace descriptors (#131)

Already satisfied by `frontend/src/workspace/persistence.ts`: `cloneWorkspaceForPersistence` is an
explicit allowlist of `activeTabId`, tab `id`/`title`/`activePaneId`/`root`/`association`, pane
`paneId`/`profileId`/`cwd`, and split `splitId`/`direction`/`ratio`/`first`/`second`. Buffer
contents, session ids, PTY handles, environment, and tokens are structurally excluded.

This task therefore adds **no new persistence**. It adds a test that pins the allowlist — asserting
that a workspace carrying extra fields round-trips without them — and documents the rule here so a
later change cannot widen it silently.

## 6. Crash-safe writes and corrupt-state fallback (#132)

Reuse `GlobalStore::update_config` (`crates/ptrack-store/src/global.rs`): read, merge, and write in
one redb write transaction, so a torn write is impossible and two concurrent updates cannot lose
each other. No new atomic-write machinery.

Each record decodes to the tri-state status. Empty is `Defaults`. Unparseable or a `version` above
the supported one is `Unreadable`, reads as defaults, and is **not** rewritten until the user
changes something — a downgrade cannot destroy a newer record. A record that fails normalization in
part is repaired field by field, never discarded wholesale.

## 7. Reset actions (#133)

Both live in **Settings ▸ Data & Diagnostics**, behind explicit confirmation. Neither appears in the
native menus: a destructive item one keystroke from "Settings…" is a footgun, and it would churn the
frozen 11-event menu list and its exact-match security test.

- **Reset Window Layout** — clears `window-state` and `layout-state`, then applies defaults live.
  Non-destructive to user data. Single confirmation.
- **Reset Application State** — clears everything app-scoped: `preferences` (including the update
  auto-check opt-in and the startup section), `window-state`, `layout-state`, every per-project
  terminal workspace descriptor in `localStorage`, and all network capability grants. It requires a
  distinct confirmation that names the capability grants explicitly, because re-granting them is
  real work the user must redo.
- **Neither ever touches project content**: plans, tasks, notes, and the recents registry are out
  of scope, and the confirmation says so.
- **Capability grants are the one project-scoped exception, and it is deliberate.** Grants live in
  the project store, so revoking them writes to the open project's database — an earlier draft of
  this contract demanded both "revoke all grants" and "never write to the project database", which
  cannot both hold. The resolution: revoking disables the enabled capability, dropping the grant and
  its broker lease, while leaving the user's capability definitions authored and intact, so the user
  re-grants rather than re-authors. With no project open, nothing is revoked and the result says so.
  The confirmation must state this plainly rather than claim project databases are untouched.
- After Reset Application State the app returns to its default state without a restart; any window
  the reset invalidates is re-laid-out in place.

## 8. Verification (#134)

Rust, following the existing `production_test.rs` pattern where a "restart" is a second runtime
constructed over the same temp home:

- Relaunch restores geometry, layout, and the opted-in project; with the opt-in off, relaunch lands
  on Welcome.
- Crash — state tampered with between two loads — falls back to defaults without rewriting the
  record.
- Display removal: a stored rect with no intersecting work area is discarded and recentered, size
  preserved and clamped.
- DPI change: a rect captured at scale 2.0 restores correctly at scale 1.0.
- Project close clears `lastProjectRoot`; theme change does not disturb window or layout state.
- Concurrent writes to the three keys never lose each other.

Frontend, following `persistence.test.ts`: the pure loader re-run against a retained fake storage,
plus the descriptor-allowlist pin from §5.

`make test` green, including `cargo fmt --check`, `clippy -D warnings`, `cargo doc -D warnings`, and
the help and release contract checks. Windows is the only platform that proves the store's private
parent requirement, so any new temp-backed test must call `protect_private_directory`.
