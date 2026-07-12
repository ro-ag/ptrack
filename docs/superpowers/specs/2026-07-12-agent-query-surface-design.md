# ptrack — Agent Query Surface

**Status:** Design (pending spec review)
**Date:** 2026-07-12
**Owner:** ro-ag
**Builds on:** the v0.1.0 MVP (model/store/report/cli/tui).

## Problem

A fresh agent resuming a **large, long-running** project must reconstruct
"where we are" without blowing its context budget. The v0.1.0 read surface
(`context`, `status`, `plan list`, `task list`) either shows too little (no
drill-down, no filters, no `note list`, no search) or risks growing unbounded as
the project scales. We need a query surface designed from the consuming agent's
seat: a tiny, bounded orientation up front, then drill-down on demand, with
payload discipline throughout.

## Principles

1. **Bounded front page.** `ptrack context` payload stays roughly constant
   regardless of project size. Show the live edge (active plan, open tasks,
   recent decisions) plus **counts and pointers**, never full dumps.
2. **Progressive disclosure.** The agent pulls detail only where relevant via
   targeted commands, instead of receiving everything.
3. **Self-describing.** `context` ends with an inventory line and the exact
   commands to drill deeper, so a fresh agent needs no external documentation.
4. **Payload discipline.** Markdown default (fewest tokens for an LLM to read);
   `--json` opt-in for programmatic parse; filters and `--limit` on every list.

## Output format

- **Markdown** is the default for every read command — least token overhead for
  an LLM, native for prose (summary, note bodies).
- **`--json`** is available on every read command for machine consumers, using
  the same tree the Markdown renderer walks. JSON is indented for readability.
- No YAML, no GraphQL (a query language, not an output format; field-selection
  is served by flags and subcommands instead).

## Read commands

### `ptrack context [--json]` (enriched, still bounded)

The cold-start reload. Sections:

1. **Goal** — north-star text.
2. **Summary** — the curated "where we are" narrative.
3. **Active plan** — title + its open tasks (todo/doing/blocked), done omitted.
4. **Blocked tasks (project-wide)** — any blocked task in any plan, since blockers
   are the highest-signal "stuck" indicator. Bounded to a small count with a
   pointer if truncated.
5. **Recent decisions** — last 5 notes (bounded).
6. **Inventory footer** — counts and pointers, e.g.:
   `8 plans (6 done) · 210 tasks (180 done · 4 blocked · 26 open) · 34 notes`
   followed by the drill-down commands available.

Bounds (constants): `contextRecentNotes = 5`, `contextBlockedShown = 8`. When a
bounded section truncates, it prints a `… +N more (use <cmd>)` pointer.

### `ptrack next [--json]`

The single most actionable task: within the active plan, the first `doing` task,
else the first `todo` task. Prints the task (id, title, plan) or a clear "no
actionable task" message. This is the agent's "what do I do right now".

### `ptrack plan show <id> [--json]`

One plan in full: its fields, its tasks (all statuses, ordered), and notes
attached to that plan. `ErrNotFound` → non-zero exit.

### `ptrack task show <id> [--json]`

One task in full: its fields, its parent plan (id + title), and notes attached
to that task.

### `ptrack plan list [--json]`

Unchanged shape, plus per-plan open/done task counts and a `--json` flag.

### `ptrack task list [--status S[,S...]] [--plan N] [--json]`

Adds a `--status` filter accepting a comma-separated set
(`todo,doing,done,blocked`); combined with the existing `--plan`. Empty filter =
all. Invalid status value → usage error.

### `ptrack note list [--plan N | --task N] [--limit K] [--json]`

Lists notes, newest first, optionally scoped to a plan or a task (mutually
exclusive), bounded by `--limit` (default 20, 0 = all). Fills the gap where
decisions beyond `context`'s recent-5 were invisible.

### `ptrack search <term> [--json]`

Case-insensitive substring match across plan titles, task titles, and note
bodies. Returns, grouped by kind: `plan #id title`, `task #id title (plan N)`,
`note #id (target) …snippet…`. Bounded snippet length. No matches → empty output,
zero exit.

## Store additions

The current store already supports most of this. New/confirmed methods:

```go
// notes scoped queries (project-wide filtering done in report/cli for now):
func (s *Store) NotesByPlan(planID uint64) ([]model.Note, error)
func (s *Store) NotesByTask(taskID uint64) ([]model.Note, error)

// counts for the inventory footer, computed without materializing all rows:
type Counts struct {
	Plans, PlansDone                     int
	Tasks, TasksDone, TasksBlocked, TasksOpen int
	Notes                                int
}
func (s *Store) Counts() (Counts, error)
```

`NotesByPlan`/`NotesByTask` filter `ListNotes()` by `Target`+`TargetID`.
`Counts` iterates buckets once. Existing `ListPlans`/`ListTasks`/`ListNotes`/
`RecentNotes`/`GetPlan`/`GetTask` cover the rest. `task list --status` filtering
lives in the CLI over `ListTasks`/`ListTasksByPlan`.

## report package additions

The digest logic grows to feed both Markdown and JSON for the new views. Rather
than bloat one file, split by view:

- `report.go` — shared `build`-style assembly helpers + `Markdown`/`JSON` for
  `context` (enriched).
- `views.go` — assemblers + renderers for `next`, `plan show`, `task show`,
  `search`: each returns a struct plus `Markdown()`/`JSON()` pair, or the CLI
  renders simple lists directly.

Every renderer keeps the "counts and pointers, not dumps" rule.

## CLI changes

- Add `--json` to: `context` (has it), `status`, `plan list`, `plan show`,
  `task list`, `task show`, `note list`, `next`, `search`, `projects`.
- New subcommands: `next`, `search`, `plan show`, `task show`, `note list`.
- `task list` gains `--status`; `note list` gains `--limit` + `--plan`/`--task`.
- A shared `--json` output helper so every command renders consistently.

## Testing

- store: `NotesByPlan`/`NotesByTask`/`Counts` unit tests (temp-dir DBs).
- report/cli: `context` inventory footer content; `next` selection order
  (doing beats todo, active-plan scoped); `task list --status` filtering;
  `search` matches across kinds; `--json` shapes unmarshal as expected.
- Bounds: context truncation pointer appears past the limit.

## Non-goals (YAGNI)

- No YAML/GraphQL output.
- No full-text ranking in `search` (substring match is enough).
- No activity/audit log of status changes (the `summary` field is the curated
  narrative; a change-log is separate future work).
- No pagination cursors (`--limit` suffices at this scale).

## Success criteria

A fresh agent runs `ptrack context`, gets a small bounded orientation that names
the goal, where-we-are, the active work, blockers, recent decisions, and the
inventory + commands to go deeper — then uses `next`, `*/show`, `note list`,
`task list --status`, and `search` to pull exactly what it needs, in Markdown by
default or `--json` when parsing, all with bounded payloads.
