# p-track Embedded Terminal Implementation Plan

> Execute this plan task by task. Use test-first implementation for the Go
> session/transport layer and pure workspace-state logic. Stop after the Stage A
> checkpoint until its acceptance criteria pass.

**Goal:** Add a performant terminal dock to the Wails GUI so project tracking
and installed shell/agent execution share one application, then extend the same
architecture to profiles, persistent tabs, and recursive splits.

**Design:** See
[`../specs/2026-07-25-embedded-terminal-design.md`](../specs/2026-07-25-embedded-terminal-design.md).
Xterm.js renders terminal panes. A Go manager owns `go-pty` sessions. An
authenticated binary loopback WebSocket carries PTY bytes; Wails bindings and
events carry lifecycle/control data.

**Initial stack:** Go 1.26, Wails v2.13, `go-pty` v0.2.3,
`gorilla/websocket`, Vite, incremental TypeScript, xterm.js 6.0.0, Vitest.
No React or TanStack dependency.

## Global constraints

- Work on a feature branch and land through a squash-merged PR.
- Do not refactor the board while adding the terminal.
- Do not retain the bbolt store while a terminal is open.
- Keep PTY commands and argument arrays separate; do not build shell strings.
- Bind the stream server only to an OS-assigned loopback port.
- Never log stream tokens, terminal input, clipboard content, environment
  values, or PTY output.
- Session maps and tokens are process-local. Persisted workspace state contains
  no backend session ID, token, URL, environment, or output by default.
- Every long-lived goroutine, listener, socket, PTY, and process has one
  documented owner and shutdown path.
- The xterm DOM renderer is the fallback after WebGL initialization/context
  failure.
- Keep the existing tag-only release workflow tag-only.

---

## Stage A — One-pane transport spike

### Task 1: Introduce the frontend build/test pipeline without changing behavior

**Files:**

- Create `frontend/package.json`
- Create `frontend/package-lock.json`
- Create `frontend/vite.config.ts`
- Create `frontend/src/app.js`
- Create `frontend/src/style.css`
- Move source HTML to `frontend/index.html`
- Modify `wails.json`
- Modify `.gitignore`
- Modify `Makefile`

**Steps:**

- [ ] Add a failing build smoke/check that expects Vite to produce
  `frontend/dist/index.html`, `app.js`, and `style.css` with stable output
  names compatible with the existing embedded asset paths.
- [ ] Add Vite and Vitest as pinned development dependencies.
- [ ] Move the current hand-maintained files out of `frontend/dist` into source
  locations without changing markup, behavior, or styles.
- [ ] Configure Vite output so `main.go` can continue embedding
  `frontend/dist`.
- [ ] Configure Wails `frontend:install` to run `npm ci`,
  `frontend:build` to run `npm run build`, and the Vite dev watcher/server for
  `wails dev`.
- [ ] Ignore generated `frontend/dist` and `frontend/wailsjs`; keep source and
  lockfile tracked.
- [ ] Add separate Make targets for frontend install, test, and build; make the
  aggregate local validation target run frontend and Go checks.
- [ ] Run `npm ci`, `npm test`, `npm run build`, `go test ./...`, and
  `make build`.
- [ ] Compare the GUI against the current board before proceeding. No terminal
  code belongs in this task.

**Checkpoint:** The board is behaviorally and visually unchanged, production
assets are reproducible from the lockfile, and a Wails application build embeds
them.

### Task 2: Define profiles and a testable PTY adapter

**Files:**

- Create `internal/terminal/profile.go`
- Create `internal/terminal/profile_test.go`
- Create `internal/terminal/pty.go`
- Create `internal/terminal/gopty.go`
- Create `internal/terminal/fake_pty_test.go`
- Modify `go.mod`
- Modify `go.sum`

**Locked public concepts:**

```go
type ProfileKind string

const (
    ProfileShell ProfileKind = "shell"
    ProfileAgent ProfileKind = "agent"
)

type Profile struct {
    ID         string            `json:"id"`
    Name       string            `json:"name"`
    Kind       ProfileKind       `json:"kind"`
    Executable string            `json:"executable"`
    Args       []string          `json:"args"`
    Env        map[string]string `json:"env"`
}
```

The internal PTY adapter must support read, write, resize, process wait/exit,
graceful termination, forced termination, and close. Its factory accepts an
executable, argument array, environment array, CWD, rows, and columns.

**Steps:**

- [ ] Write profile validation tests: stable nonempty ID/name, known kind,
  absolute or `LookPath`-resolvable executable, no NUL values, copied argument
  and environment data, and no caller mutation.
- [ ] Add built-in profile discovery for the platform's default shell and
  installed supported agent executables. Discovery reports only tools already
  present on `PATH`.
- [ ] Define a p-track-owned PTY/process interface and fake implementation for
  deterministic tests.
- [ ] Implement the adapter with `github.com/aymanbagabas/go-pty` v0.2.3.
- [ ] Set `TERM=xterm-256color`, `COLORTERM=truecolor`, and
  `TERM_PROGRAM=p-track` unless the profile explicitly supplies a safe
  override.
- [ ] Resolve the default CWD to the p-track project root; reject missing or
  non-directory CWD values.
- [ ] Ensure profile launches call `Command(executable, args...)` directly
  rather than invoking a shell wrapper.
- [ ] Run `go test ./internal/terminal`.

### Task 3: Implement session lifecycle before networking

**Files:**

- Create `internal/terminal/session.go`
- Create `internal/terminal/session_test.go`
- Create `internal/terminal/manager.go`
- Create `internal/terminal/manager_test.go`

**Session states:** `starting`, `running`, `exited`, `closing`, `closed`,
`failed`.

**Steps:**

- [ ] Write tests for valid state transitions, one `Wait`, one close, exit code
  reporting, create failure cleanup, and idempotent manager shutdown.
- [ ] Write tests proving a session starts with the requested CWD,
  environment, rows, and columns.
- [ ] Implement `Manager` as the sole owner of the session map and PTY
  factory.
- [ ] Give each session a random opaque ID and a separate cryptographically
  random stream token.
- [ ] Buffer bounded startup output until a stream attaches; do not lose the
  shell prompt emitted before xterm is ready.
- [ ] Clamp dimensions to sensible positive limits and make repeated identical
  resize calls no-ops.
- [ ] On ordinary process exit, publish one structured exit result and close
  the PTY exactly once.
- [ ] On requested close, perform graceful termination with a short bounded
  timeout, then force termination and close handles.
- [ ] On manager shutdown, reject new sessions, close existing sessions in
  parallel, wait for owned goroutines, and return aggregated errors.
- [ ] Run the race detector:
  `go test -race ./internal/terminal`.

### Task 4: Implement the authenticated binary stream and backpressure

**Files:**

- Create `internal/terminal/protocol.go`
- Create `internal/terminal/server.go`
- Create `internal/terminal/server_test.go`
- Extend `internal/terminal/session.go`
- Extend `internal/terminal/session_test.go`

**Protocol:**

- Server-to-client binary frame: PTY output bytes.
- Client-to-server binary frame: terminal input bytes.
- Client-to-server text frame:
  `{"type":"ack","bytes":<positive integer>}`.
- Close frame: normal stream/session teardown.

**Steps:**

- [ ] Write HTTP upgrade tests for loopback binding, allowed Wails origins,
  invalid origin, missing token, wrong token, unknown/closed session, and a
  second attachment to the same session.
- [ ] Write protocol tests for binary input/output, malformed control frames,
  oversized frames, disconnect, reconnect policy, and token expiry.
- [ ] Write a high-volume test with a deliberately slow client. Assert bounded
  queued/unacknowledged bytes and that PTY reading resumes after ACK.
- [ ] Start one `net.Listener` on `127.0.0.1:0` and one owned `http.Server` per
  manager, not per session.
- [ ] Use binary frames end to end. Do not JSON/base64 encode PTY output.
- [ ] Cap output chunks at 64–100 KiB and pause PTY reads after at most
  512 KiB is unacknowledged.
- [ ] Count ACK bytes only after xterm's write callback completes on the
  frontend. Reject ACK totals beyond bytes sent.
- [ ] Add read limits, deadlines, ping/pong handling, and single-writer
  serialization required by `gorilla/websocket`.
- [ ] Ensure server shutdown closes listener, active WebSockets, and handler
  goroutines without `log.Fatal`, panic, fixed-port scanning, or token logging.
- [ ] Run `go test -race ./internal/terminal`.

### Task 5: Expose terminal control through Wails lifecycle and bindings

**Files:**

- Create `internal/gui/terminal.go`
- Create `internal/gui/terminal_test.go`
- Modify `internal/gui/app.go`
- Modify `internal/gui/run.go`

**Bindings:**

```go
func (a *App) GetTerminalProfiles() ([]terminal.Profile, error)
func (a *App) CreateTerminal(profileID, cwd string, rows, columns int) (TerminalSession, error)
func (a *App) ResizeTerminal(sessionID string, rows, columns int) error
func (a *App) CloseTerminal(sessionID string, force bool) error
```

`TerminalSession` returns opaque session metadata and its one-session stream
URL. It never returns environment data.

**Steps:**

- [ ] Add binding tests using a fake terminal manager: profile selection,
  project-root default CWD, invalid profile/session, resize ordering, close,
  and manager failure propagation.
- [ ] Retain the Wails context from `OnStartup`.
- [ ] Construct the manager with the canonical project root, not the process
  CWD.
- [ ] Emit low-frequency `terminal:status` and `terminal:exit` Wails events.
  Do not emit PTY bytes as Wails events.
- [ ] Register `OnShutdown` to close the manager and stream server.
- [ ] Keep terminal lifecycle independent of `App.open()` and bbolt.
- [ ] Run `go test -race ./internal/gui ./internal/terminal`.

### Task 6: Render one terminal dock and connect it to the stream

**Files:**

- Modify `frontend/package.json`
- Modify `frontend/package-lock.json`
- Modify `frontend/index.html`
- Create `frontend/src/terminal/client.ts`
- Create `frontend/src/terminal/pane.ts`
- Create `frontend/src/terminal/paste.ts`
- Create `frontend/src/terminal/pane.test.ts`
- Modify `frontend/src/app.js`
- Modify `frontend/src/style.css`

**Steps:**

- [ ] Add pinned xterm 6, Fit, Search, Web Links, and WebGL packages.
- [ ] Unit-test stream state and teardown independently of the DOM:
  connection/open/close/error, output queue, ACK after write callback, and no
  sends after close.
- [ ] Add a bottom dock inside the main workspace with closed, opening,
  running, exited, and failed UI states.
- [ ] Let the user select a discovered profile and explicitly open/restart/close
  it. Do not auto-run a process on board launch in this stage.
- [ ] Create one xterm instance, load Fit/Search/Web Links, attempt WebGL after
  `open`, and retain DOM rendering if WebGL fails or loses context.
- [ ] Feed WebSocket binary output into `terminal.write(Uint8Array, callback)`;
  send the byte ACK from that callback.
- [ ] Feed xterm `onData`/`onBinary` as binary input frames without a Wails call
  per keystroke.
- [ ] Use `ResizeObserver` and `requestAnimationFrame` to fit the pane. Debounce
  Wails PTY resize calls to at most one per 100 ms and send a trailing final
  size.
- [ ] Make dock height draggable with accessible keyboard alternatives and
  reasonable min/max sizes.
- [ ] Stop the document-level `R` and `/` board shortcuts while xterm or a
  terminal overlay has focus.
- [ ] Open detected links only through Wails `BrowserOpenURL`, requiring
  Cmd+click on macOS or Ctrl+click elsewhere.
- [ ] Dispose the socket, listeners, observers, addons, and xterm exactly once
  when the pane closes.
- [ ] Run `npm test`, `npm run build`, `go test -race ./...`, `go vet ./...`,
  and `make build`.

### Stage A manual acceptance checkpoint

Do not begin tabs/splits until all items pass:

- [ ] Run the default login shell at the project root.
- [ ] Run at least one installed agent CLI profile.
- [ ] Verify interactive prompts, Ctrl+C, Ctrl+D, arrows, function keys, mouse,
  Unicode/emoji/CJK, IME, and terminal title.
- [ ] Verify a curses application such as `vim` or `less` and an application
  using the alternate screen.
- [ ] Generate at least 100 MiB of output while monitoring memory and UI
  responsiveness. Confirm output backpressure is bounded.
- [ ] Scroll away from the bottom during continued output; confirm the viewport
  does not jump back unexpectedly.
- [ ] Resize the window and drag the dock rapidly; confirm final rows/columns
  and no visible blank/flicker loop.
- [ ] Background/foreground and sleep/wake the app; confirm WebGL recovery or
  DOM fallback.
- [ ] Close/restart a terminal repeatedly and close the application with a
  running process. Confirm no descendant process or listening socket remains.
- [ ] Smoke-test macOS, Windows ConPTY, and Linux PTY before claiming
  cross-platform support. Record platform-specific failures as scoped follow-up
  tasks; do not conceal them with OS checks.

---

## Stage B — Production terminal usability

### Task 7: Add safe clipboard and multiline paste

**Files:**

- Extend `frontend/src/terminal/paste.ts`
- Create `frontend/src/terminal/paste.test.ts`
- Modify `frontend/src/terminal/pane.ts`
- Modify `frontend/index.html`
- Modify `frontend/src/style.css`

**Steps:**

- [ ] Test newline normalization on Windows and Unix, blank input, one line,
  multiple lines, trailing newline, large preview truncation, alternate-screen
  bypass, cancel, confirm, and bracketed-paste mode.
- [ ] Intercept Cmd/Ctrl+V and Shift+Insert before the webview/xterm default
  paste path.
- [ ] Read ordinary paste text through Wails' native clipboard API.
- [ ] Show a modal with a bounded, escaped preview for multiline content when
  not in the alternate screen.
- [ ] On confirmation, use xterm's paste API/public mode behavior so bracketed
  paste is correct. Never manually execute lines.
- [ ] Implement Ctrl+C as copy when a selection exists and SIGINT input
  otherwise.
- [ ] Add copy, paste, and select-all actions to a compact terminal context
  menu with platform-correct shortcuts.

### Task 8: Finish search, activity, notifications, and close confirmation

**Files:**

- Create `frontend/src/terminal/search.ts`
- Create `frontend/src/terminal/search.test.ts`
- Create `frontend/src/terminal/notifications.ts`
- Modify `frontend/src/terminal/pane.ts`
- Modify `internal/gui/terminal.go`
- Modify `frontend/index.html`
- Modify `frontend/src/style.css`

**Steps:**

- [ ] Add a per-pane search overlay with incremental next/previous, regex,
  case-sensitive, whole-word, result index/count, and Escape-to-close.
- [ ] Mark a hidden dock/pane as active after output; clear the marker when
  focused.
- [ ] Offer one-shot “notify on activity.”
- [ ] Offer reliable “notify when done” for direct agent profiles using the
  Go process-exit event.
- [ ] Label shell profiles as notifying only when the terminal session exits.
  Do not infer inner shell-command completion.
- [ ] Use native/system notification support and focus the terminal when a
  notification is clicked.
- [ ] Confirm before closing any running session; offer cancel and terminate.
- [ ] Add exit code, elapsed time, restart, and copy-last-selection affordances
  to the exited state.

### Task 9: Add configurable profiles without storing secrets

**Files:**

- Create `internal/terminal/config.go`
- Create `internal/terminal/config_test.go`
- Modify `internal/store/global.go` only if typed profile persistence cannot use
  its existing config interface safely
- Modify `internal/gui/terminal.go`
- Create `frontend/src/terminal/profiles.ts`
- Create `frontend/src/terminal/profiles.test.ts`

**Steps:**

- [ ] Define a versioned profile configuration containing executable, argument
  array, CWD policy, non-secret environment overrides, name, kind, and exit
  behavior.
- [ ] Merge built-in discovery and custom profiles by stable ID without
  mutating either source.
- [ ] Validate custom executables and CWD before saving and again before launch.
- [ ] Store only explicit non-secret overrides. Never snapshot the inherited
  process environment.
- [ ] Add create/edit/duplicate/delete/default-profile UI with argument fields
  represented as an array, not one shell-parsed string.
- [ ] Provide shell and installed-agent starter profiles; do not download or
  authenticate tools.

---

## Stage C — Persistent tabs and recursive splits

### Task 10: Implement the pure workspace tree and reducer

**Files:**

- Create `frontend/src/workspace/model.ts`
- Create `frontend/src/workspace/reducer.ts`
- Create `frontend/src/workspace/reducer.test.ts`
- Create `frontend/src/workspace/persistence.ts`
- Create `frontend/src/workspace/persistence.test.ts`

**Required operations:**

- add/close/select/reorder/rename/pin tab;
- split left/right/top/bottom;
- close/move/select pane;
- resize/equalize/maximize pane;
- focus nearest pane by direction;
- normalize and validate;
- serialize/restore/migrate.

**Steps:**

- [ ] Lock a version-1 JSON schema matching the design document.
- [ ] Write table-driven reducer tests before DOM integration.
- [ ] When splitting against a compatible parent, insert and proportionally
  rescale siblings.
- [ ] When orientation differs, wrap the target in a new split node.
- [ ] Normalize after mutation: remove empty splits, replace one-child splits,
  flatten same-orientation nesting, clamp ratios, and renormalize sum to one.
- [ ] Keep pane IDs stable and unique and ensure each descriptor is referenced
  by exactly one tree leaf.
- [ ] Test corrupted/unknown-version state, duplicate IDs, NaN/negative ratios,
  missing profile/CWD, and recovery fallback.
- [ ] Persist project-keyed dock/tab/layout/profile/CWD state after mutations,
  debounced by one second plus a periodic safety save.
- [ ] Exclude runtime session IDs, tokens, URLs, environment, input, and output.

### Task 11: Render tabs and split panes without losing xterm instances

**Files:**

- Create `frontend/src/workspace/controller.ts`
- Create `frontend/src/workspace/split-view.ts`
- Create `frontend/src/workspace/tab-bar.ts`
- Create corresponding `*.test.ts` files
- Modify `frontend/src/terminal/pane.ts`
- Modify `frontend/src/app.js`
- Modify `frontend/index.html`
- Modify `frontend/src/style.css`

**Steps:**

- [ ] Make the workspace controller own pane descriptors and the terminal pane
  registry. The split renderer must move/reuse pane hosts rather than recreate
  xterm on every layout mutation.
- [ ] Render accessible top-level tabs and recursive flex/grid split nodes with
  draggable separators.
- [ ] Fit only affected visible panes during separator drag and send coalesced
  PTY resize.
- [ ] Add split-direction, close, equalize, maximize/restore, and directional
  focus commands with platform-correct shortcuts.
- [ ] Add drag/drop only after keyboard/button operations are complete and
  tested.
- [ ] On tab hide, preserve session and xterm; on reveal, refit/redraw and
  recover/fallback the renderer.
- [ ] Bound simultaneous WebGL contexts. Prefer WebGL for visible panes and DOM
  for panes that cannot obtain/recover a context.
- [ ] Aggregate pane activity/exit state onto the top-level tab.
- [ ] Confirm every running pane before closing a tab containing multiple
  sessions.

### Task 12: Restore workspace descriptors as fresh sessions

**Files:**

- Extend `frontend/src/workspace/controller.ts`
- Extend `frontend/src/workspace/persistence.ts`
- Extend tests for both modules

**Steps:**

- [ ] Restore dock geometry, top-level tabs, split ratios, selected pane,
  profile IDs, and last reported valid CWD.
- [ ] Render restored panes as stopped placeholders; do not silently execute
  processes during raw state parsing.
- [ ] After the workspace is visible, either require explicit “restore
  sessions” or honor a separately named user preference for automatic relaunch.
- [ ] Clearly label relaunched processes as fresh sessions.
- [ ] If a profile disappeared or CWD no longer exists, use the default profile
  and project root and surface a non-blocking warning.
- [ ] Add an explicit “reset terminal workspace” action that removes only the
  current project's terminal layout key.
- [ ] Do not persist scrollback in version 1. Evaluate opt-in serialized
  scrollback separately with privacy limits and a migration.

---

## Stage D — Tracking/model integration

### Task 13: Design and add plan/task association

This task starts with a short interaction spec and does not automatically
construct agent prompts.

**Candidate files after design approval:**

- Modify `internal/gui/app.go`
- Modify `internal/gui/terminal.go`
- Modify `frontend/src/app.js`
- Modify `frontend/src/workspace/model.ts`
- Modify `frontend/src/workspace/controller.ts`

**Steps:**

- [ ] Decide whether association belongs to a pane, top-level tab, or both.
- [ ] Add “Open terminal” to a card and plan context. It selects an installed
  profile and opens at the project root with visible task/plan association.
- [ ] Surface associated task status and durable memory beside the terminal
  without writing terminal output into bbolt.
- [ ] Let the user record a selected terminal excerpt as a note only through an
  explicit review/edit confirmation.
- [ ] Keep task status changes explicit. Process start/exit must not
  automatically mark a task doing/done without a separately approved rule.
- [ ] Design agent prompt templates, concurrency limits, and automatic task
  transitions as a distinct future orchestration feature.

---

## Final validation and documentation

### Task 14: Cross-platform verification and user documentation

**Files:**

- Modify `README.md`
- Add terminal screenshots under `docs/assets/`
- Modify `CHANGELOG.md` only when preparing the feature PR/release entry
- Modify the existing `.github/workflows/release.yml` only if the locked
  frontend build cannot run reproducibly on its existing runners; do not change
  its tag-only triggers

**Steps:**

- [ ] Run formatting: `gofmt` on changed Go files and the chosen frontend
  formatter/check.
- [ ] Run `npm ci`, frontend unit tests, frontend production build,
  `go test -race ./...`, `go vet ./...`, and `make build`.
- [ ] Run focused security checks for loopback exposure, origin/token bypass,
  oversized frames, malformed ACKs, URL handling, profile argument injection,
  clipboard preview escaping, and accidental sensitive logging.
- [ ] Perform the Stage A manual matrix plus tabs/splits/restoration on macOS,
  Windows, and Linux.
- [ ] Document profile creation, paste behavior, shortcuts, restoration
  semantics, and the fact that full app restart launches fresh processes.
- [ ] Document troubleshooting for WebGL fallback, unavailable shell/agent
  executables, ConPTY requirements, and stale CWD.
- [ ] Verify the CLI and Bubble Tea TUI remain unchanged.
- [ ] Review the final diff for generated assets, unrelated refactors,
  dependencies outside the approved stack, and any AI attribution.

## Deferred follow-ups

- OSC 133/633 shell integration for command boundaries, exit status, duration,
  CWD, and reliable inner-shell command completion.
- Tmux-compatible profiles or a detached helper for process persistence.
- Opt-in encrypted/bounded scrollback persistence after privacy design.
- SSH and remote agent execution.
- Image/Sixel support after output-memory and webview security testing.
- TanStack Query only if board/session snapshot caching develops concrete
  invalidation problems.
