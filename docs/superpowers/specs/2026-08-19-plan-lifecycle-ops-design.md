# Plan lifecycle operations — design

Date: 2026-08-19
Status: approved

## Problem

Plans can be created, renamed, held, claimed, and finished — but never deleted,
never copied, and never moved to another project. Junk plans accumulate
forever, and work that belongs in a different repository's project cannot be
relocated. The Desktop GUI additionally has no rename affordance at all.

## Goals

- `plan delete`: permanently remove a plan and its children, guarded.
- `plan move`: relocate a plan subtree to another registered project.
- `plan copy`: duplicate a plan subtree into another project or the same one.
- Rename on arrival (`--as`) for move/copy.
- Full set in the Desktop GUI: rename, delete, move, copy, with a project
  picker.
- No payload-schema change: these are operations, not new record shapes.

## Non-goals

- Archive-style soft delete (plans already have the `archived` status).
- An export/import interchange file format (P3 journals will own portability).
- Moving milestones, or anything project-global (goal, summary, capabilities).
- Journal/tombstone semantics — when a project later becomes share-enabled
  (P3), delete must degrade to a tombstone per the multi-dev sync spec; that
  arrives with P3, not here.

## CLI surface

- `ptrack plan delete <id> --force`
  Non-interactive ethos kept: without `--force` the command refuses and
  prints exactly what would be destroyed (task count, note count, issue links
  to be detached). With `--force` it deletes and prints the same summary.
- `ptrack plan move <id> --to <project> [--as "<title>"]`
  `<project>` is a registered project name or path as shown by
  `ptrack projects`. `--as` renames the plan on arrival.
- `ptrack plan copy <id> [--to <project>] [--as "<title>"]`
  Omitting `--to` copies within the current project (duplicate); `--as` is
  required in that case (two identical titles are confusing, and the flag is
  the natural place to force the choice).

All three are content mutations: the claim gate applies (operating on a plan
claimed by someone else is refused with the standard claimed-by message).

## Delete semantics

Cascade, in one write transaction on the project store:

- The plan's tasks are deleted.
- Notes attached to the plan or its tasks are deleted.
- Issues linked to the plan's tasks are detached (issue survives, its task
  link zeroed) — issues may describe real bugs that outlive the plan. The
  summary output lists each detached issue.
- Commit records survive as audit trail with their plan/task references
  zeroed (same treatment `convert_task_to_plan` gives them).
- Milestone membership: the plan is removed from its milestone.
- Active-plan pointers: every per-actor active-plan entry and the legacy
  singleton that point at the deleted plan are reset to 0.
- Claim gate: deleting a plan claimed by another identity is refused; the
  deleter's own claim dies with the plan.

## Move/copy engine

One `ptrack` process opens the source and the target project stores (approach
A — direct store-to-store; no intermediate file format).

- Copy phase (both move and copy): in a single write transaction on the
  target store, remint sequential IDs for the plan and every child, remap all
  references (task.plan_id, note targets, issue↔task links, commit-record
  plan/task ids), and insert. The plan arrives unclaimed (claim_owner None,
  claim_epoch 0). Hold reasons travel. The milestone link is dropped (a
  milestone is a source-project grouping). `--as` replaces the title at
  insert time.
- Issues follow their task: a moved task's linked issue moves with it; a
  copied task's issue is duplicated into the target.
- Commit records travel with the plan on move and are duplicated on copy;
  their git SHAs are foreign context in the target project but remain useful
  audit history.
- Delete phase (move only): only after the target transaction has committed
  does the source store run the delete cascade (same code path as
  `plan delete`, without the `--force` ceremony — the move already succeeded).
  A crash between the two phases leaves a visible duplicate and loses
  nothing; re-running the move is safe to abort and clean up by hand
  (`plan delete` in whichever side is unwanted). The command's summary names
  both projects and the new plan id.
- Both stores get normal actor stamping and actor-directory upserts.

## Guards

- The target must be a registered project whose database this binary can
  open (exact store-schema match, payload schemas within accepted range);
  otherwise refuse with the standard fail-closed manifest error plus a hint
  to upgrade.
- Moving a plan you have claimed releases your claim in the source (the plan
  ceases to exist there); the target copy is born unclaimed.
- Source plan id must exist; target project must not be the source for
  `move` (that is a no-op rename at best — refuse and point at `plan rename`).
- `copy` without `--to` and without `--as` is refused.

## Desktop GUI

Plan context menu (sidebar and board header): Rename, Delete, Move, Copy.

- Rename: inline edit, Enter commits, Escape cancels.
- Delete: confirmation dialog showing the cascade counts (tasks, notes,
  issues to detach) fetched from a new preview query; destructive button
  style.
- Move / Copy: dialog with a project dropdown (populated from the global
  registry) and an optional new-title field; disabled OK until a target is
  chosen.
- Five new Tauri commands added to the allowlist: `rename_plan`,
  `delete_plan`, `move_plan`, `copy_plan`, `list_projects` — thin wrappers
  over the same ptrack-app service mutations the CLI uses. Claim/hold
  refusals and guard errors surface in the dialog as the store's own message.
- These are the GUI's first plan-level content mutations; they reuse the
  existing desktop runtime mutation plumbing (agent-task-ownership precedent).

## Testing

- Store: cascade-delete completeness — after delete, sweep every collection
  and assert nothing references the dead plan or its tasks; move remap
  integrity — every reference in the target points at reminted IDs, zero
  dangling, source unchanged on copy; crash-window simulation — target
  committed + source intact is a legal, recoverable state; copy-with---as
  produces an independent subtree (mutating the copy leaves the original
  untouched); claim-gate refusals on all three ops; active-plan pointer
  cleanup on delete/move.
- CLI: all new commands/flags registered in the five registration lists;
  refusal paths (`--force` missing, `--to` missing, unregistered target,
  claimed plan); summary output shape.
- GUI: desktop-runtime tests per command (including the preview query);
  frontend dialog tests following the existing vitest patterns; allowlist
  count updated.
- Docs: guide.rs, help reference + desktop pages, CHANGELOG [Unreleased];
  README command table rows.
