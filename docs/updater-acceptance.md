# Updater acceptance matrix

Run this matrix when changing update discovery, staging, installation,
recovery, or the About & Updates experience. It complements automated tests;
it is not a release procedure and does not publish, tag, or upload anything.

## Automated checks

```sh
gofmt -w internal/updater/*.go internal/gui/*.go
go test ./...
go test -race ./...
go vet ./...

cd frontend
npm ci
npm test
npm run build
```

Compile the OS-tagged updater tests for every release target, then execute them
on native macOS, Windows, and Linux hosts. Opt-in live contract tests verify the
current stable GitHub Release and the macOS DMG trust chain:

```sh
PTRACK_LIVE_UPDATE_TEST=1 go test -run 'TestLive(LatestReleaseContract|DarwinDMGTrustContract)' -v ./internal/updater
```

The live tests contact GitHub and should be run deliberately, not as an
unannounced default test side effect.

## App behavior

Exercise the native Wails app with no project open and with an open project.

- Default startup makes no update request. About & Updates opens from both the
  version trigger and native Settings menu on the Welcome screen.
- A manual check contacts only the p-track GitHub Release endpoint. An opt-in
  survives restart; opting out during an admitted automatic check cancels it.
- Check, download and install are separate actions. Cancel leaves the UI in an
  actionless canceling state until the worker exits.
- Progress stays bounded and does not repeatedly announce the whole dialog.
  Release notes are plain text and the release-page action opens only the
  validated p-track GitHub URL.
- Unknown, stale, malformed, tampered, unsupported, development, downgrade, and
  recovery-required states expose no new update authority.
- Tab and Shift+Tab remain inside the dialog without focusing the backdrop;
  Escape and the backdrop close it; focus returns to the invoker. VoiceOver,
  NVDA, or Orca announces phase changes without reading asset paths or URLs.

## Native handoff

| Platform | Acceptance |
|---|---|
| macOS | A current signed/notarized release DMG passes checksum, `hdiutil`, pinned-team `codesign`, and Gatekeeper checks before opening. The app bundle is not modified in place. |
| Windows | The verified architecture-matched ZIP is selected in the real Windows Explorer. The running executable is unchanged until the user closes p-track and replaces it. |
| Linux | A current-user standalone executable replaces atomically, reports the exact new version, retains its safe mode, and requests restart. Unsafe ownership/modes refuse. Forced probe failure rolls back; a crash journal recovers only the bound original or verified replacement. |

Record the exact native OS and architecture used in the pull request. A cross
compile is useful but does not count as execution of OS-tagged tests.
