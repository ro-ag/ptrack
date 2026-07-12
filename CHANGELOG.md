# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[0.2.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.2.0
[0.1.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.1.0
