# Release candidate checklist

A repeatable local gate for validating a release candidate commit. It
complements automated CI; it is not a release procedure.

This checklist does **not**:

- create or push a `v*` git tag
- publish a GitHub release or run `gh release create`
- upload any artifact, checksum, or release note anywhere
- require signing or notarization credentials (those steps are marked
  optional and, even when run, submit only to Apple's notary service —
  never to a release)

Releases publish via GitHub Actions on tag push only
(`.github/workflows/release.yml`). Nothing in this document tags or pushes.

## Prerequisites

- Rust toolchain matching the version pinned in CI workflows (currently
  1.89.0) (`rustc`, `cargo`, `clippy`, `rustfmt`) on `PATH`.
- Node.js + npm for `frontend/`.
- `python3` (stdlib only; no extra packages required).
- Optional, macOS packaging rehearsal only: a `SIGN_IDENTITY` codesign
  identity for `make sign`/`make signed-dmg`, and a `ptrack-notarize`
  keychain profile for `make notarize`. Skip section 5's signing steps if
  you don't have these.

## 1. Version and changelog consistency

- [ ] Confirm the candidate version. The canonical version is the newest
      `## [X.Y.Z]` heading in `CHANGELOG.md` (skip `## [Unreleased]`).
- [ ] Run:

  ```sh
  python3 -B tools/help_check.py all
  ```

  Expected: `Help Center validation passed: ... version <X.Y.Z>, ...` and
  exit code 0. This cross-checks the changelog version against the README
  badges, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`,
  `crates/ptrack-cli/Cargo.toml`, `docs/help/site.json`,
  `docs/help/search-index.json`, `docs/help/assets/screenshots/manifest.json`,
  and the `ptrack-version` meta tag / visible version badge in every help
  HTML page.

## 2. Full local quality gate

- [ ] Run:

  ```sh
  make test
  ```

  Expected: exit code 0. Chains `frontend-install`/`frontend-test`/
  `frontend-build` (npm ci, npm test, npm run build), `cargo fmt --all --
  --check`, `cargo test --workspace --all-targets --no-fail-fast`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS='-D warnings'`, and
  the help-check unit tests + `tools/help_check.py all`.
- [ ] Or, to run only the help/docs gate on its own:

  ```sh
  make help-check
  ```

## 3. Release contract and artifact validation

- [ ] Run the release contract and help-check unit tests (the same pair CI
      runs in the `test` job):

  ```sh
  python3 -B -m unittest tools.help_check_test tools.release_contract_test
  ```

  Expected: `OK`.
- [ ] `validate-dist` and `checksums` are **CI-only**, not runnable from
      this checklist's local artifacts: `validate-dist` requires the dist
      directory to contain exactly all 8 package names across the six
      release targets (`tools/release_contract.py:141-145`), and
      `checksums` calls `validate-dist` first. Section 5 below only
      produces the host machine's own DMG + tar.gz, so both steps fail
      closed on a local run. They run for real in the `release` job
      against the complete downloaded artifact set from all six `build`
      matrix legs:

  ```sh
  python3 -B tools/release_contract.py validate-dist  dist <X.Y.Z>
  python3 -B tools/release_contract.py checksums      dist <X.Y.Z>
  ```

- [ ] `release-notes` only reads `CHANGELOG.md`, so it is runnable locally
      without a complete dist directory:

  ```sh
  python3 -B tools/release_contract.py release-notes  CHANGELOG.md <X.Y.Z> <destination-file>
  ```

  It extracts the `## [X.Y.Z]` section of `CHANGELOG.md` into
  `<destination-file>` and touches nothing on GitHub or any remote.
- [ ] `validate-binary` runs automatically as part of `make package` (see
      section 5), but can be run standalone against any built binary:

  ```sh
  python3 -B tools/release_contract.py validate-binary <path-to-binary> <X.Y.Z> <os> <arch>
  ```

  `<os>` is one of `darwin`/`linux`/`windows`, `<arch>` is one of
  `amd64`/`arm64`.

## 4. Per-target native crate tests

- [ ] On each native OS available to you, run the crate-scoped test set
      that mirrors `native-acceptance.yml`:

  ```sh
  cargo test --all-targets --no-fail-fast \
    -p ptrack-app -p ptrack-capability -p ptrack-desktop \
    -p ptrack-store -p ptrack-terminal -p ptrack-updater
  ```

  Expected: all pass on that host. A cross compile does not substitute for
  running this on the real OS/architecture; record which platforms you
  actually ran it on.

## 5. macOS packaging and signing rehearsal (macOS only, local, no upload)

- [ ] Package, unsigned:

  ```sh
  make package
  ```

  `make package` builds an unsigned `.app` (via `tauri build`, which does
  its own `cargo build` — a preceding `make build` is not needed and its
  output goes unused) and runs `validate-binary` against it automatically;
  it fails closed if the version or architecture is wrong.
- [ ] Produce local artifacts (no upload):

  ```sh
  make archive   # CLI tar.gz into dist/
  make dmg       # unsigned DMG into dist/
  ```
- [ ] Optional, only with `SIGN_IDENTITY` credentials available — signing
      rehearsal:

  ```sh
  make sign          # codesign the .app, then verify-sign
  make verify-sign    # codesign --verify (re-runnable standalone)
  make signed-dmg     # signed DMG into dist/
  ```

  `make dmg` and `make signed-dmg` write to the same DMG path in `dist/`;
  running `signed-dmg` after `dmg` overwrites the unsigned one.
- [ ] Optional, only with a `ptrack-notarize` keychain profile
      (`NOTARY_PROFILE`) available — notarization rehearsal:

  ```sh
  make notarize
  ```

  This submits the DMG to Apple's notary service and staples the ticket.
  It does not upload anything to a GitHub release and does not tag or
  publish. Skip this step entirely without credentials.

## 6. Updater and manual acceptance pointers

- [ ] Updater-specific automated + manual acceptance: run the minimal
      command block in `docs/updater-acceptance.md` and, when updater or
      About & Updates behavior changed, walk its app-behavior and native
      handoff sections.
- [ ] Terminal-specific manual acceptance: when PTY/terminal/renderer
      behavior changed, walk the interactive matrix in
      `docs/terminal-acceptance.md`.
- [ ] Per-target ship criteria: cross-check against
      `docs/release-matrix.md` rather than re-deriving them here.
- [ ] VoiceOver/screen-reader acceptance is tracked separately (see
      `docs/terminal-acceptance.md`'s AX-1 row and the updater's app
      behavior section) and is not a release-candidate gating step here.

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
failed_checks: <section numbers or matrix IDs only, or none>
issue_ids: <IDs only, or none>
```

`unavailable` is not a pass. A candidate is release-ready only when every
required platform has a current passing record.
