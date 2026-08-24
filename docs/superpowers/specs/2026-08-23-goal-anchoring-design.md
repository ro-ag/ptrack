# Goal anchoring and integration gates — design

Date: 2026-08-23
Status: approved direction, pending spec review

## Problem

Agents follow ptrack plans well but exhibit two failure modes:

1. **Goal drift.** The north-star goal is read once (in `ptrack context`) and
   never re-surfaced. At the moments an agent actually decides what to do —
   `ptrack next`, `ptrack task show`, closing a task or plan — output shows the
   local item only. Across handoffs the receiving agent inherits tasks without
   the why and treats the plan as a checklist.
2. **Orphan features.** `task done` records that code exists, not that anything
   calls it. Features get built and never wired into the program, and nothing
   in the workflow forces the "what integrates this?" question. Commit
   metadata (post-commit hook, `#<task-id>` links, `commits_by_task`) already
   exists but is never surfaced or required at close time.

Gates are mandatory: closing work without evidence is an error, and an agent
cannot escape an unfinished task by opening new work. `--force` is the single,
audited override.

## Design

Six pieces, shipped together.

### 1. Goal echo at decision points

`ptrack next` and `ptrack task show` print the goal as the first line of the
markdown (and a `goal` field in `--json`):

```
Goal: Ship the widget service
```

If no goal is set, the line is omitted. Touches `next()` and `show_task()` in
`crates/ptrack-core/src/views.rs` (`NextView` / `TaskShow` structs plus their
`markdown()` renderers).

### 2. Task close gate: summary + linked commit

`ptrack task done <id>` is refused with an error unless both hold:

- `--summary "<what changed, where it is wired in, what remains>"` is given.
  Stored as a note attached to the task with the new closeout kind; rendered
  in `task show` and included among recent notes in `context`.
- At least one commit is linked to the task (existing `commits_by_task` query;
  links come from the post-commit hook via `#<task-id>` or `ptrack commit
  record`). The error explains both linking paths.

On success the command prints `Linked commits: N`.

`--force` closes the task anyway and records an override note on the task
("closed without summary/commit via --force"), so the audit trail shows the
gate was bypassed. Legitimate uses: abandoned work, external changes.

### 3. Auto integration task per plan

`ptrack plan add "<title>"` automatically appends a final task to the new
plan:

```
Integrate and verify against goal: <goal text>
```

- `--no-verify-task` skips it.
- Without a goal set, the fixed title "Integrate and verify against the
  project goal" is used.
- The task is created through the same path as `task add` (system-created, so
  the WIP gate in piece 5 does not apply to it); dependency, hold, and note
  behavior are unchanged.

### 4. Plan close gate + checkpoint

`ptrack plan done <id>` is refused with an error while the plan has open
tasks; the error lists them. `--force` closes anyway and records an override
note on the plan.

On successful close, the command prints a re-evaluation block instead of a
bare confirmation:

```
Plan #4 done.

Goal: Ship the widget service
Rolling summary: <current summary>
Remaining open plans: #5 storage layer, #6 API surface
Open issues: 2 (1 high)
Milestone: v1 — 2/4 plans done

CHECKPOINT — before continuing, re-evaluate:
- Does the remaining roadmap still reach the goal? Missing plans? Obsolete ones?
- What did this plan change that the next plans must know?
- Update: ptrack summary set "..." | ptrack plan add "..." | ptrack issue add "..."
```

- Milestone line appears only when the plan belongs to a milestone.
- The same block is available on demand as `ptrack checkpoint [--json]` so a
  handoff agent (or the guide) can request the whole picture at any time.
- `--json` output carries the structured fields (goal, summary, open plans,
  open issue counts, milestone progress); the CHECKPOINT prose is
  markdown-only.

### 5. WIP gate: finish before opening new work

While a started (in-progress) task exists, these commands are refused with an
error naming that task and how to resolve it (close it properly, or park it
with `task hold`/`task block` with a reason):

- `task start` (a second task)
- `task add`
- `plan add`

`--force` on each bypasses the gate and records an override note on the new
item. Explicit parking stays free — `task hold`, `task block`, `note add`,
`issue add`, and all read commands are never gated, so recording problems and
re-planning are always possible.

Scope: when a per-machine identity is configured, the gate considers only
started tasks in plans claimed by that identity (another agent's in-progress
work never blocks you). Without identity, the gate is project-wide, matching
the existing single-active-plan behavior.

### 6. Playbook (agent guide) update

`guide_body()` in `crates/ptrack-core/src/guide.rs` — the managed
`AGENTS.md`/`CLAUDE.md` section installed by `ptrack init`/`ptrack guide` — is
rewritten to teach the gated workflow as the contract, not advice:

- Closing a task requires `--summary` answering "what calls this now?" and a
  linked commit (`#<task-id>` in the message, hook installed); `task done`
  errors otherwise.
- One task in progress at a time: finish or park (`task hold`/`task block`
  with a reason) before `task start`, `task add`, or `plan add`.
- Every plan ends with its integration task; a plan cannot close with open
  tasks.
- After every `plan done`, act on the CHECKPOINT block: re-evaluate the
  roadmap against the goal, refresh `summary set`, add/adjust plans and
  issues. `ptrack checkpoint` re-prints it on demand.
- `--force` exists for genuine exceptions only and is recorded in the audit
  trail.

Existing installs pick the new text up via `ptrack guide` (marker-delimited
section replacement, already implemented). The README agent-workflow section
and the help-site pages that describe the close/next workflow are updated to
match.

## Data flow

All pieces read the existing `ProjectSnapshot` (goal, summary, plans, tasks,
issues, milestones, notes, commits). Writes: new note kinds (closeout,
override) and one auto-created task per plan. Gate checks live at the
mutation boundary (`ptrack-store`) so CLI, TUI, GUI, and MCP all enforce the
same rules; the CLI renders the friendly errors. Desktop GUI and TUI render
closeout/override notes and the auto task like any other note/task.

## Error handling

- Missing goal: goal echo omitted; integration-task title falls back to fixed
  text; checkpoint prints `Goal: (not set)` with a hint to `ptrack goal set`.
- `--summary` on statuses other than done: rejected with usage error.
- Gate errors are single, specific messages that name the blocking item and
  the exact commands that resolve the state.
- `checkpoint` outside an initialized project: same getting-started screen as
  other read commands.

## Testing

Go-style sibling `_test.rs` files per repo convention:

- views: goal line present/absent in `next` and `task show` markdown and JSON.
- store: task done refused without summary/commit and allowed with both;
  plan done refused with open tasks; WIP gate blocks task start/add and plan
  add with a started task, scoped by identity; `--force` paths record
  override notes.
- dispatch: closeout note created from `--summary`; linked-commit count
  printed; integration task appended by `plan add` and skipped by
  `--no-verify-task`.
- checkpoint view: block content with/without milestone, goal, summary; JSON
  shape.
- guide: managed section update covered by existing guide tests; refreshed
  install replaces only the marker-delimited section.

## Out of scope

A separate needs-verify status, plan intent fields, drift scoring, CI
verification of commits, any Desktop/TUI surface work beyond default
rendering.
