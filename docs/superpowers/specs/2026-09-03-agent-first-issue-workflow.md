# Agent-first issue workflow contract

## Outcome

p-track issues are durable intake records that agents can create and maintain
without silently changing the project's execution order. The desktop gives the
user a complete inbox for reviewing those records and an explicit scheduling
action that turns an issue into normal plan work.

## Authority and lifecycle

- An issue owns its title, detailed body/evidence, severity, open/closed
  status, timestamps, and an optional link to one task.
- Creating or editing an issue, closing or reopening it, and linking or
  unlinking an existing task mutate only the issue. They do not claim a plan,
  move a task, or change task status.
- An open issue with no task is **unscheduled**. It remains visible to agents
  as triage context, but `ptrack next` never selects it as work.
- An issue linked to a task is **scheduled**. The task remains the sole unit of
  execution: plan selection, dependency checks, holds, claims, and task status
  continue to govern whether `ptrack next` can select it.
- Scheduling an unscheduled issue into a plan creates one todo task and links
  the issue to it in the same store transaction. The new task title defaults
  to the issue title and may be overridden by the caller. Creating that task
  obeys the target plan's claim.
- Linking an existing task replaces any previous link atomically. Unlinking
  clears only the link. Neither operation deletes, moves, reopens, or closes a
  task or issue. Link mutations compare the caller's observed link with the
  durable link and reject a stale multi-agent relink instead of overwriting it.
- Issue status is explicit. Completing a linked task does not auto-close its
  issue, and closing an issue does not complete its task. This keeps recovery
  from interrupted or partially verified fixes honest.
- Moving a linked task between plans carries the issue through the existing
  task identity. Deleting a plan keeps the existing safety contract: linked
  issues survive and become unscheduled. Moving or copying a plan keeps the
  existing move/copy contracts.

## Agent-facing surfaces

- `issue add` captures a title, body/evidence, severity, and optional task.
- `issue edit` updates title, body/evidence, severity, and status in one
  mutation. Existing focused commands remain compatible.
- `issue link`, `issue unlink`, and `issue schedule` expose the explicit
  association lifecycle. `schedule` is the only issue operation that creates
  plan work.
- `ptrack context` separates unscheduled open issues from scheduled open
  issues and labels unscheduled records as triage-only.
- `ptrack next` still selects only tasks. When the selected task has open
  linked issues, it prints their identifiers, severity, and titles beneath the
  task so the agent receives the reason for the work without a second lookup.
- JSON views carry the same structured distinction; consumers never infer it
  by parsing display text.

## Desktop surfaces

- A first-class **Issues** view lists open and closed issues, exposes severity,
  scheduling state, and linked task/plan, and opens a full-detail editor.
- The detail view can edit the report, severity, and status; link or relink to
  an existing task; unlink; or schedule into a chosen open plan.
- Target lists remain bounded; title or exact-ID search reaches plans and
  tasks beyond the initial suggestions. Moving a task between plans rejects
  pending resource admissions and live linked terminals or agents; stop or
  detach those resources before moving so their associations remain valid.
- Links navigate both directions: an issue can open its linked task and select
  that task's plan, while issue rows linked from a task open the issue detail.
- Every request carries the current workspace generation. Stale responses are
  ignored and every successful mutation refreshes the authoritative snapshot.
- Empty, loading, validation, claim-conflict, and disappeared-target states
  remain recoverable; no optimistic UI is treated as durable state.

## Non-goals

- Issues do not gain comments, attachments, multiple assignees, labels,
  dependencies, or multiple task links.
- p-track does not infer scheduling from severity and does not auto-create,
  auto-start, auto-complete, or auto-close work.
- This plan does not alter plan/task claim semantics, introduce a new database
  schema, or create a separate issue history table.

## Acceptance

1. Store tests prove atomic schedule/link/relink/unlink behavior, missing-target
   rollback, plan-claim enforcement for scheduling, and task/issue lifecycle
   independence.
2. CLI tests cover detailed capture/editing, scheduling, links, compatibility
   commands, JSON, context, and next-work projection.
3. Desktop runtime tests cover generation fences, complete issue detail, every
   mutation, stale/missing targets, and bounded list behavior.
4. Frontend tests and personal fixture inspection cover inbox filtering, full-detail editing, scheduling and
   link controls, issue-to-task and task-to-issue navigation, focus return,
   empty/error states, and accessibility labels.
5. The signed production desktop is exercised with an unscheduled issue and a
   scheduled issue: both are obvious in the inbox, the scheduled issue opens
   its task/plan, and the unscheduled issue can be scheduled without losing its
   report.
