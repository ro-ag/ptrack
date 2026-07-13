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

Every read command supports `--json` (Markdown is the default — fewer tokens for
an LLM to read).

| Command | Purpose |
|---|---|
| `ptrack init [--goal S] [--root D] [--force] [--no-guide]` | Create `.ptrack/` (refuses to nest unless `--force`) and write the agent guide |
| `ptrack guide [--print]` | Install/refresh the agent guide in AGENTS.md/CLAUDE.md, or print it |
| `ptrack goal [show\|set S]` | Show or set the north-star goal |
| `ptrack summary [show\|set S]` | Show or set the rolling context summary |
| `ptrack milestone add\|list\|show <id>\|done\|open\|due` | Milestones (checkpoints grouping plans); `add --due YYYY-MM-DD` |
| `ptrack plan add [--milestone N]\|list\|show\|done\|use` | Manage plans; `show` includes tasks + notes |
| `ptrack task add\|list [--status …]\|show <id>\|start\|done\|block` | Manage tasks; `list --status todo,doing,…` filters |
| `ptrack issue add [--severity S] [--task N]\|list [--status]\|show\|close\|open\|severity` | Track issues/bugs |
| `ptrack note add S [--task N\|--plan N]` / `note list [--plan\|--task\|--limit]` | Attach or list notes |
| `ptrack context [--json]` | Bounded restore digest: goal, summary, active plan, blockers, open issues, recent notes, inventory |
| `ptrack next [--json]` | The single most-actionable task (active plan: doing, else todo) |
| `ptrack board [--plan N] [--json]` | Kanban view of a plan's tasks by status |
| `ptrack search <term> [--json]` | Substring match across plan/task titles and note bodies |
| `ptrack status [--json]` | Quick overview: goal, active plan, task counts |
| `ptrack projects [--json]` | List projects in the global registry |
| `ptrack backup` | Copy the project DB into the global backups directory |
| `ptrack version` | Print the version |

### Agent workflow

A fresh agent resuming a large project reads `ptrack context` (bounded — it never
dumps the whole project, just the live edge plus counts and drill-down commands),
then pulls detail on demand with `next`, `milestone show`, `plan show`,
`task show`, `task list --status`, `issue list`, `note list`, `search`, and
`board`. It records decisions with `note add` and updates `summary set` before
the session ends.

**Populating an existing repo:** point the agent at the repo and have it seed
ptrack — `goal set` the north star, `milestone add` the checkpoints, `plan add
--milestone N` the workstreams, `task add` the work (with `start`/`done`/`block`
status), `issue add` known bugs, `note add` decisions. The agent guide written
into `AGENTS.md`/`CLAUDE.md` spells this out.

### Agent guide (onboarding)

`ptrack init` writes a short, marker-delimited **ptrack section** into the
project's `AGENTS.md` and `CLAUDE.md` (creating them if absent, preserving any
existing content), teaching any AI agent this workflow. Refresh it anytime with
`ptrack guide`; print it with `ptrack guide --print`. Skip it at init with
`--no-guide`. The block is idempotent — re-running only rewrites when the guide
changes.

**Your own working agreements:** put a Markdown file at
`~/.ptrack/guide.md` (or `$PTRACK_HOME/guide.md`) and its contents are appended
inside the block, after the built-in guide. Use it for personal conventions
(branch-first, no AI attribution, release rules, …) so every project you `init`
carries them — without changing what ptrack ships to others.

## TUI keys

**List mode:** `tab` switch pane · `↑/↓` move · `v` board · `a` add plan/task ·
`n` note · `g` edit goal · `m` edit summary · `u` set active plan · `x` mark plan
done · `s/d/b` start/done/block task · `r` reload · `B` backup · `q` quit.

**Board mode:** `←/→` column · `↑/↓` card · `H/L` move card across columns
(changes status) · `a` add · `n` note · `v` back to list · `q` quit.

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
