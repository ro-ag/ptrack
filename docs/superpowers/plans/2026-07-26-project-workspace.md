# P-TRACK Project Workspace Implementation Plan

**Date:** 2026-07-26
**Design:** `../specs/2026-07-26-project-workspace-design.md`

> Execute test-first. Project lifecycle, Git snapshotting, and AgentRun
> tracking are independent tasks and must pass their focused validation before
> GUI integration.

## Global Constraints

- Work only on `feat/project-workspace`.
- Preserve CLI/TUI and v0.14.1 terminal behavior.
- Keep project bbolt handles transient.
- Retain vanilla Vite and incremental TypeScript; no React or TanStack.
- Use no shell command construction for Git or terminal launches.
- Bound every context cleanup, Git command/output, snapshot list, and async
  wait.
- Do not land, release, tag, publish, push, or change CI triggers.
- Run frontend tests/build after every frontend task.
- Run Go race tests and vet after every backend task.

## Stage 1 — Lock CLI and Discovery Compatibility

### Task 1: Canonical GUI command

1. Add failing CLI tests for:
   - `ptrack gui` calling the GUI callback with an empty/default path;
   - `ptrack gui PATH`;
   - rejecting more than one path;
   - `ptrack board --gui --plan ID` forwarding its legacy plan selection;
   - GUI-unavailable errors.
2. Change the injected callback to accept path and initial plan.
3. Add `gui [PATH]` and retain `board --gui`.
4. Change `main.go` and GUI `Run` to accept an optional starting path.
5. Run `go test -race ./internal/cli ./internal/gui` and `go vet ./...`.

### Task 2: Worktree-safe project resolution and bounded recents

1. Add failing store tests for `.git` file boundaries, nested selected
   directories, stale registry entries, and bounded recent results.
2. Recognize `.git` files/directories as repository boundaries.
3. Add a bounded recent-project helper that labels path availability without
   changing the persisted gob shape.
4. Run `go test -race ./internal/store` and `go vet ./...`.

## Stage 2 — Project Lifecycle (Independent Backend Task)

### Task 3: Generation-scoped `WorkspaceContext`

1. Add fake-resource tests before implementation for:
   - operation admission/rejection;
   - cancel-before-wait ordering;
   - active-resource summary;
   - normal close;
   - idempotent concurrent close;
   - one caller timing out followed by eventual cleanup and a successful later
     Close observation;
   - internally bounded blocked cleanup steps;
   - joined errors;
   - no goroutine left after successful close.
2. Implement `WorkspaceContext` with explicit context, operation gate,
   terminal manager, AgentRun registry, monitor ownership, and bounded Close.
3. Keep store opening transient behind context operations.
4. Run `go test -race ./internal/gui` and `go vet ./...`.

### Task 4: Durable app coordinator

1. Add failing lifecycle/race tests for:
   - Welcome startup with no project;
   - candidate construction failure preserving the old project;
   - unpublished candidate cleanup on every failure;
   - transition IDs not advancing published generations;
   - open/close/switch generations;
   - confirmation fencing, cancel/release, stale resource-revision token, and
     terminal/AgentRun admission races;
   - confirmation expiry and deterministic supersession after renderer reload
     or an abandoned response;
   - stale event suppression;
   - stale binding/response rejection;
   - repeated concurrent transitions;
   - application shutdown reusing context cleanup.
2. Refactor `App` into a durable coordinator with a workspace factory seam.
3. Serialize transitions and generation publication.
4. Add `GetWorkspaceState`, `PickProjectDirectory`, `OpenProject`, and
   `CloseProject`/`CancelWorkspaceChange` bindings with structured fenced
   confirmation results.
5. Start Wails even when initial project discovery returns `ErrNoProject`.
6. Run `go test -race ./internal/gui ./internal/terminal` and `go vet ./...`.

## Stage 3 — Read-Only Git Snapshot (Independent Backend Task)

### Task 5: Bounded command runner and porcelain parser

1. Create `internal/gitinfo` tests first for:
   - context cancellation and timeout;
   - stdout/stderr output limits;
   - no shell invocation;
   - `--no-optional-locks`, optional-lock environment, no pager, and no prompt;
   - porcelain v2 branch, detached, staged, unstaged, untracked, conflicted,
     ignored, upstream, and ahead/behind records;
   - malformed/truncated records.
2. Implement the runner interface and production `exec.CommandContext`
   adapter with fixed locale, limited writers, and typed errors.
3. Implement byte/NUL-oriented porcelain v2 parsing.
4. Run `go test -race ./internal/gitinfo` and `go vet ./...`.

### Task 6: Refs, remotes, commits, divergence, and stale branches

1. Add table-driven fake-runner tests for:
   - non-repository/detached/worktree states;
   - fetch/push remote URLs;
   - local/remote ref fields and worktree paths;
   - recent commit fields/refs/changed-area summaries;
   - upstream divergence and bounded unpushed commits;
   - 90-day age-based stale branches;
   - per-command and aggregate command/output bounds and partial section
     errors.
2. Implement `gitinfo.Snapshot`.
3. Add a Windows cross-compile test/build seam with no Unix-only code.
4. Run `go test -race ./internal/gitinfo`, `go vet ./...`, and cross-compile
   the package for Windows.

## Stage 4 — AgentRun Registry (Independent Backend Task)

### Task 7: Registry and lease semantics

1. Create `internal/agentrun` tests first for:
   - stable opaque IDs and unique lease tokens;
   - immutable registration data;
   - launched and explicit-external registration;
   - authenticated heartbeats;
   - heartbeat activity ordering;
   - lease expiry to stale/unknown;
   - explicit exit result;
   - bounded snapshots;
   - idempotent bounded shutdown;
   - deterministic injected clock/ticker behavior.
2. Implement the in-memory registry without process-name/title scanning.
3. Add failing integration tests and implementation for the context-owned
   authenticated loopback register/heartbeat/exit API, bounded bodies and
   handlers, user-private global/OS runtime descriptor keyed by canonical
   project root, platform-specific permission checks, expiry, idempotent socket
   shutdown, and descriptor removal.
4. Keep authoritative process state separate from lease state and define
   launched/profile or explicit-registration provider metadata.
5. Ensure registration/lease tokens never appear in snapshot DTOs, events,
   errors, or logs.
6. Run `go test -race ./internal/agentrun` and `go vet ./...`.

### Task 8: Terminal association and visibility

1. Add terminal fake/manager/session tests for PID, profile kind, start/last
   activity, process state, and bounded session snapshots.
2. Extend the PTY adapter with owned-process metadata on all platforms.
3. Register agent-profile terminals as launched AgentRuns and update them from
   explicit terminal lifecycle signals.
4. Expose integration status in Wails, but use the authenticated loopback API
   for external register/heartbeat/exit calls.
5. Include generation in terminal events and suppress old-generation events.
6. Run `go test -race ./internal/agentrun ./internal/terminal ./internal/gui`
   and `go vet ./...`.
7. Cross-compile relevant terminal and AgentRun packages for Windows.

## Stage 5 — One Bounded Project Snapshot

### Task 9: Bounded tracking/store reads

1. Add store/report tests first for bounded plans, selected-plan tasks,
   blockers, issues, notes, and activity with correct total/more counts.
2. Implement bounded bbolt cursor helpers and a single-pass inventory.
3. Allow projects with no active plan to return an empty board snapshot.
4. Preserve legacy `GetBoard` behavior/binding.
5. Run `go test -race ./internal/store ./internal/report ./internal/gui` and
   `go vet ./...`.

### Task 10: Composite workspace snapshot

1. Add fake-section tests for an immutable, generation-tagged composite
   snapshot, complete deadline, partial Git error, cancellation, storage
   status, terminal/AgentRun lists, and stale-generation rejection.
2. Implement `GetWorkspaceSnapshot(planID, generation)` with an eight-second
   top-level timeout and section errors.
3. Avoid retaining any project store after snapshot assembly.
4. Run `go test -race ./internal/gui ./internal/gitinfo ./internal/agentrun`
   and `go vet ./...`.

## Stage 6 — Workspace Frontend and Accessibility

### Task 11: Pure workspace controller

1. Add Vitest tests first for Welcome/loading/open/error/closed transitions,
   request generations, stale-response suppression, interval disposal,
   overlapping refreshes, every mutation/terminal await crossing a switch, and
   section stale/error retention.
2. Implement a small TypeScript controller with an injected backend/timers.
3. Implement a generation-aware facade for snapshot, all board mutations,
   profile initialization, terminal create/resize/close, and AgentRun calls;
   retain legacy backend wrappers only for compatibility.
4. Make terminal mount return an idempotent disposer; add teardown tests for
   socket, renderer, event, overlay, timer, and pending async invalidation.
5. Run `npm test` and `npm run build`.

### Task 12: Welcome and project lifecycle UX

1. Add pure test-first controller/focus-policy coverage with injected
   elements/runtime for picker cancel/error, recents, confirmation Tab/
   Shift+Tab/Escape/restore, shortcut isolation, and composing keys. Do not add
   a DOM-test dependency.
2. Add Welcome, loading/error/closed panels, recent projects, and project
   action controls to the existing DOM.
3. Wire Open/Switch to the native directory picker and recent project
   selection to the same transition.
4. Add active-resource confirmation with focus trap, Escape, focus restore,
   and generation-safe completion.
5. Move focus appropriately on Welcome/open/close/error transitions.
6. Isolate board and terminal shortcuts from all project dialogs/menus and
   composing key events.
7. Run `npm test` and `npm run build`.

### Task 13: Overview, Git, terminal, and AgentRun rendering

1. Add pure formatting/render-state tests before DOM wiring.
2. Refresh the project screen through one workspace-snapshot request.
3. Render bounded tracking overview, repository/storage status, terminal and
   AgentRun visibility, Git status/remotes/branches/commits/divergence/stale
   branches, and total/more indicators.
4. Render explicit first-load loading/error and retained stale section states.
5. Preserve existing board editing and terminal interaction.
6. Run `npm test` and `npm run build`.

## Stage 7 — Verification and Review

### Task 14: Automated lifecycle/stress validation

1. Add/execute a switch stress test with active refreshes, terminal sessions,
   blocked cleanup, old events, and repeated close.
2. Run formatting for changed Go/TypeScript-compatible files.
3. Run:
   - `cd frontend && npm ci && npm test && npm run build`;
   - `go test -race ./...`;
   - `go vet ./...`;
   - `make test`;
   - `make build`.
4. Cross-compile without execution using explicit disposable artifacts:
   - create `ptrack_cross_dir=$(mktemp -d)` and install an EXIT trap that
     removes that exact directory;
   - `GOOS=windows GOARCH=amd64 CGO_ENABLED=0 go test -c -o
     "$ptrack_cross_dir/gitinfo.test.exe" ./internal/gitinfo`;
   - `GOOS=windows GOARCH=amd64 CGO_ENABLED=0 go test -c -o
     "$ptrack_cross_dir/terminal.test.exe" ./internal/terminal`;
   - `GOOS=windows GOARCH=amd64 CGO_ENABLED=0 go test -c -o
     "$ptrack_cross_dir/agentrun.test.exe" ./internal/agentrun`;
   - `GOOS=windows GOARCH=amd64 CGO_ENABLED=0 go test -c -tags bindings -o
     "$ptrack_cross_dir/gui.test.exe" ./internal/gui`.
5. Verify the CLI and Bubble Tea TUI tests remain unchanged/passing.

### Task 15: Interactive macOS verification

Exercise:

- launch in and outside a P-TRACK project;
- native Open/Switch selection and cancel;
- recent-project keyboard navigation;
- close/switch with running terminal and AgentRun confirmation;
- repeated switches during refresh;
- focus restoration and shortcut isolation;
- board add/move/edit/memory;
- terminal typing, Ctrl+C, clipboard, right-click, multiline paste,
  restart/close;
- Git detached/upstream/dirty rendering where practical;
- application shutdown with a live terminal and absence of descendants/socket.

Record observed results. Do not claim Windows/Linux, IME/Unicode, sleep/wake, or
stress checks that were not performed.

### Task 16: Independent final diff review

1. Have a read-only subagent review the complete branch diff for correctness,
   lifecycle races, stale-generation escape paths, boundedness, read-only Git
   behavior, lease claims, accessibility, compatibility, unrelated changes,
   dependencies, CI triggers, and attribution.
2. Reproduce and fix every validated finding.
3. Re-run affected focused validation and the complete suite.
4. Report completed checks and outstanding manual Windows/Linux, IME/Unicode,
   stress, and platform checks.

Validated final-review corrections are separate regression tasks:

- same-root descriptor replacement/removal ownership and Windows protected
  DACL enforcement;
- context-ignoring resource shutdown and application wait-group deadlines;
- stale terminal/mutation continuations, inert transition state, state focus,
  coalesced refreshes, and stale Git-section retention;
- capped AgentRun storage, bounded-memory recents, namespace-specific Git ref
  limits, repository-error classification, complete selected-task association
  counts, and a complete-snapshot deadline.
