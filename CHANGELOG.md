# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Multi-developer groundwork on a single shared database. `ptrack config set
  user <name>` mints a stable per-machine identity once (rename any time, the
  ID never changes), every mutation is stamped with the acting identity, and
  the active plan becomes per-developer (machines without a configured
  identity keep the old project-wide active plan). `ptrack plan use` now
  claims the plan for the caller: content changes to a plan claimed by someone
  else are refused, while holds, notes, and issue links stay open to everyone.
  `ptrack plan release` frees a claim, finishing a plan releases it
  automatically, and `ptrack plan use --steal` takes over a claim. Claim
  owners are shown in `plan list`, plan JSON, the TUI, and the desktop
  sidebar (display-only; claim changes are CLI-only).

### Changed
- **Compatibility: upgrade every installed copy of p-track.** The first write
  from this version stamps the project or global database with payload schema
  3, which also reserves the record fields the upcoming git-synced
  multi-developer mode needs (entity ULIDs and claim-conflict markers).
  Older p-track builds accept only schemas 1 and 2 and refuse the whole
  database fail-closed with `invalid database manifest: collection plans
  requires codec 3 schema 1..=2, found codec 3 schema 3` or the equivalent
  for another collection. Nothing is damaged and no data is lost. Databases
  written by 0.25 or 0.26 open unchanged and upgrade lazily per record.

## [0.26.0] - 2026-08-18

### Added
- Plans and tasks go on hold with a reason, independently of status.
  `ptrack plan hold|resume` and `ptrack task hold|resume` pause work that is
  waiting on something external; the item keeps its `todo`/`doing`/`blocked`
  status and its board column, and gains a hold marker in every read surface —
  CLI lists and `show`, `search`, the dashboard, the desktop board, and the
  launch context handed to an agent. `ptrack next` refuses held work,
  `ptrack context` lists it in its own bucket instead of the pick-up list, and
  `resume` — or completing the item — clears the hold.

### Changed
- **Compatibility: upgrade every installed copy of p-track.** The first write
  from this version stamps the project or global database with payload schema
  2. Older p-track builds accept only schema 1 and refuse the whole database
  fail-closed, with `invalid database manifest: collection plans requires
  codec 3 schema 1, found codec 3 schema 2` or the equivalent for another
  collection. Nothing is damaged and no data is lost — the older binary reads
  nothing and writes nothing — but a machine that runs both an old and a new
  p-track against the same project will see the old one refuse it from the
  first write of any kind onward, hold or not.

## [0.25.0] - 2026-08-18

### Removed
- The offline bbolt migration tool (`ptrack-db-import`) and every legacy-
  database refusal. With no projects left on the old format, retired
  bbolt-era files (`global.db`, `.ptrack/ptrack.db`) are now inert evidence:
  initialization and validation never read them, never refuse because of
  them, and never steer anyone to the removed "offline migration workflow".
  Open-validation of existing migrated stores (their `imported`/`json-stage`
  manifest contracts) is unchanged.

## [0.24.4] - 2026-08-17

### Fixed
- Initializing a home directory as a project is refused by name. `ptrack
  init` in `~` used to fail with a misleading "database destination already
  exists" recovery error; both the CLI and the desktop folder picker now
  answer "the p-track home directory cannot be a project" — and the OS user
  home is refused even when `PTRACK_HOME` points elsewhere.

## [0.24.3] - 2026-08-17

### Fixed
- Initializing a new project no longer fails with "global runtime state
  requires recovery" on a healthy machine that kept its legacy `global.db` as
  migration evidence. A bound runtime now vouches for the global state;
  unfinished migrations and leftover bootstrap plans still fail closed.
- A global home directory carrying group/other permission bits — a restore, a
  sync, or a copy made under a default umask — no longer locks the runtime out
  with "global home permissions are not private". The directory is tightened
  to owner-only on use and re-verified, matching the database-file healing in
  0.24.1; paths that cannot be tightened still fail closed.

## [0.24.1] - 2026-08-17

### Fixed
- The desktop no longer aborts silently when startup fails. A setup error used
  to unwind out of the platform's launch callback — an immediate crash with no
  message anywhere. Failures are now recorded to `ptrack-startup-failure.log`
  in the home directory, shown in a native dialog, and the process exits
  cleanly; a panic hook leaves the same evidence for any startup panic.
- A project database with leaked group/other permission bits — a git checkout
  or a copy made under a default umask — no longer bricks every command and
  the desktop launch. The store tightens the file to owner-only on open and
  re-verifies; files that cannot be tightened still fail closed.

## [0.24.0] - 2026-08-17

### Added
- Terminal tabs pop out into their own windows: the whole tab moves — split
  tree, running sessions, title — without restarting a shell, and closing the
  window returns everything to the main window. Moves are all-or-nothing; a
  failed pop-out leaves every session exactly where it was.
- Terminal windows render splits with the same split view as the main window
  and carry its per-session surfaces: search, the multi-line paste guard,
  per-profile zoom, copy, and select-all. Split resizes made in the window
  survive the return to the main window.
- Terminal windows follow the shared stored profile settings, per-profile
  font sizes, and color theme, so a choice made in either window means the
  same thing in both.
- First-run project onboarding, a coherent Settings and About experience, and
  desktop state persistence: window geometry, layout, and the open project
  are restored on the next launch.
- Releasing a terminal renderer is now separate from terminating its session,
  so losing a renderer can never kill a running shell; renderers re-claim
  their stream with bounded backoff and a stated scrollback-gap notice.

### Fixed
- Windows: candidate destination directories are synced, project path
  resolution through a file ancestor is denied, the pty working directory
  strips the verbatim prefix cmd.exe rejects, and cmd.exe per-drive
  environment entries are dropped before spawning.
- Native window chrome matches the application theme, capability forms are
  tightened, the plural capability listing routes through the broker, and the
  activity panel grows when its detail is expanded.

### Removed
- Removed the final Go source module and its build/CI integration after the
  v0.23.0 offline migration cutover. Historical exporter source remains
  available from that tagged revision when recovery evidence requires it.

## [0.23.0] - 2026-08-13

### Changed
- The application runtime is now entirely Rust/Tauri. Legacy bbolt data moves
  through an explicit offline export, verified redb import, and generation
  activation; the only retained Go module is the read-only exporter.

### Fixed
- Linux and Windows release executables are built through Tauri so the
  embedded Desktop frontend is present when `ptrack gui` starts.

### Security
- Runtime store routing is pinned by a private, attested active-generation
  marker and retained cutover lock. Automatic migration rollback is refused
  after any application write to the new global or project stores; preserve
  the migration journal, handoff, receipt, marker, and legacy files for manual
  recovery instead of forcing a downgrade.

## [0.22.0] - 2026-08-12

### Added
- The branded, versioned Help Center now documents onboarding, Desktop,
  terminals, agents and capabilities, reference material, installation,
  safety, and troubleshooting with local search and contextual app links.
- The Rust rewrite now has a frozen, source-backed parity contract plus its
  first pure-Rust domain codec and transactional redb destination store.
- Developer-only migration tooling can freeze the global bbolt database and
  every registered project read-only, emit a private typed JSON stage, and
  create verified inactive redb candidates. It never replaces live databases,
  and malformed legacy capability history remains inert in quarantine.

### Changed
- Desktop native menus, project switching, keyboard behavior, and application
  overlays have clearer focus, accessibility, and lifecycle boundaries.
- Product documentation now presents p-track consistently as a local control
  layer for observing agent work, keeping durable plans, and passing bounded
  context between supported tools.

### Fixed
- The Help Center home page now displays the branded product visuals instead
  of omitting its workflow previews.

## [0.21.0] - 2026-08-11

### Added
- Terminal profiles now provide bounded renderer, theme, scrollback, working
  directory, exit, and non-secret environment policies while preserving agent
  executable identity and capability boundaries.
- Terminal compatibility now includes cross-platform PTY and shell-integration
  handling, resize and lifecycle hardening, content-free diagnostics and
  recovery actions, and renderer retry/fallback behavior.
- The desktop app now provides an accessible, app-level About & Updates flow
  for explicit or opt-in startup checks against stable GitHub Releases,
  separate verified download and installation actions, bounded progress,
  cancellation, and restart-safe recovery.
- Native release updates use exact OS/architecture assets and `checksums.txt`:
  pinned Developer ID and Gatekeeper verification before macOS DMG handoff,
  verified ZIP reveal on Windows, and user-owned atomic replacement with
  rollback and crash recovery on Linux. Generated source archives,
  prereleases, downgrades, development builds, and arbitrary URLs are refused.

## [0.20.0] - 2026-08-10

### Added
- Multi-session terminal workspaces now support persistent tab and split
  layouts, independent live sessions, bounded descriptor-only restore, and
  explicit lifecycle cleanup without persisting terminal contents or tokens.
- Project- and agent-profile-scoped HTTP, Git, and SSH capabilities now use
  explicit approval windows, bounded operations, connection diagnostics, and
  metadata-only audit records. Capabilities remain disabled until approved and
  never store credentials.
- Terminals and AgentRuns can be linked to plans and tasks through
  generation-scoped associations, with explicit relink, detach, typed
  write-back, and resource-fenced task-transition confirmation.
- Provider-neutral AgentRun intelligence accepts authenticated, allowlisted
  structured evidence from Codex, Claude, Gemini, Agy, and OpenCode while
  excluding prompts, reasoning, tool payloads, terminal output, and secrets.
- Unified agent coordination adds conservative activity states, explicit task
  ownership and overlap warnings, content-free notifications, single-use
  handoffs, evidence-only drift warnings, read-only existing-worktree
  association, and user-approved workflow proposals that execute nothing.

### Changed
- The product display name is consistently styled as `p-track` across the
  desktop app, CLI prose, documentation, terminal environment, and release
  artifacts. The all-caps form remains reserved for terminal wordmark artwork.
- The desktop app displays the resolved build version and coalesces agent
  runtime refreshes without re-announcing unchanged activity or discarding
  focused controls and pending worktree selections.

### Fixed
- Embedded terminal panes now fill and resize with their dock, split, and
  window while retaining the correct live session.

## [0.19.0] - 2026-08-09

### Added
- Independent panel controls can hide the sidebar, board, or terminal. Hiding
  the board expands the embedded terminal to the full workspace height, while
  the sidebar can be resized with either a pointer or the keyboard and keeps
  its width across launches.
- The embedded terminal now searches its 25,000-line scrollback, persists font
  zoom, and exposes clear and emulator-reset actions through compact toolbar,
  keyboard, and context-menu controls.
- Agy joins the detected agent profiles, and macOS profile discovery also
  checks common Homebrew, local-bin, and OpenCode locations for Agy, Claude,
  Codex, Gemini, and other supported agents.
- A design note records the deferred cgo-free Tauri, Rust `native-ipc`, and
  `libghostty-vt` migration boundary.

### Fixed
- Embedded shells preserve ANSI and truecolor output even when the desktop
  launcher inherited `NO_COLOR`, and receive a UTF-8 locale when none was set.
- Modern Unicode 15 grapheme and emoji cell widths are enabled by default in
  the embedded terminal and can be disabled with a persisted compatibility
  setting.
- Terminal rendering now falls back cleanly after WebGL context loss and makes
  bounded attempts to restore accelerated rendering.

## [0.18.0] - 2026-08-02

### Added
- Command palette (⌘K): search across plans, tasks, and notes with grouped
  results; activating a result jumps to the plan, opens the task's detail
  drawer, or heads to the relevant Overview section.
- Keyboard shortcuts: ⌘1 Board, ⌘2 Overview, ⌘N new task, ⌘K palette.
- Progress visuals: activity heatmap (16 weeks of notes + commits) and a
  plan progress ring on the Overview, plus per-plan progress bars in the
  sidebar plan list. Hand-rolled SVG, theme-aware, no chart library.
- Live refresh: the workspace reloads itself when the project database
  changes on disk (CLI or another agent) and when the window regains focus;
  the manual Refresh button is gone.
- Empty kanban lanes collapse into slim rails so populated lanes get the
  space; click a rail to expand it back.
- Canvas-sheet workspace layout: a flush navigation sidebar (brand, Board /
  Overview links, the project's plan list, recent projects) with the content
  on a rounded, elevated canvas. The plan picker moved from the topbar into
  the sidebar as a scrollable list.
- Overview page: a dedicated full-width view holding the project memory —
  North star, Agent handoff, Project status (compact stat tiles), Open
  issues, Recent memory, and the repository intelligence cards — so the
  board gets the whole canvas.
- Light theme for the desktop workspace, with a topbar toggle (☀/☾) that
  persists the choice; without an explicit choice the app follows the macOS
  appearance live.
- The window titlebar now blends into the workspace: transparent, full-size
  content with inset traffic lights instead of the detached default bar. The
  topbar itself is a slim 44px strip with compact, low-key controls; the
  brand block moved out of it into the sidebar.
- Settings ▸ "Install 'ptrack' Shell Command…" in the desktop app adds the
  app's own binary directory to PATH in ~/.zprofile (idempotent, marked
  block), so the `ptrack` CLI works in new terminal sessions without a
  separate install.

### Fixed
- The built-in terminal now starts the user's real login shell (zsh with
  their rc files, colors, and prompt) instead of a bare `sh`. The account's
  UserShell record is read from Directory Services and takes priority over
  the SHELL variable, which apps launched through LaunchServices inherit
  from the *requesting* process rather than the user.
- Overview page cards no longer let long issue titles and memory entries
  bleed into neighboring cards, and Recent memory items no longer overlap:
  list items shrink and clip correctly, and the two-line detail clamp no
  longer relies on -webkit-line-clamp, which mis-measures in grid
  containers.
- Launching the installed app from Finder or the Dock actually opens the
  desktop GUI now. The bundle's entry point is a launcher script that always
  runs `ptrack gui`; the previous stdio sniffing misread the /dev/null
  descriptors launchd attaches as a usable terminal and silently exited.
  Invoking the `ptrack` binary directly with no subcommand is unchanged and
  stays the terminal dashboard.

## [0.17.0] - 2026-08-02

### Added
- Task detail drawer in the desktop workspace: clicking a board card (or
  pressing Enter on it) opens a side panel with the task's notes, linked
  commits, and linked issues, plus status, rename, and record-memory actions.
  Backed by a new `GetTaskDetailV2` GUI binding.

### Changed
- Premium visual overhaul of the desktop workspace: layered elevation and
  shadows, frosted-glass topbar and drawer, lane-tinted board columns,
  hover-lifting cards, refined controls, custom scrollbars, entrance
  animations, and a `prefers-reduced-motion` fallback.

### Fixed
- Launching the app bundle from Finder or the Dock now opens the desktop GUI.
  Previously a no-argument launch always tried to start the terminal
  dashboard, which silently exited without a controlling terminal.

## [0.16.1] - 2026-08-01

### Changed
- macOS release disk images are now automatically notarized and stapled by
  Apple in CI, so downloads install with no Gatekeeper warning. The notarized
  v0.16.0 disk images were republished as well.

## [0.16.0] - 2026-08-01

### Added
- Brand identity: a code-drawn app icon (kanban columns over a track rail in
  the brand palette), standalone PNG exports, a compiled `AppIcon.icns`, a
  README banner, and a social card — all reproducible from
  `assets/brand/generate_icons.py` (`make icons`). The stock Wails icon is
  gone from the app bundle.
- macOS app bundles are now properly packaged: `build/darwin/Info.plist` with
  bundle id `com.ro-ag.ptrack`, display name, developer-tools category, macOS
  12.0 minimum, and hardened-runtime `entitlements.plist` ready for signed
  releases. `make package` builds the macOS app bundle and `make dmg` builds a
  disk image with an `/Applications` drop link for both architectures.
- Developer ID signing: `make sign` / `make signed-dmg` sign locally with the
  identity fingerprint in `SIGN_IDENTITY`, and the release workflow imports
  the certificate into a throwaway keychain and signs the app, the disk image,
  and therefore the CLI binary inside the tarball whenever the
  `APPLE_CERTIFICATE_*` secrets are present. Notarization (`make release-dmg`
  and a matching CI step) activates automatically once the `APPLE_API_*`
  secrets exist.
- Registered agent runs now persist a bounded on-disk history
  (`~/.ptrack/runtime/<project>/agent-runs.json`) that survives app restarts
  and project switches. A launched run interrupted by a restart restores as
  stale with unknown process state instead of vanishing; an external run keeps
  its lease, so a still-alive agent resumes heartbeating automatically.
- The AgentRun integration descriptor now records its hosting process PID, and
  consumers get a documented recovery path: read the descriptor, treat a dead
  owner PID as stale, and wait for a fresh descriptor instead of dialling a
  dead port after a crash.

### Fixed
- Terminal exit recording now marks every not-yet-exited launched run on the
  terminal instead of stopping at the first match, so an exited record from a
  restarted session can never shadow a still-running one.
- The build assets under `build/` (app icon, darwin plists) are no longer
  excluded by `.gitignore`; previously they existed only on local machines,
  so CI could never produce a branded bundle.

## [0.15.0] - 2026-07-26

### Added
- `ptrack gui [PATH]` is now the canonical desktop command, with the current
  directory as its default and `ptrack board --gui` retained as a compatible
  alias.
- The desktop app now has native project open, switch, and close actions, a
  welcome screen with recent projects, and confirmation before transitions
  stop active terminals or registered agent runs.
- A bounded project overview combines goals, plans, tasks, blockers, issues,
  notes, recent p-track activity, storage health, terminal sessions, and
  explicit agent-run registrations.
- Read-only Git intelligence reports repository state, worktrees, status
  counts, upstream divergence, remotes, branches, recent commits, unpushed
  commits, and stale branches using bounded machine-readable Git commands.
- Explicitly registered agent runs now have stable identities, project and
  terminal associations, process state, heartbeats, lease expiry, and exit
  results.

### Changed
- Project resources are now owned by generation-scoped workspace contexts, so
  switching or closing cancels pending work and disposes stores, terminal
  sessions, agent integrations, listeners, sockets, and refresh activity
  without restarting the app or accepting stale responses.

The project-switching and terminal interaction paths were verified on macOS,
including active refresh and terminal cleanup. Windows and Linux interaction,
IME and Unicode input, curses and mouse applications, sustained high-volume
output, rapid resize and switching stress, and sleep/wake recovery remain in
the manual acceptance matrix.

## [0.14.1] - 2026-07-26

### Fixed
- Fresh source and release builds now generate Vite assets before Wails binding
  generation, instead of failing because the generated `frontend/dist`
  directory is intentionally absent from version control.
- This release supersedes the v0.14.0 tag, whose workflow stopped before
  creating native archives or a GitHub release.

## [0.14.0] - 2026-07-26

### Added
- The desktop board now includes a resizable embedded terminal dock that opens
  the default login shell or a detected installed-agent profile at the project
  root, with exit status and restart controls.
- Terminal I/O uses xterm.js 6 and an authenticated binary loopback WebSocket
  backed by PTYs, with explicit bounded backpressure, resize propagation, and
  lifecycle cleanup.
- Native clipboard copy and paste, selection-aware `Ctrl+C`, platform
  shortcuts, a terminal context menu, and bounded multiline-paste confirmation
  with bracketed-paste support.

### Changed
- Application builds now require Wails desktop build tags, preventing plain
  `go build` or `go install` from producing a binary with a broken `--gui`
  option. The default `make build` target always creates the complete hybrid
  CLI, TUI, and GUI executable.
- Installation guidance now directs users to native release archives, which
  always include GUI support.

Clipboard, shortcut, context-menu, restart, and shutdown interactions were
verified on macOS. Windows and Linux native builds are included, but their PTY,
clipboard, input, and descendant-process cleanup behavior remains pending
interactive validation. The broader manual matrix for curses and mouse input,
IME and Unicode, sustained high-volume output, resize stress, and sleep/wake
recovery also remains open.

## [0.13.1] - 2026-07-25

### Fixed
- Native release archives are written to the artifact upload directory on
  every runner, and Windows uploads include only the final ZIP file.
- This release supersedes the v0.13.0 tag, whose workflow stopped before
  creating a GitHub release.

## [0.13.0] - 2026-07-25

### Added
- `ptrack board --gui`, a Wails desktop kanban board with plan switching,
  drag-and-drop status changes, task creation and renaming, periodic refresh,
  task memory notes, linked context on cards, a project-memory rail, and
  transient database access for safe use alongside agents and the CLI.

### Changed
- Release archives are now built natively with Wails on Linux, macOS, and
  Windows for amd64 and arm64, so the CLI and desktop GUI ship together.
- The tag-only release workflow uses Node.js 24-compatible major versions of
  the official GitHub actions.

## [0.12.0] - 2026-07-25

### Added
- Selected-entry editing actions in the TUI command menu and item detail view.
- `ptrack task move <id> --plan <id>` and matching TUI actions for moving a
  task to another plan.
- `ptrack task convert <id>` (also available as `promote`) and matching
  confirmed TUI actions for promoting a task to a plan while preserving its
  milestone, notes, and commits.

### Changed
- Converting a task now maps a completed task to a completed plan, removes the
  original task atomically, and safely unlinks issues because issues cannot
  target plans.

## [0.11.0] - 2026-07-17

### Added
- A focused launch screen for bare `ptrack`, featuring the p-track Unicode
  block wordmark, a compact narrow-terminal fallback, and direct shortcuts to
  the dashboard, numbered screens, command menu, and quit action.
- A keyboard-driven command menu (`?`) that makes navigation, goal and summary
  editing, reload, and backup actions discoverable from the TUI.
- A dedicated **Maintenance** screen with project and database diagnostics,
  backup location, reload and backup actions, plus agent-guide and commit-hook
  upkeep commands.
- README screenshots captured from the real TUI for the launch screen,
  Overview, Command Menu, Board, and Maintenance views.

### Changed
- The dashboard now returns to its compact, full-window layout after the launch
  screen, with five numbered destinations: Overview, Board, Milestones, Issues,
  and Maintenance.
- The README has been reorganized around human and agent workflows, with a
  complete keyboard map, command reference, and storage guide.
- CLI help and the no-project hint now use the p-track product identity and
  clearer getting-started language.

### Fixed
- Backups created from the TUI now record the project root consistently with
  backups created through the CLI.

## [0.10.0] - 2026-07-16

### Changed
- Reworked the interactive dashboard into a denser, full-window layout with a
  framed navigation bar, balanced overview panes, full-row selection surfaces,
  and a restrained dark-cyan-to-blue-green focus treatment.
- Detail views now group notes, commits, tasks, plans, and explanations into
  distinct nested panels with a magenta-to-indigo accent family, making long
  item histories easier to scan.

### Fixed
- Long detail content now wraps to the available terminal width without
  splitting ANSI styling, and scrolling follows the wrapped visual lines so
  note tails remain reachable instead of being clipped.
- Dashboard panels now honor the terminal's exact outer dimensions at common
  and narrow viewport sizes, preventing borders and footer hints from escaping
  the visible window.

## [0.9.2] - 2026-07-12

### Fixed
- The TUI no longer holds the bbolt database open for its whole session. It
  reads a snapshot and closes, re-opening only briefly for edits and refreshes,
  so an AI agent (or the CLI) can read and write the same project concurrently
  while the dashboard is open — previously the viewer's exclusive lock could
  block or time out a concurrent write.

## [0.9.1] - 2026-07-12

### Added
- `ptrack commit show <id|sha> [--stat]` — prints a tracked commit's diff via
  `git show`, resolving a ptrack commit id to its SHA (or passing any git ref
  through). Closes the loop: see exactly what an agent changed.

## [0.9.0] - 2026-07-12

### Added
- **Commit tracking.** A first-class `Commit` record (SHA, subject, task/plan
  link). `ptrack commit add|list|record`, and `ptrack hook install` writes a
  git post-commit hook that auto-records every commit — linked to a task when
  the message contains `#<id>`, otherwise to the active plan. Commits appear in
  the TUI detail view for tasks and plans and in the `context` inventory.
- The agent guide now frames notes as the **human audit trail** ("record
  decisions, not narration") and documents commit linking via `#<id>`.

### Changed
- Database schema is now **format v3** (adds the commits bucket). Existing v1/v2
  databases migrate automatically on open.

## [0.8.0] - 2026-07-12

### Added
- **TUI detail view.** Press `enter` on any selected plan, task, milestone, or
  issue to open a scrollable detail panel showing its full fields, linked
  entities, and attached notes (the agent's decisions/explanations) — or the
  issue's body. `esc`/`enter` closes; `↑/↓`/pgup/pgdn scroll.

## [0.7.0] - 2026-07-12

### Added
- **Rename commands** for every entity: `ptrack plan|task|milestone|issue rename
  <id> "new title"`, and an `e` (edit title) key in the TUI on the selected item.
  Titles were previously immutable.

### Changed
- The agent guide now states **"titles are names, not status"** — agents should
  not prefix titles with "Pending:"/"In progress:"/"Done:" (ptrack tracks status
  separately via `task/plan/milestone/issue` status commands).

## [0.6.0] - 2026-07-12

### Changed
- **Rebuilt the TUI as a polished tabbed dashboard.** Four tabs — Overview,
  Board, Milestones, Issues — with an inventory header (colored badges),
  bordered lipgloss panels, status/severity colors, a starred active plan,
  scrolling lists, and edit actions across every entity (add/status/close/etc.).
  Navigate tabs with `tab`/`shift+tab` or `1`–`4`. The old two-pane list view is
  replaced; all previous actions remain, now organized per tab.

## [0.5.0] - 2026-07-12

### Added
- **Milestones** — a first-class tier grouping plans toward a checkpoint, with an
  optional due date. `ptrack milestone add|list|show|done|open|due`, and
  `ptrack plan add --milestone N` to assign a plan.
- **Issues** — first-class tracked problems/bugs with status (open/closed),
  severity (low/medium/high/critical), and an optional task link.
  `ptrack issue add|list|show|close|open|severity`.
- `context` now surfaces **open issues** (bounded) and reports milestones and
  issues in the inventory footer; `search` matches milestones and issues too.
- The agent guide gained an **"if the project is empty, populate it from this
  repo"** section covering goal → milestones → plans → tasks → issues → notes.

### Changed
- Database schema is now **format v2** (adds the milestones and issues buckets
  and `Plan.MilestoneID`). Existing v1 databases are migrated automatically on
  open; no action needed.

## [0.4.2] - 2026-07-12

### Changed
- `ptrack init` run inside an **already-initialized** project now refreshes it
  (updates the goal if given and re-installs the agent guide) instead of erroring.
  It refuses only when creating a genuinely *nested* new project — a different
  root under an existing one — which still needs `--force`. This makes
  `ptrack init` a safe sync command for existing projects (e.g. ones created
  before the guide feature that have no AGENTS.md/CLAUDE.md yet).

## [0.4.1] - 2026-07-12

### Fixed
- Guide install/refresh is now robust to malformed marker state. An orphaned
  `ptrack:begin` (no matching end) or duplicate blocks previously caused a second
  block to be appended; installs now normalize any marker mess into exactly one
  block while preserving all non-marker text and the block's position when it is
  well-formed.

## [0.4.0] - 2026-07-12

### Added
- **Global guide template.** A Markdown file at `~/.ptrack/guide.md` (or
  `$PTRACK_HOME/guide.md`), when present, is appended inside the installed guide
  block after the built-in section — so `ptrack init`/`guide` carry your own
  working agreements into every project you initialize, without changing what
  ptrack ships to other users. `guide --print` shows the combined result.

## [0.3.0] - 2026-07-12

### Added
- **Agent guide onboarding.** `ptrack init` now writes a marker-delimited ptrack
  section into the project's `AGENTS.md` and `CLAUDE.md` (creating them if
  absent, preserving existing content), teaching any AI agent the ptrack
  workflow — read `context` at session start, log decisions with `note add`,
  update `summary set` before ending, and drill with `next`/`board`/`show`/
  `search`. Skip with `--no-guide`.
- **`ptrack guide`** installs/refreshes that section idempotently;
  `ptrack guide --print` writes it to stdout.

## [0.2.1] - 2026-07-12

### Fixed
- Running bare `ptrack` outside any project now prints getting-started guidance
  (init / --goal / --help) and exits 0, instead of a terse `no ptrack project
  found` error with a non-zero exit.

## [0.2.0] - 2026-07-12

### Added
- **Agent query surface**, designed for bounded payloads: `next` (the single
  most-actionable task), `search`, `plan show`, `task show`, `note list`, and a
  `task list --status` filter. `--json` on every read command (Markdown remains
  the default).
- **Enriched `context`** that stays bounded regardless of project size: adds
  project-wide blocked tasks and an inventory footer (counts + the exact
  drill-down commands), so a fresh agent orients without dumping the whole
  project.
- **Kanban board**: `ptrack board` (Markdown/JSON) and an interactive TUI board
  view (`v` to toggle) with four status columns and card-move keys (`H/L`).
- **Schema versioning**: the database records a format version and the writing
  ptrack version; opening adopts pre-versioning databases, migrates older ones,
  and refuses databases written by a newer ptrack rather than corrupting them.
- **Safer `init`**: refuses to create a project nested inside an existing one
  (`--force` to override) and accepts `--root` to choose the location.

## [0.1.0] - 2026-07-12

Initial release.

### Added
- Embedded bbolt storage (`encoding/gob` values): per-project store
  (`.ptrack/ptrack.db`, discovered by walking up like `.git`) and a global store
  (`~/.ptrack/global.db`, override via `PTRACK_HOME`) for config, a project
  registry, and backups.
- Data model: goal, rolling context summary, plans, tasks, and notes.
- Agent-facing CLI: `init`, `goal`, `summary`, `plan`, `task`, `note`,
  `context`, `status`, `projects`, `backup`, `version`.
- `ptrack context` restore digest in Markdown (default) or `--json`.
- Interactive Bubble Tea dashboard (bare `ptrack`) for browsing and editing
  plans, tasks, goal, summary, and notes.
- `go install` support and cross-platform release binaries via GoReleaser.

[Unreleased]: https://github.com/ro-ag/ptrack/compare/v0.26.0...HEAD
[0.26.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.26.0
[0.25.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.25.0
[0.24.4]: https://github.com/ro-ag/ptrack/releases/tag/v0.24.4
[0.24.3]: https://github.com/ro-ag/ptrack/releases/tag/v0.24.3
[0.24.1]: https://github.com/ro-ag/ptrack/releases/tag/v0.24.1
[0.24.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.24.0
[0.23.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.23.0
[0.22.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.22.0
[0.21.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.21.0
[0.20.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.20.0
[0.19.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.19.0
[0.18.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.18.0
[0.17.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.17.0
[0.16.1]: https://github.com/ro-ag/ptrack/releases/tag/v0.16.1
[0.16.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.16.0
[0.15.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.15.0
[0.14.1]: https://github.com/ro-ag/ptrack/releases/tag/v0.14.1
[0.14.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.14.0
[0.13.1]: https://github.com/ro-ag/ptrack/releases/tag/v0.13.1
[0.13.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.13.0
[0.12.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.12.0
[0.11.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.11.0
[0.10.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.10.0
[0.9.2]: https://github.com/ro-ag/ptrack/releases/tag/v0.9.2
[0.9.1]: https://github.com/ro-ag/ptrack/releases/tag/v0.9.1
[0.9.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.9.0
[0.8.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.8.0
[0.7.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.7.0
[0.6.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.6.0
[0.5.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.5.0
[0.4.2]: https://github.com/ro-ag/ptrack/releases/tag/v0.4.2
[0.4.1]: https://github.com/ro-ag/ptrack/releases/tag/v0.4.1
[0.4.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.4.0
[0.3.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.3.0
[0.2.1]: https://github.com/ro-ag/ptrack/releases/tag/v0.2.1
[0.2.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.2.0
[0.1.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.1.0
