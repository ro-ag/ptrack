# P-TRACK Embedded Terminal Design

**Status:** Proposed
**Date:** 2026-07-25
**Owner:** ro-ag

## Goal

Put project tracking and model execution in one P-TRACK desktop window without
embedding another terminal application or turning the Wails frontend into a
second backend.

The first delivery is one performant terminal dock that can run an installed
shell or agent CLI in the project root. Tabs, recursive splits, profiles, and
workspace restoration build on the same session model after the transport spike
passes.

## Current P-TRACK Constraints

- The desktop app uses Wails v2.13 and a Go backend.
- `frontend/dist` is currently hand-maintained vanilla HTML, CSS, and JavaScript.
  There is no package manager, module bundler, or frontend test runner.
- GUI calls are ordinary Wails bindings on `gui.App`; the app does not currently
  retain the Wails startup context or own long-lived resources.
- The board opens the bbolt store only for an operation. A terminal must not
  change that property or retain the project database lock.
- Application binaries embed `frontend/dist`, and production builds run through
  Wails.

The terminal work therefore includes a small frontend build pipeline. It does
not require React, TanStack Start, or a web server-side application framework.

## Tabby Architecture Analysis

This analysis is based on Tabby commit
[`14e2d60`](https://github.com/Eugeny/tabby/tree/14e2d60b9b6dee84a53c37f05eefeb803787de04).
The useful part is its separation of concerns and mature terminal behavior, not
its Angular plugin framework.

### Package boundaries

Tabby divides terminal functionality across several packages:

1. `tabby-terminal` owns the renderer abstraction, xterm.js integration,
   terminal behavior, search, paste handling, OSC middleware, and saved
   scrollback.
2. `tabby-local` owns local shell profiles and a transport-neutral local
   `Session` implemented against a `PTYInterface`.
3. `tabby-electron` implements the renderer-side PTY proxy over Electron IPC.
4. The Electron main process owns the real `node-pty` instances in a
   `PTYManager`.
5. `tabby-core` owns top-level tabs, recursive split layout, activity state,
   recovery tokens, profile dispatch, commands, and notifications.

The dependency concern is real. The current
[`tabby-terminal/package.json`](https://github.com/Eugeny/tabby/blob/14e2d60b9b6dee84a53c37f05eefeb803787de04/tabby-terminal/package.json)
has Angular 15, ng-bootstrap, RxJS, `tabby-core`, and `tabby-settings` peer
dependencies. `tabby-local` and `tabby-electron` add more Tabby and Electron
coupling. Importing these packages would bring an application framework into
P-TRACK, not just a terminal widget.

### Renderer abstraction

[`Frontend`](https://github.com/Eugeny/tabby/blob/14e2d60b9b6dee84a53c37f05eefeb803787de04/tabby-terminal/src/frontends/frontend.ts)
is a renderer-neutral contract. It exposes input, resize, title, bell, alternate
screen, selection, search, state serialization, and terminal-mode information.
[`XTermFrontend`](https://github.com/Eugeny/tabby/blob/14e2d60b9b6dee84a53c37f05eefeb803787de04/tabby-terminal/src/frontends/xtermFrontend.ts)
implements it using xterm.js and addons.

Tabby currently loads:

- Fit
- Search
- Serialize
- Unicode 11
- optional ligatures
- optional image/Sixel support
- WebGL or Canvas rendering

It also reaches into private xterm internals for scroll pinning, immediate
repaint, keyboard behavior, and renderer recovery. P-TRACK should adopt the
observable behavior but stay on public xterm APIs wherever possible. Tabby is
on xterm 5.x; P-TRACK should use xterm 6.x, where DOM is the supported fallback
when WebGL is unavailable.

### Session and PTY separation

[`BaseSession`](https://github.com/Eugeny/tabby/blob/14e2d60b9b6dee84a53c37f05eefeb803787de04/tabby-terminal/src/session.ts)
connects a frontend to a session through input/output streams and middleware.
It buffers initial PTY output until the frontend has attached and established
its first dimensions.

The local
[`Session`](https://github.com/Eugeny/tabby/blob/14e2d60b9b6dee84a53c37f05eefeb803787de04/tabby-local/src/session.ts)
does not know Electron details. It asks `PTYInterface` to spawn or restore a
PTY, subscribes to data/exit/close, forwards input and resize, manages
environment/CWD, and performs graceful shutdown.

[`ElectronPTYProxy`](https://github.com/Eugeny/tabby/blob/14e2d60b9b6dee84a53c37f05eefeb803787de04/tabby-electron/src/pty.ts)
translates that interface into Electron IPC. The real
[`PTYManager`](https://github.com/Eugeny/tabby/blob/14e2d60b9b6dee84a53c37f05eefeb803787de04/app/lib/pty.ts)
lives in the main process and owns `node-pty`.

P-TRACK should preserve these boundaries:

```
xterm pane -> terminal client -> byte transport -> Go session -> go-pty -> process
```

Wails replaces Electron IPC, and `go-pty` replaces `node-pty`.

### Flow control and UTF-8

Tabby has two independent forms of backpressure:

- The main-process `PTYDataQueue` emits at most 100 KiB per chunk, pauses the
  PTY after roughly 500 KiB is unacknowledged, and resumes after renderer ACKs.
- `XTermFrontend` tracks queued xterm write callbacks. After enough 128 KiB
  batches are pending, it waits for the callback queue to drain before writing
  more.

The PTY queue also avoids splitting incomplete UTF-8 sequences when converting
bytes for the renderer.

This is a critical production behavior. A terminal that works at a prompt can
still consume unbounded memory or freeze during a large build log. P-TRACK's
stream protocol needs bounded chunks and acknowledgements tied to xterm write
completion.

P-TRACK can send `Uint8Array` directly to xterm, allowing xterm's streaming
decoder to handle byte boundaries. It should not convert arbitrary PTY chunks
to separate JavaScript strings.

### Resize behavior

Tabby uses a `ResizeObserver`, coalesces frontend resize work to about one fit
per 32 ms, and audits PTY resize events at 100 ms. It preserves viewport
position when the user has scrolled away from the bottom.

P-TRACK should similarly separate:

- fitting/redrawing xterm in response to layout changes;
- sending coalesced rows/columns to the Go PTY;
- preserving user scroll position during output and window resize.

Polling dimensions every few seconds, as WailsTerm currently does, is not
adequate for a dock or recursive split layout.

### WebGL lifecycle

Tabby detects known-incompatible renderers, handles WebGL context loss, retries
context creation only while the pane is visible and focused, and eventually
falls back.

P-TRACK should start with WebGL when available and fall back to xterm's DOM
renderer. It should dispose xterm/addon instances when panes close and call
fit/redraw when a hidden tab becomes visible. Multiple tabs and panes make GPU
context limits a real concern.

### Paste protection and clipboard

Tabby implements multiline-paste protection outside xterm:

1. Read through the platform clipboard service.
2. Normalize line endings.
3. Apply configured trimming/newline behavior.
4. Warn when pasted text contains multiple lines, unless an alternate-screen
   application is active.
5. Use bracketed-paste markers when the terminal mode requests them.

P-TRACK needs the same host-level interception so the warning cannot be bypassed
by the ordinary keyboard paste path. Wails' native clipboard API is preferable
to relying solely on browser clipboard permissions inside a webview.

### Search

Tabby's search panel wraps xterm's Search addon. It offers next/previous,
incremental search, regex, case-sensitive, and whole-word options, and reports
the active result/count. The options are persisted.

P-TRACK should implement the same compact overlay per pane. This behavior can be
implemented directly with `@xterm/addon-search`; no Tabby code is required.

### Activity and completion notifications

Terminal output marks a background Tabby tab as active. “Notify on activity”
waits for that marker and sends a one-shot browser notification.

“Notify when done” is more limited than it appears. Tabby's
[`CompletionObserver`](https://github.com/Eugeny/tabby/blob/14e2d60b9b6dee84a53c37f05eefeb803787de04/tabby-core/src/services/app.service.ts)
polls `getCurrentProcess()` once per second until no child process is found.
The local terminal implementation obtains the child process tree through
platform-specific native modules. It is not based on OSC 133/633 shell command
markers.

For P-TRACK:

- Direct agent profiles can notify reliably when their PTY process exits.
- Any output can mark a hidden pane/tab as active.
- Reliable completion of a command run inside a persistent shell is deferred
  until shell integration exists.
- The first implementation must not scrape prompts or claim shell-command
  completion based on output silence.

### Profiles

Tabby discovers built-in shells through platform `ShellProvider`s. A profile
provider maps a profile to new-tab parameters. Profiles carry command, argument
array, environment overrides, CWD, icon/color, terminal color scheme, and
behavior on session end. Custom profile defaults and group defaults are layered
without mutating the source profile.

P-TRACK needs a smaller immutable profile descriptor:

```text
id, name, executable, args[], cwd policy, env overrides, kind, exit behavior
```

Commands and arguments stay separate; profiles must not concatenate
user-controlled strings into a shell command. P-TRACK inherits the user's
environment and never installs an agent CLI.

### Recursive splits

Tabby models each top-level workspace tab as a recursive split tree.
[`SplitContainer`](https://github.com/Eugeny/tabby/blob/14e2d60b9b6dee84a53c37f05eefeb803787de04/tabby-core/src/components/splitTab.component.ts)
contains:

- orientation (`h` or `v`);
- terminal or nested-container children;
- one normalized ratio per child.

Adding a pane either inserts into a compatible parent or wraps the target in a
new container of the required orientation. Normalization removes empty/single
containers, flattens adjacent containers with the same orientation, and
renormalizes ratios. The same tree drives pane geometry, drag/drop targets,
spanners, focus navigation, and serialization.

P-TRACK should adopt this data model, expressed as plain versioned data rather
than Angular component instances:

```json
{
  "type": "split",
  "orientation": "horizontal",
  "ratios": [0.55, 0.45],
  "children": [
    {"type": "terminal", "paneId": "pane-1"},
    {
      "type": "split",
      "orientation": "vertical",
      "ratios": [0.5, 0.5],
      "children": [
        {"type": "terminal", "paneId": "pane-2"},
        {"type": "terminal", "paneId": "pane-3"}
      ]
    }
  ]
}
```

The layout model must not contain backend session IDs because those are
ephemeral.

### Recovery

Tabby asks each tab type for a JSON recovery token. The split token recursively
stores orientation, ratios, and children. A local-terminal token stores the
profile, latest CWD, optional live PTY ID, and optional serialized xterm state.

The app writes recovery tokens to `localStorage` after layout hints, tab
changes, and a 30-second timer, debounced by one second. Xterm serialization
keeps up to 1,000 scrollback lines and excludes alternate-buffer content and
terminal modes.

A “live PTY ID” only helps while the process owning the PTY survives, such as a
renderer reload. It does not make an ordinary terminal process survive a full
application shutdown.

P-TRACK restoration semantics are:

- restore dock visibility/height, tabs, split tree, selected pane, profile, and
  last reported CWD;
- launch fresh processes after a full application restart;
- do not persist terminal output by default because output may contain tokens,
  paths, prompts, or other secrets;
- leave live process persistence to a later tmux-compatible profile or detached
  helper design.

### Close and shutdown

Tabby checks for active child processes before closing a local terminal, offers
a kill/cancel warning, and performs TERM-then-KILL graceful shutdown on Unix.
The PTY manager outlives renderer components, which enables reattachment by ID
within the same application process.

P-TRACK's Go manager similarly owns sessions independently of frontend DOM
nodes. Closing a live pane requires confirmation. Application shutdown closes
WebSockets, terminates process groups/ConPTY sessions, closes PTY handles, and
waits for goroutines with a bounded timeout.

## WailsTerm Findings

[WailsTerm](https://github.com/rlshukhov/wailsterm/tree/5d9859a5cfb570c4234e66d0a1bc2e384e56ce20)
validates Wails 2 + vanilla Vite + xterm.js + `go-pty`, but its transport is
important: terminal bytes use an authenticated loopback WebSocket. Wails
bindings handle URL discovery and resize, while a Wails event handles clear.

That is stronger than routing PTY bytes through Wails events, which serialize
event payloads as JSON. P-TRACK should retain this control-plane/data-plane
split.

WailsTerm remains a reference, not code to embed:

- it starts a separate HTTP listener for its terminal;
- it searches a fixed port range;
- resize is polled every two seconds;
- there is one terminal;
- tabs and splits are TODO;
- Windows and Linux are not tested;
- its source is MPL-2.0.

P-TRACK will use one dynamically assigned loopback listener for all sessions,
strict token/origin validation, explicit shutdown, event-driven resize, bounded
flow control, and its own Apache-2.0-compatible implementation.

## Proposed Architecture

### Frontend

- Vite with incremental TypeScript modules; keep the existing board DOM code
  intact during the spike.
- `@xterm/xterm` 6.0.0.
- Initial addons:
  - `@xterm/addon-fit` 0.11.0
  - `@xterm/addon-search` 0.16.0
  - `@xterm/addon-web-links` 0.12.0
  - `@xterm/addon-webgl` 0.19.0
- Follow-up/optional addons:
  - `@xterm/addon-ligatures` 0.10.0
  - `@xterm/addon-unicode-graphemes` 0.4.0
  - `@xterm/addon-clipboard` 0.2.0 for terminal clipboard protocols; ordinary
    copy/paste still uses Wails clipboard control
  - `@xterm/addon-image` 0.9.0 only after explicit performance/security testing

Addon versions are the compatible versions published with xterm 6.0.0 as of
2026-07-25 and must remain pinned by the package lock.

No TanStack package is needed:

- TanStack Start's SSR and server-function runtime duplicate Wails/Go.
- Router adds no value to the single-window workspace.
- Query is optional later if Wails snapshot caching becomes complex.
- A plain reducer/state module is sufficient for a versioned split tree.

### Go backend

Create an `internal/terminal` package independent of `internal/gui`:

```text
Profile catalog
    -> Manager
        -> Session
            -> go-pty PTY + process
        -> authenticated stream server
```

The manager owns all session maps, process lifecycle, stream attachment, and
shutdown. `gui.App` exposes only profile/lifecycle/resize bindings and emits
low-frequency status events.

Use `github.com/aymanbagabas/go-pty` v0.2.3. It provides Unix PTYs and Windows
ConPTY behind one Go interface. Wrap it behind a P-TRACK-owned factory so unit
tests use fakes and a future backend can be substituted without changing GUI
bindings.

### Transport

Bind one `http.Server` to `127.0.0.1:0`. Each session receives an unguessable
token and a URL such as:

```text
ws://127.0.0.1:<port>/terminal/<session-id>?token=<random-token>
```

Rules:

- reject missing/invalid tokens;
- accept only Wails application origins required by supported platforms;
- tokens are single-session and expire when the session closes;
- one active stream attachment per session;
- use binary frames for PTY bytes and terminal input;
- use small text control frames for ACKs only;
- bound frame/chunk size and unacknowledged output;
- set deadlines and close handlers;
- never expose the listener on a non-loopback interface.

Wails bindings/events remain the control plane:

- list profiles;
- create session and return its stream URL;
- resize;
- request close/kill/restart;
- report exit/status/title/CWD;
- native clipboard and external-link opening.

### Workspace model

```text
Workspace
  version
  dock { open, height, maximized }
  activeTabId
  tabs[]
    id, title, activePaneId, root SplitNode|TerminalNode

Pane descriptor
  paneId
  profileId
  cwd
  taskId?       (future association)
  notification preference
```

Backend session IDs and WebSocket URLs are runtime-only.

Persist a project-keyed workspace document in `localStorage` for the initial
implementation. Validate the version and shape before use; corrupt state falls
back to one default pane. A later change can move the same schema into the
global P-TRACK store if cross-webview migration or centralized settings require
it.

## Delivery Stages

### Stage A: transport spike

One resizable dock, one shell/agent profile, binary I/O, flow control, resize,
exit/restart, clean shutdown, DOM fallback, and a project-root CWD.

This stage answers the highest-risk questions before tabs or layout persistence:

- Is output smooth during large logs?
- Do interactive agent CLIs, curses apps, mouse input, Unicode, and IME work?
- Does resize remain stable while dragging the dock and window?
- Do all goroutines/processes terminate on pane and app close?
- Does the loopback transport work on macOS, Windows, and Linux webviews?

### Stage B: terminal usability

Native clipboard integration, multiline-paste warning, bracketed paste, search,
safe links, activity state, direct-process completion notification, profile
selection, and close confirmation.

### Stage C: workspace

Persistent tabs, recursive splits, draggable ratios, focus navigation,
maximize/equalize, restoration of profile/CWD/layout, and renderer lifecycle
management for hidden panes.

### Stage D: tracking integration

Associate a pane with a plan/task, launch an installed agent profile from task
context, and surface session state beside the board. This stage needs a separate
interaction design before automatically constructing agent prompts or changing
task status.

## Non-goals

- Embedding or importing Tabby.
- TanStack Start, SSR, or a second JavaScript backend.
- Installing shells or agent CLIs.
- Running commands without a user action.
- Persisting live processes across full P-TRACK shutdown.
- Prompt scraping as completion detection.
- SSH, serial, file transfer, ZMODEM, plugin compatibility, or terminal images
  in the initial delivery.
- Writing terminal output into the project database.

## Success Criteria

- An installed shell or agent CLI runs interactively in the GUI at the project
  root.
- A sustained high-output command remains responsive with bounded memory.
- Input/output is byte-correct across chunk boundaries.
- Resize and dock dragging do not flicker or flood PTY resize calls.
- Paste protection cannot be bypassed by ordinary keyboard paste.
- Links open only through Wails in the system browser.
- Closing a pane or app leaves no child process, PTY handle, listener, or
  goroutine behind.
- Board auto-refresh and keyboard shortcuts do not interfere while the terminal
  has focus.
- Workspace restoration accurately restores layout/profile/CWD and clearly
  starts fresh processes.
