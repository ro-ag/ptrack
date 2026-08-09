<div align="center">

![P-TRACK — persistent project memory for humans and AI agents](assets/brand/banner.png)

Keep goals, plans, tasks, decisions, issues, and commit context alive across
terminal sessions—without a server or a cloud account.

[![Go](https://img.shields.io/badge/Go-1.26%2B-00ADD8?logo=go&logoColor=white)](https://go.dev/)
[![Release](https://img.shields.io/badge/release-v0.17.0-5FAFFF)](https://github.com/ro-ag/ptrack/releases/tag/v0.17.0)
[![License](https://img.shields.io/badge/License-Apache--2.0-3DD6A3)](LICENSE)
[![Storage](https://img.shields.io/badge/Storage-local--first-AFA8FF)](#storage-and-safety)

</div>

![P-TRACK launch screen](docs/assets/welcome.png)

P-TRACK gives one project two complementary interfaces:

- **Humans get a full-screen terminal dashboard.** Run `ptrack` to browse and
  edit the live project state, move work on a board, and perform maintenance.
- **Humans also get a canonical desktop project workspace.** Run
  `ptrack gui [PATH]` for project switching, tracking context, read-only Git
  intelligence, registered agent runs, terminals, and the kanban board.
- **Agents get small, scriptable commands.** Run `ptrack context` to restore a
  bounded handoff, then query or update only what the current task needs.

Both interfaces use the same embedded database. The TUI opens it only for each
action, so an agent and a human can work side by side without the dashboard
holding a long-lived lock.

## Contents

- [Install](#install)
- [Quick start](#quick-start)
- [The terminal dashboard](#the-terminal-dashboard)
- [The desktop project workspace](#the-desktop-project-workspace)
- [Organize tasks](#organize-tasks)
- [Agent workflow](#agent-workflow)
- [Command reference](#command-reference)
- [Storage and safety](#storage-and-safety)
- [Development](#development)

## Install

Download the native archive for your platform from the
[GitHub releases page](https://github.com/ro-ag/ptrack/releases), then place the
`ptrack` executable somewhere on your `PATH`. Every release binary includes the
CLI, terminal dashboard, and Wails desktop GUI.

On macOS you can instead download `P-TRACK_<version>_darwin_<arch>.dmg` and drag
**P-TRACK.app** into `/Applications`. The app is not signed with a Developer ID
yet, so on first launch either right-click the app and choose **Open**, or run
`xattr -dr com.apple.quarantine /Applications/P-TRACK.app`. The same executable
inside the bundle doubles as the CLI; symlink it onto your `PATH` with
`ln -s /Applications/P-TRACK.app/Contents/MacOS/ptrack /usr/local/bin/ptrack`.

Do not install P-TRACK with `go install`. Wails requires platform-specific build
tags, CGO setup, and native linker flags that `go install module@version` cannot
apply. P-TRACK rejects plain Go application builds instead of producing a
binary whose `--gui` option fails at runtime. Building from source requires Go
1.26 or newer and the Wails prerequisites for your platform.

## Quick start

Initialize a project and give it a north star:

```sh
cd your-project
ptrack init --goal "Ship the widget service"
ptrack plan add "Build the storage layer"
ptrack plan use 1
ptrack task add "Define bbolt buckets" --plan 1
ptrack task start 1
ptrack note add "Chose bbolt over Badger" --task 1
```

Now choose the interface that fits the job:

```sh
ptrack          # human: open the interactive dashboard
ptrack gui       # human: open the desktop project workspace
ptrack context  # agent: restore a compact project handoff
ptrack next     # agent: ask for the single most-actionable task
```

Running `ptrack` outside an initialized project shows a branded getting-started
screen instead of a database error.

## The terminal dashboard

Bare `ptrack` opens a focused P-TRACK launch screen: a high-density Unicode
wordmark with one highlighted action and a few small shortcuts underneath.
Press `enter` to open the dashboard, `1`–`5` to jump directly to a screen, or
`?` to open the command menu. Narrow terminals fall back to a compact line-art
brand so the launch screen never overflows.

![P-TRACK overview dashboard](docs/assets/overview.png)

Inside the dashboard, the header becomes compact again. The numbered navigation
stays visible at the top, contextual actions stay visible at the bottom, and
`?` opens the command menu from any screen.

![P-TRACK command menu](docs/assets/command-menu.png)

Use `↑`/`↓` and `enter` in the command menu, or press its shortcut directly.
The five main screens are also available with `1`–`5`:

| Key | Screen | What it is for |
|---:|---|---|
| `1` | **Overview** | Browse plans and tasks; add, edit, move, promote, complete, or annotate work. |
| `2` | **Board** | See the active plan as Todo, Doing, Blocked, and Done columns. |
| `3` | **Milestones** | Review project checkpoints, their plans, due dates, and task rollups. |
| `4` | **Issues** | Track problems and bugs by severity and status. |
| `5` | **Maintenance** | Inspect project storage, reload concurrent changes, create backups, and review agent upkeep commands. |

### Work from the board

The board is a live kanban view of the selected plan. Move between columns with
`←`/`→`, select a card with `↑`/`↓`, and change its status with `H`/`L`.

![P-TRACK kanban board](docs/assets/board.png)

Press `enter` on a plan, task, milestone, issue, or card to open its full item
view. Notes, linked entities, explanations, and recorded commits are shown in
scrollable nested panels; `enter` or `esc` returns to the dashboard.

### Keep the project healthy

Maintenance is a first-class screen instead of a hidden shortcut. It shows the
project root, database location, schema, last writer, and backup destination.

![P-TRACK maintenance screen](docs/assets/maintenance.png)

- `r` reloads changes written by an agent or another CLI process.
- `B` creates a timestamped database backup.
- `ptrack guide` refreshes the instructions installed for AI agents.
- `ptrack hook install` records git commits in the project audit trail.

### Keyboard map

| Scope | Keys |
|---|---|
| Launch screen | `enter` dashboard · `1`–`5` jump · `?` menu · `q` quit |
| Everywhere else | `?` menu · `tab`/`shift+tab` switch · `1`–`5` jump · `g` goal · `m` summary · `r` reload · `B` backup · `q` quit |
| Overview | `←`/`→` pane · `↑`/`↓` select · `a` add · `e` edit · `M` move task to plan · `P` convert task to plan · `u` activate plan · `x` complete plan · `s`/`d`/`b` task status · `n` note |
| Board | `←`/`→` column · `↑`/`↓` card · `H`/`L` change status · `a` add · `e` edit · `M` move to plan · `P` convert to plan · `n` note |
| Milestones | `↑`/`↓` select · `a` add · `e` rename · `x` complete · `o` reopen |
| Issues | `↑`/`↓` select · `a` add · `e` rename · `c` close · `o` reopen |
| Item view | `↑`/`↓` scroll · `pgup`/`pgdn` page · `e` edit · `M` move task · `P` convert task to plan · `r` refresh · `enter`/`esc` back |

## The desktop project workspace

Use the Wails GUI for the canonical native project workspace:

```sh
ptrack gui
ptrack gui ../another-project
ptrack board --gui          # compatible alias
ptrack board --gui --plan 4
```

![P-TRACK desktop kanban board](docs/assets/gui-board.png)

Outside a project, the app opens a welcome screen with recent projects. Use the
native directory picker to open or switch projects, or close a project without
exiting the app. P-TRACK confirms before a transition stops active terminals or
registered agent runs.

The project-memory rail combines a bounded tracking snapshot with repository
and storage status, read-only Git status/remotes/branches/commits/divergence,
active terminals, and explicitly registered agent runs. Select a plan from the
header, drag cards between Todo, Doing, Blocked, and Done, or use the status
selector on a card. Add tasks from the board header,
double-click a card to rename it, or record durable task context with **Memory**.
Cards surface linked notes, commits, and open issues, while the project-memory
rail keeps the goal, agent handoff, project status, issues, and recent decisions
in view. The board refreshes automatically while it is open; press `R` to reload
immediately after another process changes the project.

Registered agent runs keep a bounded on-disk history, so they survive app
restarts and project switches: a run interrupted by a restart comes back
marked stale instead of disappearing, and an external agent whose lease is
still valid resumes heartbeating on its own. Agents integrating over the local
API should treat the registry descriptor as stale when its hosting process is
gone — after a crash, wait for a fresh descriptor instead of dialling a dead
port.

![P-TRACK task-memory dialog](docs/assets/gui-memory.png)

The bottom dock hosts one resizable embedded terminal at the project root.
Choose the default login shell or a detected installed-agent profile, then use
an interactive PTY-backed shell. Copy, paste, selection-aware `Ctrl+C`,
platform shortcuts, and the right-click terminal menu use the native clipboard.
Multiline text is held behind a bounded review dialog and sent through xterm's
bracketed-paste behavior only after confirmation. Exited sessions show their
status and can be restarted without reopening the board. Search the 25,000-line
scrollback with `⌘F` on macOS or `Ctrl+Shift+F` elsewhere, change the persisted
font size from the toolbar or standard zoom shortcuts, and clear or reset the
emulator without stopping its shell. WebGL rendering retries after a lost GPU
context and keeps xterm's built-in renderer as its fallback.

The top-right panel controls can independently hide the project sidebar, board,
or terminal; hiding the board gives the terminal the full workspace height
without restarting its session. The sidebar remains pointer- and
keyboard-resizable. **Modern Unicode** enables Unicode 15 grapheme and emoji
cell-width handling by default; turn it off from the terminal toolbar to return
to xterm's built-in compatibility mode.

Clipboard, shortcut, context-menu, restart, and shutdown interactions have been
verified on macOS. Windows and Linux archives include the native implementation,
but their PTY, clipboard, input, and descendant-process cleanup paths still
require interactive validation. Curses and mouse input, IME and Unicode,
sustained high-volume output, resize stress, and sleep/wake recovery also remain
in the manual acceptance matrix.

Like the terminal dashboard, the GUI opens the database only for each action.
The CLI and AI agents can therefore keep reading and writing the same project
without the board retaining bbolt's write lock.

## Organize tasks

Move a task without recreating it:

```sh
ptrack task move 12 --plan 4
```

When a task grows into a workstream, convert it into a plan:

```sh
ptrack task convert 12
```

Conversion is atomic. The new plan keeps the task's title, creation time,
milestone, notes, and commits. A done task becomes a done plan; other task
statuses become an active plan. The original task is removed, and linked issues
are retained but unlinked because issues cannot target plans.

In the dashboard, select a task and press `M` to move it or `P` to convert it.
The same actions are available from `?` and from the task's item view.

## Agent workflow

A fresh agent starts with `ptrack context`. The digest is intentionally bounded:
it restores the goal, rolling summary, active plan, blockers, open issues,
recent notes, and inventory without dumping the whole project.

```sh
ptrack context                # restore the live edge
ptrack next                   # choose the next task
ptrack task show 12           # drill into one item
ptrack note add "..." --task 12
ptrack task done 12
ptrack summary set "..."      # leave a compact handoff
```

Read commands render Markdown by default because it is compact for an LLM.
Add `--json` at automation boundaries.

### Agent onboarding

`ptrack init` installs a short, marker-delimited P-TRACK section into the
project's `AGENTS.md` and `CLAUDE.md`. Existing content is preserved, and
re-running `ptrack guide` updates only that managed section.

Use `ptrack init --no-guide` to skip guide installation. Personal working
agreements can live at `~/.ptrack/guide.md` or `$PTRACK_HOME/guide.md`; P-TRACK
appends them to the installed guide without changing the defaults shipped to
other users.

### Audit trail and commits

Notes are the human-visible record of what an agent did and why. Install the git
hook once with `ptrack hook install`; future commits are recorded automatically.
Put `#<task-id>` in a commit message to link the commit to that task.

## Command reference

| Command | Purpose |
|---|---|
| `ptrack init [--goal S] [--root D] [--force] [--no-guide]` | Create or refresh `.ptrack/` and the agent guide. |
| `ptrack guide [--print]` | Install, refresh, or print the agent guide. |
| `ptrack goal show\|set S` | Show or update the north-star goal. |
| `ptrack summary show\|set S` | Show or update the rolling context summary. |
| `ptrack milestone add\|list\|show\|done\|open\|due\|rename` | Manage checkpoints that group plans. |
| `ptrack plan add\|list\|show\|done\|use\|rename` | Manage plans; `show` includes tasks and notes. |
| `ptrack task add\|list\|show\|start\|done\|block\|rename\|move\|convert` | Manage tasks; move them between plans or convert them into plans. |
| `ptrack issue add\|list\|show\|close\|open\|severity\|rename` | Track issues and bugs, optionally linked to tasks. |
| `ptrack note add\|list` | Attach or list project, plan, and task notes. |
| `ptrack commit add\|list\|show\|record` | Browse the recorded git audit trail; `show` prints the diff. |
| `ptrack hook install` | Install the post-commit hook that records commits. |
| `ptrack context [--json]` | Print the bounded restore digest. |
| `ptrack next [--json]` | Print the most-actionable task in the active plan. |
| `ptrack gui [PATH]` | Open the canonical desktop project workspace; PATH defaults to the current directory. |
| `ptrack board [--plan N] [--json] [--gui]` | Print a kanban board or open it as a Wails desktop GUI. |
| `ptrack search <term> [--json]` | Search plan and task titles plus note bodies. |
| `ptrack status [--json]` | Print a compact project overview. |
| `ptrack projects [--json]` | List projects in the global registry. |
| `ptrack backup` | Copy the current project database into global backups. |
| `ptrack version` | Print the P-TRACK version. |

Run `ptrack <command> --help` for flags and examples specific to a command.

## Storage and safety

P-TRACK is local-first and has no server process.

| Store | Location | Contents |
|---|---|---|
| Project | `.ptrack/ptrack.db` | Goal, summary, milestones, plans, tasks, issues, notes, and commit records. |
| Global | `~/.ptrack/global.db` | Configuration, the project registry, and backup metadata. |
| Backups | `~/.ptrack/backups/` | Timestamped copies created by `ptrack backup` or `B` in the TUI. |

Set `PTRACK_HOME` to move the global store and backups. Project discovery walks
upward from the current directory, similar to git. Values are encoded Go
structures stored in [bbolt](https://github.com/etcd-io/bbolt); JSON is produced
only when a command is explicitly asked for `--json` output.

## Development

```sh
go test ./...
go vet ./...
```

Application builds always go through Wails so the resulting executable includes
the CLI, terminal dashboard, and desktop GUI. Install the
[Wails v2 prerequisites](https://wails.io/docs/gettingstarted/installation/)
for your platform, then run:

```sh
make build
./build/bin/ptrack version
```

On macOS, two more targets produce the branded desktop artifacts:

```sh
make package   # build/bin/P-TRACK.app — bundle id com.ro-ag.ptrack, version
               # stamped from git, icon from build/appicon.png
make dmg       # build/bin/P-TRACK-<version>-macOS-<arch>.dmg with an
               # /Applications drop link
```

With a Developer ID identity in the login keychain (`SIGN_IDENTITY` is the
certificate SHA-1 fingerprint, found via `security find-identity -v -p
codesigning`):

```sh
make sign        # Developer ID signature: hardened runtime, entitlements,
                 # secure timestamp
make signed-dmg  # signed app + signed disk image (Gatekeeper still warns
                 # until notarized)
make release-dmg # full pipeline: sign, DMG, sign, notarize, staple — needs a
                 # one-time `xcrun notarytool store-credentials ptrack-notarize`
```

The release workflow runs the same steps on tag pushes: macOS builds are
signed when the `APPLE_CERTIFICATE_*` secrets exist, and notarized as well
once the `APPLE_API_*` secrets are added.

Bundle metadata lives in `build/darwin/` (`Info.plist`, `Info.dev.plist`,
hardened-runtime `entitlements.plist` for future signed releases). Brand
assets — the icon generator, PNG exports, `AppIcon.icns`, the README banner,
and a social card — live in [`assets/brand/`](assets/brand/); regenerate them
with `make icons`.

The equivalent command, for environments without `make`, is:

```sh
cd frontend
npm ci
npm run build
cd ..
go run github.com/wailsapp/wails/v2/cmd/wails@v2.13.0 build \
  -clean -nopackage -trimpath -windowsconsole
```

Architecture and product design notes live in
[`docs/superpowers/`](docs/superpowers/). The deferred cgo-free Tauri host,
`native-ipc` runner, and libghostty-vt migration is captured separately in
[`docs/tauri-rust-recode.md`](docs/tauri-rust-recode.md).

## License

[Apache License 2.0](LICENSE) © 2026 ro-ag.
