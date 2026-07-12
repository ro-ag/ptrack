# ptrack — Plan Tracker for AI Agents

`ptrack` persists AI planning state (goal, plans, tasks, notes, rolling context
summary) in an embedded database so a **fresh** agent session can reload enough
context to continue where a previous, oversized session left off.

## Why

Long AI coding sessions grow too large to sustain. Restarting loses the goal,
the plan in flight, and decisions already made. `ptrack context` hands a new
agent a compact digest of exactly that state.

## Install

```sh
go install github.com/ro-ag/ptrack@latest
```

Or download a prebuilt binary from the [releases page](https://github.com/ro-ag/ptrack/releases).

Requires Go 1.26+ to build from source.

## Two interfaces, one store

- **AI agents → subcommands.** Scriptable, non-interactive:
  `init`, `goal`, `summary`, `plan`, `task`, `note`, `context`, `status`.
- **Humans → Bubble Tea TUI.** Interactive dashboard for browsing and editing
  plans, tasks, goal, summary, and notes. Launch with bare `ptrack`.

## Quick start

```sh
ptrack init --goal "Ship the widget service"
ptrack plan add "Build storage layer"
ptrack plan use 1
ptrack task add "Define bbolt buckets" --plan 1
ptrack task start 1
ptrack note add "chose bbolt over badger" --task 1
ptrack context            # fresh agent reads this (add --json for machines)
```

## Commands

| Command | Purpose |
|---|---|
| `ptrack init [--goal S]` | Create `.ptrack/` in the project root, set the goal |
| `ptrack goal [show\|set S]` | Show or set the north-star goal |
| `ptrack summary [show\|set S]` | Show or set the rolling context summary |
| `ptrack plan add\|list\|done <id>\|use <id>` | Manage plans; `use` sets the active plan |
| `ptrack task add\|list\|start\|done\|block` | Manage tasks (default to the active plan) |
| `ptrack note add S [--task N\|--plan N]` | Attach a note to the project, a plan, or a task |
| `ptrack context [--json]` | Print the restore digest (Markdown, or JSON) |
| `ptrack status` | Quick overview: goal, active plan, task counts |
| `ptrack projects` | List projects in the global registry |
| `ptrack backup` | Copy the project DB into the global backups directory |
| `ptrack version` | Print the version |

## TUI keys

`tab` switch pane · `↑/↓` move · `a` add plan/task · `n` note · `g` edit goal ·
`m` edit summary · `u` set active plan · `x` mark plan done · `s/d/b`
start/done/block task · `r` reload · `B` backup · `q` quit.

## Storage

- **Project store** — `.ptrack/ptrack.db` in the project root (discovered by
  walking up from the working directory, like `.git`).
- **Global store** — `~/.ptrack/global.db` for config, project registry, and
  backups. Override the location with `PTRACK_HOME`.

Values are `encoding/gob`-encoded structs in [bbolt](https://github.com/etcd-io/bbolt).
JSON appears only at `--json` output boundaries.

## Development

```sh
go build ./...
go test ./...
```

Design docs live in [`docs/superpowers/`](docs/superpowers/).

## License

[Apache License 2.0](LICENSE) © 2026 ro-ag.
