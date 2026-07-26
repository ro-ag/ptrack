# P-TRACK Project Workspace Design

**Status:** Reviewed
**Date:** 2026-07-26
**Owner:** ro-ag

## Goal

Make the desktop application the canonical P-TRACK project GUI. The process
must be able to start without a project, open and switch projects without a
restart, present one bounded project-intelligence snapshot, and reliably
dispose every resource owned by the previous project.

This milestone retains the existing vanilla Vite/TypeScript frontend and
single-pane terminal behavior from v0.14.1. It does not add terminal
tabs/splits, terminal search/notifications, releases, or unrelated Stage B/C
work.

## Product States

The durable application has five user-visible states:

1. **Welcome** — no project is open. Show an Open Project button and bounded
   recent registered projects.
2. **Loading** — a project is being resolved and its context is being built.
   Keep the previous project visible but inert during a switch.
3. **Project open** — board, overview, Git intelligence, terminal sessions, and
   registered agent runs all belong to one published project generation.
4. **Project error** — opening or refreshing failed. Preserve the error and
   offer retry, another project, or return to welcome.
5. **Project closed** — cleanup completed. Briefly announce closure, then
   present the welcome screen without exiting the process.

Starting `ptrack gui [PATH]` resolves PATH, defaulting to the current directory.
If PATH does not contain or sit beneath a P-TRACK project, Wails still starts
in Welcome. `ptrack board --gui [--plan ID]` remains a compatible alias for
`ptrack gui` in the current directory.

## Durable App and Generation-Scoped Context

Wails binds one durable `gui.App` for the process lifetime. The app owns only:

- the retained Wails context;
- a serialized open/close/switch coordinator;
- the monotonically increasing project generation;
- the currently published `WorkspaceContext`;
- the global recent-project reader;
- application-shutdown state.

Each `WorkspaceContext` owns exactly one project generation:

```text
WorkspaceContext
  generation
  context + cancel
  canonical project root and database path
  transient store-operation gate
  terminal manager and stream listener
  terminal exit monitors
  AgentRun registry, loopback integration server, descriptor, and lease sweeper
  in-flight Git/overview refresh work
  event subscriptions and operation wait groups
  idempotent bounded Close
```

Project stores remain transient: a binding opens and closes bbolt for one
operation. The context owns the ability to begin those operations, not a
long-held bbolt handle, preserving concurrent CLI/TUI access.

### Publication and switching algorithm

Open, close, and switch are serialized.

1. Reserve a transition/request ID. This is distinct from the published
   project generation and cannot invalidate the still-open project.
2. Resolve the selected directory to an existing P-TRACK database and
   canonical root.
3. Fence active-resource admission on the current context and capture its
   resource revision. If terminals or live AgentRuns are active, return a
   structured confirmation token containing the transition ID, published
   generation, and resource revision. New terminals/AgentRuns are rejected
   while the fence is held. Cancel explicitly releases the fence. Tokens and
   fences expire after 60 seconds, and a later serialized transition
   deterministically supersedes/releases an abandoned fence.
4. After confirmation, construct the candidate context completely. A
   construction failure bounded-closes every partial/unpublished resource,
   releases the old context's fence, and leaves it published.
5. Allocate the next published generation only at the atomic unpublish/publish
   point.
6. Cancel and close the old context with a fixed deadline.
7. Emit a generation-tagged workspace-changed event.

No old context remains published while its cleanup runs. Bindings capture a
context and generation through `BeginOperation`; once closing starts, new work
is rejected. Existing work observes context cancellation.

Every unpublished candidate is bounded-closed on every failure/cancellation
path. A cancelled picker or confirmation never creates a candidate and never
advances the published generation.

Close follows the same path without a candidate: require confirmation when
active work exists, atomically unpublish, cancel, bounded-close, and return
Welcome.

Application shutdown marks the durable app closed, unpublishes the current
context, and calls the same context close path. Close is safe to call multiple
times.

### Bounded cleanup

`WorkspaceContext.Close(ctx)` starts teardown once and is idempotent:

1. reject new operations and cancel the generation context;
2. stop accepting terminal streams and new terminal/AgentRun operations;
3. close sockets/listeners and terminate terminal sessions;
4. stop lease/exit monitor goroutines;
5. wait for owned work within resource-specific internal deadlines;
6. publish the joined cleanup result on one completion channel.

Each caller waits on the shared completion channel with its own context. A
caller's deadline returns only that caller's context error; it does not cache a
deadline as the teardown result or start duplicate cleanup. A later caller can
observe eventual completion and the final joined resource errors. Production
transition callers use a three-second deadline.

A timed-out caller is reported, but the new generation remains protected
because old callbacks carry the old generation and the old context is no
longer published.

Tests use fakes that deliberately block operations and cleanup to prove:

- cancellation occurs before waiting;
- duplicate close does not duplicate cleanup;
- one caller can time out, resources can unblock, and teardown can later
  complete successfully for another caller;
- each teardown step enforces its internal bound;
- in-flight old responses/events cannot mutate or emit as the new generation;
- repeated open/close/switch races leave one or zero published contexts.

## Generation-Safe Bindings and Events

Every workspace snapshot and low-frequency event includes `generation`.
Frontend requests capture the controller generation and discard responses when
it changes. Backend event emitters check that the originating context and
generation are still published before emitting.

Terminal session IDs remain opaque, but all terminal bindings resolve the
current context first. Terminal status/exit payloads gain a generation field.
A terminal ID from an old project cannot be resized or closed through a new
project context.

The frontend uses generation-aware V2 bindings for every workspace-scoped
operation: snapshot, add/move/rename/note, profile initialization, terminal
create/resize/close, and AgentRun operations. Existing binding names remain as
compatibility wrappers for older generated clients. V2 bindings capture and
validate the expected generation before admission. The frontend facade also
checks its generation after every await, so a delayed old mutation cannot
trigger a render, refresh, focus change, or terminal mount in the next
workspace.

The frontend workspace controller owns:

- one refresh interval handle;
- one request sequence per generation;
- one single-flight gate that coalesces explicit refresh requests;
- the terminal dock disposer;
- workspace-scoped DOM/event listeners;
- state transitions and focus restoration.

Disposal clears the interval, invalidates pending requests, disposes the
terminal dock, closes overlays, and removes listeners. Board shortcuts are
disabled whenever a project menu or dialog is visible or focus is inside a
terminal/dialog/menu.

## Project Actions and Recent Projects

The project toolbar and Welcome screen expose:

- **Open Project…** when no project is open;
- **Switch Project…** when a project is open;
- **Close Project** when a project is open;
- recent registered projects, newest first.

Open/Switch invokes Wails' native directory dialog. Selecting a nested
directory resolves upward using P-TRACK discovery. Cancelling makes no state
change. Recent projects are bounded to 20 entries and stale paths are labeled;
choosing one uses the same open/switch path.

The global registry remains a convenience. Opening succeeds even if registry
refresh fails. Discovery treats either a `.git` directory or Git worktree
`.git` file as a repository boundary.

Before switching/closing, the frontend asks the backend for the active-resource
summary. If confirmation is needed, the backend fences active-resource
admission and returns an opaque confirmation token tied to the generation and
resource revision. An accessible modal names terminal and AgentRun counts.
Confirm presents that token; stale/wrong tokens are rejected and re-prompted.
Cancel releases the fence and restores focus.
The retained workspace is inert while a transition or confirmation is active.
Welcome/error/closed states focus Open Project, while successful activation
focuses the project heading.

## One Bounded Workspace Snapshot

The Project open screen refreshes through one
`GetWorkspaceSnapshot(generation, planID)` binding. The result is immutable and
contains independently errorable sections:

```text
WorkspaceSnapshot
  generation, capturedAt, stale
  project {name, root, dbPath, storage status/version}
  tracking {goal, handoff, plans, selected/active plan, board tasks}
  blockers, issues, recent notes, recent P-TRACK activity, inventory
  terminals
  agentRuns
  git {state, status, refs, remotes, commits, divergence, stale branches}
  sectionErrors
```

Bounds:

- plans: 100;
- selected-plan board tasks: 300;
- project-wide blockers: 50;
- open issues: 50;
- recent notes: 50;
- recent P-TRACK activity: 24;
- terminal sessions: 64;
- AgentRuns: 64;
- Git remotes: 16;
- local branches: 100;
- remote branches: 150;
- recent commits: 40;
- unpushed commits: 40;
- changed paths considered per commit: 500;
- process output per Git command: 4 MiB;
- each Git command: three seconds;
- complete snapshot: eight seconds.

The Git section executes at most eight commands, sequentially, and captures at
most 12 MiB across the complete section in addition to each four-MiB command
limit.

Each bounded list includes a total or `more` count where available. Tracking
data and inventory are assembled from bounded cursor reads/single-pass counts,
not unbounded full-table materialization. A valid project with no active plan
still opens; it shows project intelligence and an empty board with guidance.
Selected-task note/commit/open-issue counts and latest-note context come from a
context-aware, bounded-memory association scan rather than the recent-activity
window. The eight-second deadline covers tracking assembly and the remaining
Git work.

Frontend section states are explicit:

- loading: retain the last snapshot and mark it stale;
- success: replace only for the matching generation;
- section error: retain last successful section as stale and show the error;
- first-load error: show an empty error state with retry.

## Read-Only Git Intelligence

Create `internal/gitinfo`, independent of GUI and store packages. It accepts a
context, root, clock, and command runner interface. The production runner uses
`exec.CommandContext` directly, never a shell, fixes Git's locale, applies
timeouts, captures stdout/stderr through limited writers, and returns a typed
output-limit error.

Every invocation prefixes `--no-optional-locks` and sets
`GIT_OPTIONAL_LOCKS=0`, `GIT_PAGER=cat`, and `GIT_TERMINAL_PROMPT=0`. This
prevents status/log inspection from refreshing the index, paging, or prompting.
Runner tests assert the exact argument prefix and environment.

The snapshot uses stable fields only:

- `git status --porcelain=v2 --branch -z --ignored=matching`
  for HEAD, detached state, upstream, ahead/behind, and staged/unstaged/
  untracked/conflicted/ignored counts;
- `git rev-parse` explicit flags for worktree/repository identity;
- `git for-each-ref --format=...` with NUL field separators, explicit refs,
  sort, and count limits for local/remote branches, worktree paths, upstream,
  object ID, and commit epoch;
- one NUL-delimited `git config --get-regexp` query for bounded remote
  fetch/push URLs, with `pushurl` falling back to `url`;
- `git log -n 40 --date=unix --format=... --name-only` with record/field
  separators for author, date, subject, refs, and bounded changed-area
  summaries;
- `git rev-list --left-right --count <upstream>...HEAD` and a bounded explicit
  log range for divergence/unpushed commits.

Porcelain v2 parsing is byte-oriented and NUL-aware. No parser consumes
localized human status/log text.

Stale local branches are non-current branches whose tip is older than 90 days.
The result says that this is an age signal, not proof a branch is safe to
delete. Divergence is reported only when an upstream exists. No Git command
mutates refs, index, config, remotes, or worktrees; this milestone has no
fetch/pull/push/checkout/commit actions or watchers.

Non-repositories produce an explicit `notRepository` state, not a project
error. Cancellation, timeout, output truncation, or parse failure appears in
the Git section while the tracking snapshot remains usable.

## AgentRun Registry

Create `internal/agentrun`, independent of GUI, terminal, and store packages.
It tracks only agents launched through a P-TRACK agent profile or explicitly
registered through the P-TRACK API. It never scans process names, terminal
titles, or arbitrary process tables.

An AgentRun has:

```text
id (stable opaque ID)
profile, explicit provider
pid, processState, leaseState
projectRoot, planID, taskID, terminalID, cwd
startedAt, lastActivityAt, lastHeartbeatAt
state
exit {code, result, occurredAt}
registrationKind (launched|external)
```

Launched agent profiles are registered when terminal creation succeeds and are
updated only from that owned terminal's output/activity and exit lifecycle.
The registry receives explicit terminal associations; it does not infer them.

External wrappers cannot call renderer-only Wails bindings, so each
`WorkspaceContext` owns an authenticated loopback HTTP API and a private
runtime descriptor under
`<global-home>/runtime/<sha256(canonical-project-root)>/agent-registry.json`.
The directory maps back to the canonical project root in the descriptor but is
never placed in the project tree. On Unix the parent is mode 0700 and
descriptor mode 0600, with unsafe permissions rejected. On Windows the runtime
directory and descriptor receive a protected DACL containing only the current
process user's SID, including when `PTRACK_HOME` is outside the user profile.
Publishing replaces a stale/same-project descriptor atomically, and shutdown
removes it only when its generation and token still identify the owning
server. The descriptor contains only the canonical project root, loopback URL,
project generation, and context registration token.
Registration requires that token and returns a stable run ID plus per-run lease
token. Heartbeat and explicit-exit requests require the per-run token. The
listener uses an OS-assigned `127.0.0.1` port, bounded JSON bodies, deadlines,
and no browser CORS access. Context close removes the descriptor and closes all
sockets/handlers.

Tokens are compared in constant time and never included in snapshots, errors,
events, or logs. The default lease is 30 seconds:

- a valid heartbeat keeps the external lease `active`;
- lease expiry marks the run `stale`;
- an external process remains `unknown` unless an authoritative integration
  supplies process state; a lease alone is not called OS-process verification;
- an explicit exit result marks it `exited`.

External runs cannot be claimed as live after lease expiry. A context-owned
lease sweeper uses the injected clock/ticker in tests and stops on context
close. Snapshot output is newest-activity-first and bounded. Registry storage
is capped at 1,024 records; inactive oldest records are evicted, while a full
set of active runs rejects new registration instead of hiding live work.

Provider is immutable metadata from an installed P-TRACK agent profile or an
explicit external registration request. PID for launched profiles comes from
the owned PTY process. Launched process state is authoritative; external
process state and lease state remain distinct. Active-resource confirmation
counts launched/running agents and external agents with an active lease.

The loopback API is deliberately explicit; it is an integration point for
wrappers/hooks, not automatic process discovery.

## Terminal Compatibility

The existing terminal manager remains project-root scoped and moves beneath
`WorkspaceContext`. It adds a bounded session snapshot and PID/activity
metadata needed for confirmation and launched-AgentRun association.

The terminal frontend mount returns an idempotent disposer. Disposal preserves
v0.14.1 behavior:

- native clipboard copy/paste;
- control-key and SIGINT behavior;
- right-click/context menu;
- multiline-paste confirmation and focus trap;
- renderer/socket generation guards;
- restart/close semantics;
- process-group/ConPTY cleanup;
- WebGL fallback.

Switch/close confirmation is workspace-level and happens before terminal
teardown. It does not replace terminal's own close behavior.

## Accessibility and Keyboard Navigation

- Initial Welcome focus goes to Open Project.
- Successful open moves focus to the project heading.
- Close returns focus to Welcome/Open Project.
- Loading uses `aria-busy`; status and errors use live regions.
- Project action menus and recent projects are keyboard reachable in DOM order.
- Confirmation dialogs trap Tab/Shift+Tab, close on Escape, and restore the
  invoking control.
- Board `R` and `/` shortcuts do not run while dialogs, project menus, terminal
  overlays, form controls, or terminal content have focus.
- No shortcut handles composing (`isComposing`) key events.

## Backward Compatibility

- `ptrack board` TUI/JSON behavior is unchanged.
- `ptrack board --gui [--plan ID]` remains supported.
- Existing Wails board and terminal binding names remain available.
- The project GUI itself uses generation-aware V2 bindings; delayed legacy
  calls cannot target a different captured context.
- Project database format remains v3; AgentRuns are process-local in this
  milestone.
- The global project registry's existing gob shape remains readable.
- No React, TanStack, additional frontend framework, CI-trigger change,
  release, tag, or publish action is introduced.

## Acceptance Criteria

- `ptrack gui`, `ptrack gui PATH`, and `ptrack board --gui` start correctly.
- Starting outside a project opens Welcome rather than exiting.
- Open, close, and switch work repeatedly without restarting the application.
- Active terminals/AgentRuns require explicit confirmation before close/switch.
- Delayed old-generation snapshots/events are ignored on both backend and
  frontend.
- Lifecycle/race tests prove bounded idempotent cleanup.
- One bounded refresh populates tracking, storage, terminal, AgentRun, and Git
  intelligence with loading/stale/error treatment.
- Git parsing is machine-readable, bounded, cancellable, timed, and read-only.
- External agents appear only after explicit registration and become
  stale/unknown when their lease expires.
- Existing terminal clipboard, key, context-menu, paste, restart, and cleanup
  tests continue to pass.

## Manual Verification Scope

This milestone must be exercised interactively on macOS for Welcome, native
directory selection, open/switch/close confirmation, focus, board edits,
terminal input/paste/restart, stale refresh display, and app shutdown.

Windows/Linux GUI behavior, Windows ConPTY, Linux PTY, IME composition,
Unicode/CJK/emoji, sleep/wake, 100 MiB terminal output, rapid switch/refresh
stress, and prolonged lease/stale stress remain explicitly outstanding unless
performed and recorded during implementation.
