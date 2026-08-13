# Terminal acceptance matrix

This matrix is the repeatable support gate for p-track's existing Go PTY,
loopback stream, and xterm renderer. It deliberately excludes the deferred
Rust, Tauri, native-IPC, and Ghostty recode.

Record only the result fields below. Do not paste terminal output, commands,
clipboard contents, environment values, credentials, prompts, or transcripts
into an issue, pull request, or p-track note.

## Result record

Use one record per operating-system and architecture combination:

```text
date_utc: YYYY-MM-DD
ptrack_commit: <git oid>
os: macos | windows | linux
os_version: <public version>
architecture: amd64 | arm64
webview: wkwebview | webview2 | webkitgtk
result: pass | fail | unavailable
failed_checks: <matrix IDs only, or none>
issue_ids: <IDs only, or none>
```

`unavailable` is not a pass. A platform is supported only when all required
rows have current passing evidence on that platform. Never turn an unavailable
tool into a hidden skip or claim interactive support from compilation alone.

## Plan 6 result records

```text
date_utc: 2026-08-11
ptrack_commit: e3f243270476e02b0e78ab0cc6c30a266207f4a1
os: macos
os_version: 26.5.2
architecture: arm64
webview: wkwebview
result: pass
failed_checks: none
issue_ids: none
```

```text
date_utc: 2026-08-11
ptrack_commit: e3f243270476e02b0e78ab0cc6c30a266207f4a1
os: windows
os_version: 10.0.26200.8873
architecture: arm64
webview: webview2
result: pass
failed_checks: none
issue_ids: none
```

```text
date_utc: 2026-08-11
ptrack_commit: e3f243270476e02b0e78ab0cc6c30a266207f4a1
os: linux
os_version: Ubuntu 24.04.3 LTS
architecture: arm64
webview: webkitgtk
result: pass
failed_checks: none
issue_ids: none
```

## Setup

1. Start from a clean checkout and build the application with `make build`.
2. Run `go run ./tools/terminal-acceptance inventory`. It reports only
   platform facts and executable availability; it intentionally omits paths,
   versions, environment values, and authentication state.
3. Open the checkout with the built p-track GUI. Use a disposable project and
   safe fixture text. Do not use a shell containing production credentials.
4. Run the automated validation block at the end of this document before the
   interactive rows.

## Interactive matrix

| ID | Area | Repeatable procedure | Passing result | Required platforms |
|---|---|---|---|---|
| SH-1 | Default shell | Open the discovered default shell; run `printf 'ready\\n'`, an interactive prompt, arrows, Home/End, F1, Ctrl+C, and Ctrl+D. | Correct project CWD, input editing, signals, function keys, and one clean exit event. | macOS, Windows, Linux |
| SH-2 | Additional shells | For each available configured shell (zsh, bash, fish, PowerShell, or cmd), repeat SH-1. | Every shown profile launches directly with its argument array; unavailable profiles are visibly unavailable. | Where available |
| AG-1 | Agent CLIs | Open each discovered installed agent (Agy, Claude Code, Codex, Gemini, OpenCode) and exercise one harmless interactive prompt without granting a capability. | UI remains interactive; discovery installs or authenticates nothing; terminal close cleans up the process. | Where available |
| TU-1 | Curses and alternate screen | Run `less README.md`, then an available `vim README.md` or `nvim README.md`; enter and leave the alternate screen repeatedly. | Screen restores without residue, keyboard input works, and scrollback is not corrupted. | macOS, Windows, Linux |
| IN-1 | Mouse | Run `go run ./tools/terminal-acceptance interactive`; move, click, drag, and scroll inside the pane. | Mouse coordinates/actions update and no board action fires through the terminal. | macOS, Windows, Linux |
| IN-2 | Keyboard layouts and IME | In the interactive fixture, enter `café`, `日本語`, `한국어`, and one composed character with the platform IME; resize during composition. | Composition stays in the active pane, no shortcut steals it, and the rune count advances without duplicated input. | macOS, Windows, Linux |
| RD-1 | Unicode and emoji | Run `go run ./tools/terminal-acceptance render` with Modern Unicode both enabled and disabled. | Combining, wide, emoji, and flag fixtures remain legible; enabled mode aligns the sample separators. | macOS, Windows, Linux |
| RD-2 | Hyperlinks | In RD-1, click the OSC 8 fixture normally, then modifier-click it (Cmd on macOS, Ctrl elsewhere). | Plain click does nothing; modifier-click opens only the exact HTTPS fixture externally. | macOS, Windows, Linux |
| CB-1 | Clipboard and selection | Select safe fixture text; use platform copy, paste, terminal Ctrl+C, keyboard context menu, and right-click actions. | Selection copy never sends SIGINT; Ctrl+C without a selection does; clipboard access stays native. | macOS, Windows, Linux |
| CB-2 | Multiline paste | Paste two safe lines, cancel, repeat and confirm; repeat in the alternate-screen fixture. | Ordinary mode shows an escaped bounded review; cancel sends nothing; confirm uses bracketed paste; alternate screen follows terminal mode. | macOS, Windows, Linux |
| AX-1 | Screen readers and keyboard access | With VoiceOver on macOS, NVDA on Windows, or Orca on Linux, navigate two named panes, terminal rows, compact terminal actions, and the multiline-paste dialog; rename the tab and close one split while focus remains in the terminal. Repeat with bounded and sustained output. | Each live pane has a current distinct label, rendered rows are navigable without duplicate announcements, controls expose full names/focus/disabled state, paste detail is announced without forcing the preview, focus returns safely, and sustained output remains responsive. | macOS, Windows; Linux observed and qualified |
| RS-1 | Resize and visibility | Rapidly resize the window, dock, sidebar, and splits; hide/show board and terminal; use full-height mode and switch tabs. | Final PTY dimensions match the pane, sessions stay attached, viewport position is preserved, and hidden panes do not consume WebGL budget. | macOS, Windows, Linux |
| RC-1 | Renderer recovery | With a running fixture, background/foreground, sleep/wake, change displays or scaling, and force a WebGL context loss using the webview inspector when available. | The renderer retries within its bound, falls back to DOM, refits on reveal, and terminal I/O continues. | macOS, Windows, Linux |
| ST-1 | Sustained output | Run `go run ./tools/terminal-acceptance output --mib 100`; scroll away from the bottom and monitor the app process. | Output completes, UI remains responsive, queued transport stays bounded, scrollback stays within the selected profile bound, and the viewport does not jump. | macOS, Windows, Linux |
| LC-1 | Process cleanup | Start a shell child and grandchild using platform-native commands; close the pane normally and forcibly, then close the project and app. | The entire owned process tree exits and the loopback listener is gone; unrelated processes remain untouched. | macOS, Windows, Linux |
| DG-1 | Recovery controls | Exercise stream disconnect, renderer failure, an unresponsive process, and corrupt saved layout using the documented development fixtures. | Diagnostics expose content-free state and bounded actions; retry/restart/force-stop/reset affect only the selected current-generation resource. | macOS, Windows, Linux |

## Recovery fixtures

Use only disposable sessions and never save inspector output:

1. **Disconnected stream:** with a harmless shell running, terminate only its
   WebSocket from the webview inspector without copying its URL. Diagnostics
   must report a disconnected or failed stream. **Restart terminal** must close
   the old session idempotently and create a fresh process and one-shot stream;
   no reconnect action is offered.
2. **Renderer fallback:** request `WEBGL_lose_context` for the selected xterm
   canvas when the inspector exposes it, or run the packaged Linux fixture with
   WebGL unavailable. After three bounded retries diagnostics must report DOM
   fallback. **Retry terminal renderer** affects only the selected, visible
   current-generation pane and never restarts its PTY.
3. **Unresponsive process:** start a disposable child that ignores graceful
   termination. **Force stop terminal** must require confirmation, revoke the
   selected session's authority, and use the host force-close path. Sibling and
   unrelated processes must remain alive.
4. **Corrupt layout:** replace only the disposable project's terminal-workspace
   storage value with bounded malformed JSON, then reopen the project. The raw
   value must be deleted, diagnostics must report a discarded layout, and the
   project-scoped reset control must create a fresh stopped layout after any
   live-pane confirmation.

## Automated validation

Run these from the repository root on every platform before recording an
interactive result:

```sh
gofmt -w tools/terminal-acceptance/*.go
go test ./tools/terminal-acceptance ./internal/terminal ./internal/gui
go test -race ./internal/terminal ./internal/gui
go vet ./...
cd frontend && npm ci && npm test && npm run build
```

The release workflow compiles native Rust/Tauri applications on macOS, Windows, and
Linux only when an explicit version tag is pushed. A successful compile is
useful evidence for build compatibility but never substitutes for this
interactive matrix.
