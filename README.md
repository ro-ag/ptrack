# ptrack — Plan Tracker for AI Agents

`ptrack` persists AI planning state (goal, plans, tasks, notes, rolling context
summary) in an embedded database so a **fresh** agent session can reload enough
context to continue where a previous, oversized session left off.

## Why

Long AI coding sessions grow too large to sustain. Restarting loses the goal,
the plan in flight, and decisions already made. `ptrack context` hands a new
agent a compact digest of exactly that state.

## Two interfaces, one store

- **AI agents → subcommands.** Scriptable, non-interactive:
  `init`, `goal`, `summary`, `plan`, `task`, `note`, `context`, `status`.
- **Humans → Bubble Tea TUI.** Interactive control center for config, backups,
  the project registry, and browsing/editing plans. Launch with `ptrack`.

## Storage

- **Project store** — `.ptrack/ptrack.db` in the project root (discovered by
  walking up from the working directory, like `.git`).
- **Global store** — `~/.ptrack/global.db` for config, project registry,
  backups, and reusable templates.

Values are `encoding/gob`-encoded structs in [bbolt](https://github.com/etcd-io/bbolt).
JSON appears only at `--json` output boundaries.

## Status

Early development. See the design spec in
[`docs/superpowers/specs/`](docs/superpowers/specs/).

## Quick start (planned)

```sh
ptrack init --goal "Ship the widget service"
ptrack plan add "Build storage layer"
ptrack task add "Define bbolt buckets" --plan 1
ptrack context            # fresh agent reads this
```
