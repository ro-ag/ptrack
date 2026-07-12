# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[0.1.0]: https://github.com/ro-ag/ptrack/releases/tag/v0.1.0
