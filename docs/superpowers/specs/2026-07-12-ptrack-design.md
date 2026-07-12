# ptrack — Plan Tracker for AI Agents

**Status:** Design approved (pending spec review)
**Date:** 2026-07-12
**Owner:** ro-ag

## Problem

Long AI coding sessions grow too large to sustain. When a session must restart
fresh, the new agent loses all context about the project's goal, the plan in
flight, and decisions already made. `ptrack` is a CLI that persists this
planning state so a fresh agent can reload enough context to continue seamlessly.

## Users & Interfaces

Two distinct consumers, two distinct interfaces over the **same** data:

- **AI agents → subcommands.** Scriptable, non-interactive commands
  (`init`, `goal`, `summary`, `plan`, `task`, `note`, `context`, `status`).
  Agents read state with `ptrack context` and record progress with
  `plan`/`task`/`note`. Never blocks on a TTY.
- **Humans → Bubble Tea TUI.** Interactive control center for setup/config,
  backups & restore, project registry management, and browsing/editing plans
  and tasks. Launched via `ptrack` (no args) or `ptrack ui`.

The TUI is a convenience layer for the user; the subcommands are the source of
truth and the agent-facing API. Every mutation possible in the TUI is also
possible via a subcommand, so scripted use never depends on the TUI.

## Storage

Two tiers. Project scope outranks global scope.

### Project store — `.ptrack/ptrack.db`

- Lives in the project root, discovered by walking up from cwd until a
  `.ptrack/` directory or a `.git/` directory is found. If neither exists,
  `ptrack init` creates `.ptrack/` in cwd.
- Travels with the repository (may be committed or gitignored — user choice).
- Holds the actual planning data for one project.

### Global store — `~/.ptrack/global.db`

- Cross-cutting concerns: config/options, a registry of known projects,
  backup bookkeeping, and reusable templates ("skills", reserved for later).
- Provides defaults and services (backup/restore, project list); never holds
  a project's plan data.

### Encoding

- **bbolt** single-file key/value DB per tier.
- Values are Go structs serialized with **`encoding/gob`** (native, compact,
  fast). gob is internal-only.
- **JSON** appears only at the `--json` output boundary of `context` (and any
  other command that grows a `--json` flag). The DB never stores JSON.

## Data Model

### Project DB buckets

- `meta` — singleton record:
  - `Goal` — the north-star design/main goal (free text).
  - `Summary` — rolling context summary maintained across sessions.
  - `CreatedAt`, `UpdatedAt`.
- `plans` — `Plan{ ID, Title, Status, Order, CreatedAt, UpdatedAt }`
  - `Status ∈ { active, done, archived }`.
- `tasks` — `Task{ ID, PlanID, Title, Status, Order, CreatedAt, UpdatedAt }`
  - `Status ∈ { todo, doing, done, blocked }`.
- `notes` — `Note{ ID, Target, TargetID, Body, CreatedAt }`
  - `Target ∈ { project, plan, task }`; `TargetID` is the plan/task ID (0 for project).

IDs are per-project incremental integers via bbolt `NextSequence`, giving
human-friendly refs (`plan 1`, `task 3`). One plan may be the "active" plan;
`plan use <id>` sets it (stored in `meta`).

### Global DB buckets

- `config` — key/value options (backup dir, default editor, TUI prefs).
- `projects` — registry mapping project path → `{ Name, Path, LastSeen }`.
- `templates` — reserved for reusable plan/task templates ("skills"). Schema
  present, no commands in MVP.
- `backups` — records of backups taken (path, timestamp, project).

## Subcommand Surface (agent-facing)

```
ptrack init [--goal "..."]        # create .ptrack/, register project, set goal
ptrack goal    [show | set "..."] # main design/goal
ptrack summary [show | set "..."] # rolling context summary
ptrack plan    add "title"        # add plan
           list                   # list plans + status
           done <id>              # mark plan done
           use  <id>              # set active plan
ptrack task    add "title" [--plan <id>]   # add task (defaults to active plan)
           list [--plan <id>]     # list tasks
           start <id>             # status → doing
           done  <id>             # status → done
           block <id>             # status → blocked
ptrack note    add "text" [--task <id> | --plan <id>]  # attach note
ptrack context [--json]           # RESTORE digest (see below)
ptrack status                     # quick human/agent overview
ptrack projects                   # list registered projects (global)
ptrack backup                     # back up current project DB (global service)
```

### `ptrack context` — the restore command

The single most important command. Emits a token-efficient **Markdown** digest
for a fresh agent to read at session start:

1. Project goal.
2. Rolling context summary.
3. Active plan with its open tasks (todo/doing/blocked), in order.
4. Recent notes/decisions (bounded count).

`--json` emits the same tree as structured JSON for programmatic consumers.
Markdown is the default because the primary consumer is an LLM reading prose.

## Interactive TUI (human-facing)

Built with **bubbletea** (Elm architecture) + **lipgloss** (styling) +
**bubbles** (components). Responsibilities:

- Setup/config editing (writes global `config`).
- Backups & restore (invokes the same backup service as `ptrack backup`).
- Project registry: list, open, forget.
- Browse and edit plans/tasks/notes for the current project.

The TUI is read/write but optional; it is never on the critical path for agent
usage. `status`, `list`, and `context` also use lipgloss for readable
non-interactive output.

## Layout

```
main.go                   # entry: dispatch to cobra
internal/model/           # structs + gob (de)serialization helpers
internal/store/           # bbolt project store + global store, discovery, CRUD, backup
internal/cli/             # cobra commands (agent-facing subcommands)
internal/tui/             # bubbletea dashboard, lipgloss styles
```

Boundaries: `cli` and `tui` both depend on `store`; `store` depends on `model`.
`model` depends on nothing project-internal. No import cycles.

## Dependencies

All pure Go (no CGO):

- `github.com/spf13/cobra` — subcommand parsing.
- `go.etcd.io/bbolt` — embedded KV store.
- `github.com/charmbracelet/bubbletea` — TUI runtime.
- `github.com/charmbracelet/lipgloss` — styling.
- `github.com/charmbracelet/bubbles` — TUI components.

## Error Handling

- Store operations return wrapped errors (`fmt.Errorf("...: %w", err)`).
- Missing project (`.ptrack/` not found) → clear message pointing to
  `ptrack init`, non-zero exit.
- Bad IDs / unknown refs → explicit "plan 5 not found", non-zero exit.
- All subcommands set a non-zero exit code on failure so agents can detect it.

## Testing

- Go stdlib `testing`, table-driven where natural.
- Sibling `*_test.go` files (no separate test packages required).
- Each store test opens a bbolt DB in a `t.TempDir()`; no shared global state.
- `context` output covered by golden-ish assertions on Markdown and JSON.

## MVP Cuts (YAGNI)

- **Milestones layer** — dropped; plan→task→note is enough.
- **Multi-project switching within one DB** — dropped; per-directory scoping +
  global registry covers it.
- **Templates / "skills"** — schema bucket reserved, no commands.
- **Archive workflow** — just a status value, no dedicated flow.
- **Backup** — simple file copy of the project DB to the global backup dir.
- **TUI** — v1 covers config, backups, registry, and browse/edit; no advanced
  views.

## Success Criteria

A fresh agent, given only `ptrack context` output, can state the project goal,
the active plan, what is done, what is open, and the key decisions — enough to
resume work without re-reading the whole codebase.
