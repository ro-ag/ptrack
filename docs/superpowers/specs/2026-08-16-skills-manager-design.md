# Skills, Plugins, and Rules Manager — Design

Date: 2026-08-16
Status: approved direction, decomposed into three sequential plans

## Problem

Agent tooling accumulates skills, plugins, rules, hooks, and instruction files
across ecosystems (Claude Code, Codex, Cursor). Nothing shows what is
installed, what every session silently pays for in context tokens, which items
overlap or conflict, or how to manage one canonical set across agents. p-track
already positions itself as the local agent control plane; this feature extends
that to the configuration the agents run with.

## Decisions (from brainstorming)

- **Scope:** full lifecycle (inventory → audit → manage/sync), decomposed into
  three plans that each ship standalone value.
- **Evaluator:** reuse installed agent CLIs (`claude -p`, `codex exec`,
  `cursor-agent`, …) as the LLM for saturation/overlap judgment. No API keys in
  p-track, agent-agnostic by construction.
- **Ecosystems v1:** Claude Code, Codex, Cursor. Adapter trait keeps the set
  open.
- **Cost signal:** deterministic static token footprint of always-loaded
  artifacts (instruction files, rules, hook output, skill frontmatter). The LLM
  judges overlap/saturation on top of those numbers; it never invents them.
- **Surfaces v1:** CLI (scriptable, `--json`) and Desktop workspace panel. TUI
  later.
- **Source of truth (lifecycle stage):** a p-track canonical library;
  per-agent directories become materialized output (write or symlink). Existing
  items are adopted into the library, not moved silently.

## Architecture

New crate `ptrack-skillset` (working name), used by CLI, Desktop, and later
TUI. Follows the existing capability pattern: pure scanning/normalization
inward, persistence and process execution at the facade.

- **Adapter trait** per ecosystem: discovers artifacts, classifies them
  (skill / plugin / rule / hook / instruction / command), scopes them
  (user-global vs project), and reports load semantics (always-loaded vs
  triggered).
- **Normalized model** persisted in the existing store, keyed by content hash
  so rescans are idempotent and drift is detectable.
- **Footprint engine:** tokenizes artifact content with a local tokenizer,
  aggregates per scope: "every session in this project starts N tokens deep."
- **Evaluator runner:** detects installed agent CLIs, runs one non-interactive
  prompt with the inventory + footprint table, expects a JSON verdict
  (overlaps, conflicts, stale candidates, saturation grade). Runs only on
  explicit user action; output stored as an audit report.
- **Canonical library (plan 3):** versioned artifact store under p-track's
  data dir; materializers translate to each agent's on-disk format. Dry-run
  and backup before any mutation of agent directories.

## Sequential plans

1. **Inventory** — adapters, normalized model, persistence, `ptrack skills
   list/show`, Desktop inventory panel.
2. **Audit** — token footprint, cost ranking, evaluator runner, saturation
   report, `ptrack skills audit`, Desktop report view.
3. **Library** — canonical store, adopt/install, per-agent materialization,
   enable/disable, sync + drift detection, management UI.

## Error handling

- Missing/unreadable agent dirs: skip with a per-adapter warning, never fail
  the whole scan.
- Evaluator CLI absent or non-JSON output: audit degrades to the deterministic
  footprint report and says so.
- Materialization (plan 3) is transactional per artifact: backup, write,
  verify, else restore; partial sync reports exactly what changed.

## Testing

- Fixture directory trees per ecosystem; adapter contract tests, tolerant of
  Windows paths and line endings.
- Golden footprint fixtures with a pinned tokenizer.
- Evaluator runner tested against a stub CLI; JSON-schema validation of
  verdicts.
- Materializer round-trip tests (adopt → materialize → rescan detects no
  drift).

## Out of scope (v1)

- Session-log/transcript parsing for realized token spend (revisit after
  inventory + static footprint exist).
- Marketplace/registry of shared skills; cross-machine sync.
- Ecosystems beyond the three above.
