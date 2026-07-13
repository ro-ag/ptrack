# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
