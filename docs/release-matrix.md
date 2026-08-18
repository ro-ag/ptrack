# Release matrix

This document defines the supported OS/architecture targets for p-track and
the criteria that must be true before a target ships. It is not a release
procedure: it does not publish, tag, or upload anything. See
`.github/workflows/release.yml` for the mechanics (tag-triggered only) and
`docs/updater-acceptance.md` / `docs/terminal-acceptance.md` for the related
interactive matrices.

## Supported targets

| OS | Arch | CI runner | Minimum OS / baseline | Webview | Package format | Signing |
|---|---|---|---|---|---|---|
| macOS | amd64 | macos-15-intel | 12.0 (`minimumSystemVersion` in `src-tauri/tauri.conf.json`) | wkwebview | Signed, notarized DMG + signed (not notarized/stapled) CLI tar.gz | Developer ID signing on both packages; notarization + stapling on the DMG only |
| macOS | arm64 | macos-15 | 12.0 | wkwebview | Signed, notarized DMG + signed (not notarized/stapled) CLI tar.gz | Developer ID signing on both packages; notarization + stapling on the DMG only |
| Windows | amd64 | windows-2025 | Windows as provided by windows-2025 | webview2 | ZIP | None (unsigned) |
| Windows | arm64 | windows-11-arm | Windows as provided by windows-11-arm | webview2 | ZIP | None (unsigned) |
| Linux | amd64 | ubuntu-24.04 | glibc/webkit2gtk as provided by ubuntu-24.04 | webkitgtk | tar.gz | None (unsigned) |
| Linux | arm64 | ubuntu-24.04-arm | glibc/webkit2gtk as provided by ubuntu-24.04-arm | webkitgtk | tar.gz | None (unsigned) |

These six targets are the exact `build` matrix in `.github/workflows/release.yml`.
`tools/release_contract.py`'s `package_names()` hardcodes the same two arches
via `ARCHES = ("amd64", "arm64")`, but emits eight package names, not six: for
each arch it lists a darwin `.dmg`, a darwin CLI `.tar.gz`, a linux `.tar.gz`,
and a windows `.zip` (macOS ships two packages per arch — DMG plus the signed
but not notarized/stapled CLI archive — while Windows and Linux ship one
each). There is no distro
matrix pinned for Linux beyond the ubuntu-24.04 runner images; no other libc,
webview engine, or distro is validated.

Every package ships alongside a `checksums.txt` (SHA-256, one line per
package, written by `tools/release_contract.py checksums`) in the same
GitHub release.

## macOS signing

- Identity: Developer ID, hardened runtime (`codesign --options runtime`),
  entitlements from `build/darwin/entitlements.plist`.
- Notarization: `xcrun notarytool submit --wait` then `xcrun stapler staple`
  and `xcrun stapler validate`.
- Team pinned: the `build` job's "Verify macOS updater trust contract" step
  requires `certificate leaf[subject.OU] = "3CAJR4ZDMQ"` on the built DMG.
- The `build` job's "Require updater-compatible macOS credentials" step
  fails the run closed if `APPLE_CERTIFICATE_BASE64`,
  `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
  `APPLE_API_KEY`, `APPLE_API_KEY_ID`, or `APPLE_API_ISSUER` is missing —
  there is no unsigned macOS release path.

## Ship criteria (all platforms)

A target does not ship unless all of the following hold for the tagged
revision:

1. **CI test job green.** The `test` job in `release.yml` (ubuntu-24.04)
   passes: frontend tests/build, `cargo fmt --check`, `cargo test --workspace`,
   `cargo clippy -D warnings`, `cargo doc -D warnings`, and
   `tools.release_contract_test` / `tools.help_check_test` plus
   `tools/help_check.py all` (this cross-checks the newest CHANGELOG version
   against the README badges, `tauri.conf.json`, `Cargo.toml`, and the help
   pages — it does not compare that version against the pushed tag).
2. **Build succeeds for that target.** The matching row of the `build` job
   compiles, and for macOS also signs and notarizes successfully.
3. **Release contract validates.** `tools/release_contract.py validate-binary`
   passes at build time (exact version string, correct machine type) and
   `validate-dist` / `checksums` pass in the `release` job. Tag↔version
   equality is enforced separately in that job by `release-notes`, which
   requires a CHANGELOG `## [X.Y.Z]` section matching the tag-derived
   version — all before `gh release create --verify-tag` runs.
4. **checksums.txt present** for every package in the published release.
5. **Native acceptance evidence exists**, per `.github/workflows/native-acceptance.yml`:
   - Linux and Windows: the `native` job (always runs on `pull_request` and
     `push` to `main` for the tracked paths) built and smoke-tested the
     target.
   - macOS: the `native-macos` job, which is label-gated
     (`native-acceptance-approved`) on pull requests and otherwise runs on
     push to `main`.
6. **Updater handoff criteria met** per `docs/updater-acceptance.md`'s
   native handoff table for that platform (signed/notarized DMG passes
   checksum + `hdiutil` + pinned-team `codesign` + Gatekeeper on macOS;
   verified ZIP selected in Explorer on Windows; atomic replace with
   rollback on Linux).

## Updater

`crates/ptrack-updater` is the only update path and only ever targets these
six combinations:

- Discovery is from the GitHub releases `/latest` endpoint only
  (`crates/ptrack-updater/src/discovery.rs`), and maps the running host to
  one of the six `os`/`arch` pairs above (`Target::host`).
- Prereleases, drafts, and development builds are rejected (the release tag
  must be a canonical, non-prerelease, non-draft release, and the running
  binary's own version must parse as a release version).
- Downgrades are rejected: a candidate is only accepted if its version is
  strictly greater than the current version.
- Trust: SHA-256 verification against the release's `checksums.txt`, plus
  platform-native trust on macOS (`hdiutil verify`, `codesign`, `spctl`
  Gatekeeper assessment) before install.
- Apply mechanism differs by platform: Linux performs an atomic replace with
  journal-based rollback; Windows requires the user to close p-track and
  manually replace the executable via Explorer; macOS installs a new signed,
  notarized, stapled DMG.

## Explicitly unsupported

- 32-bit builds (only amd64/arm64 are built or validated).
- musl/Alpine Linux, or any Linux libc/webview baseline other than what
  ubuntu-24.04 / ubuntu-24.04-arm provide.
- Prerelease or non-canonical tags — `release.yml`'s "Validate stable release
  tag" step rejects anything that isn't `vX.Y.Z`, and the updater rejects
  prerelease/draft releases outright.
- Development builds — the updater refuses to operate when its own version
  string doesn't parse as a release version.
