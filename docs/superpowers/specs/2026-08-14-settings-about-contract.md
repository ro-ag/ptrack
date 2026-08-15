# Settings and About contract (plan #13)

Status: contract for tasks #118–#126. Frozen surface for implementation and review.

## 1. Information architecture

Three distinct surfaces. They do not duplicate each other.

| Surface | Trigger | Scope |
| --- | --- | --- |
| Settings dialog | `CmdOrCtrl+,`, Project ▸ Settings…, topbar control | Appearance, Terminal, Updates, Data & Diagnostics |
| About & Updates dialog | version chip, native About item, Help ▸ Check for Updates | identity, version, build, licenses, links, update status/actions |
| Capabilities page | Project ▸ Network Capabilities…, `Cmd+3` | network grants (unchanged) |

Settings is an application-level modal dialog, available with no project open. It never
holds project-scoped state. Its four sections are a roving-tabindex `tablist`:

1. **Appearance** — theme, density, reduced motion.
2. **Terminal** — default profile, font family, font size, Unicode mode, scrollback, renderer.
3. **Updates** — automatic-check opt-in and privacy statement; link to About & Updates for
   check/download/install actions.
4. **Data & Diagnostics** — data paths, backup ledger, migration receipts/quarantine,
   capability summary with a link to the Capabilities page, recovery status.

Settings owns the word "Settings". The Capabilities page keeps its own label and route;
the pre-existing `settings` view id is renamed to `capabilities` so the two are unambiguous.

## 2. Persistence contract

Single durable record in the existing global store, table `ptrack.global.config`
(`crates/ptrack-store/src/schema.rs`), key **`preferences`**. Value is UTF-8 JSON:

```json
{
  "version": 1,
  "appearance": { "theme": "system", "density": "comfortable", "reducedMotion": "system" },
  "terminal": {
    "defaultProfileId": null,
    "fontFamily": "monospace",
    "fontSize": 14,
    "unicodeMode": "modern",
    "scrollback": 25000,
    "renderer": "auto"
  }
}
```

Rules:

- **Write is whole-record and atomic.** Callers send a partial patch; the runtime merges it
  onto the current normalized record and stores the full document. No partial keys on disk.
- **Normalization is total.** Every read normalizes: unknown enum values, out-of-range
  numbers, wrong types, and missing fields fall back to the documented default. A malformed
  or unreadable record reads as defaults and is **not** rewritten until the user changes a
  setting, so a downgrade cannot silently destroy a newer record.
- **Forward compatibility.** `version` greater than the supported version is treated as
  unreadable (defaults, no rewrite). `version` less than current is upgraded in memory and
  persisted on next write. Unknown top-level members are dropped on write.
- **Reset** deletes the key. The next read returns defaults.
- **Ranges.** `fontSize` 10–24 (clamped). `scrollback` 1000–200000 (clamped).
  `theme` ∈ {`system`,`dark`,`light`}. `density` ∈ {`comfortable`,`compact`}.
  `reducedMotion` ∈ {`system`,`always`,`never`}. `unicodeMode` ∈ {`modern`,`legacy`}.
  `renderer` ∈ {`auto`,`webgl`,`canvas`,`dom`}. `defaultProfileId` is a stored string or
  null; an id that no longer resolves is reported as unavailable, never coerced.
- **Update preferences are not stored here.** `updates.auto-check` remains the single source
  of truth, read and written through the existing update runtime commands.
- **`localStorage` is a cache, not an authority.** The stored record wins on load. The theme
  key `ptrack-theme` keeps being written so the pre-paint guard in `index.html` avoids a
  flash; terminal font-size and Unicode keys become mirrors of the stored record.

## 3. IPC contract

Four new allowlisted `DesktopRuntime` commands (no new `#[tauri::command]`; the Tauri
surface stays at three):

| Command | Arguments | Returns |
| --- | --- | --- |
| `GetPreferences` | none | normalized preferences document + `storage` status |
| `SetPreferences` | partial patch | normalized document after merge |
| `ResetPreferences` | none | normalized defaults |
| `GetDiagnosticsReport` | none | paths, backups, migration, recovery, capability summary |

`storage` reports `ok`, `defaults` (no record yet), or `unreadable` (malformed or newer
version) so the UI can state plainly that stored settings could not be read.

`GetDiagnosticsReport` is read-only. It exposes: global home, project database path,
runtime and updates directories, the backup ledger, migration quarantine counts and import
receipt locations, recovery-required status, and granted/total capability counts. It never
returns secrets, tokens, or capability credentials.

## 4. Behavior

- **Appearance.** `theme` resolves `system` against `prefers-color-scheme`. `density`
  switches the spacing scale token set. `reducedMotion` `system` follows the media query;
  `always`/`never` force the corresponding behavior.
- **Terminal.** Changes apply to newly opened panes; the active pane keeps its per-pane
  zoom override. The default profile is a preference, not a lease — sessions already bound
  to a profile are untouched.
- **Updates.** The toggle states that checks are opt-in and that downloads and installs stay
  manual. Toggling writes through the existing `SetAutomaticUpdateChecks` command.
- **Data & Diagnostics.** Read-only. Paths are copyable. Recovery status is shown with the
  existing remediation text; no new destructive control is added.

## 5. Accessibility and keyboard

- `role="dialog"` `aria-modal="true"`, labelled by its heading; focus moves to the section
  list on open and is restored to the trigger on close; focus is trapped while open;
  `Escape` closes through the existing application-overlay registry.
- Section list is `role="tablist"` with roving tabindex: `ArrowUp`/`ArrowDown`/`Home`/`End`
  move, and each panel is `role="tabpanel"` labelled by its tab.
- `CmdOrCtrl+,` opens Settings from anywhere the native menu command is allowed, including
  with no project open. The shortcut is also mapped in the frontend shortcut table so the
  behavior is identical without the native menu.
- Every control has a persistent visible label, a described range where applicable, and an
  `aria-live` save-status region outside any busy wrapper.
- Reflow at 320 CSS pixels and 400% zoom without loss of function; forced-colors focus and
  disabled states; 3:1 control boundaries and 4.5:1 text.

## 6. Verification

- Rust unit tests for normalization, clamping, merge, version gating, reset, and the
  unreadable-record no-rewrite rule (`*_test.rs` siblings).
- Frontend unit tests for the pure preferences/section/keyboard modules.
- Frozen-list tests updated for the four new commands on both sides of the bridge.
- `make test` green: frontend build and tests, `cargo fmt --check`, workspace tests,
  `clippy -D warnings`, `cargo doc -D warnings`, help and release contract checks.
