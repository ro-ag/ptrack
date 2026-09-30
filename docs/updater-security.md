# Updater security model

p-track updates only from stable GitHub Releases published at
`ro-ag/ptrack`. The updater is an app-owned facility: it does not use agent
integrations, accept project configuration, store credentials, build
from source, elevate privileges, or run unattended installation helpers.

## Consent and authority

- Manual checks happen only when the user selects **Check for updates**.
- Automatic checks are disabled by default and persist only after explicit
  opt-in. Opting out cancels an admitted automatic check.
- A successful check grants authority only to download the exact candidate it
  returned. Download and installation are separate user actions fenced by that
  version.
- Frontend state contains bounded release facts and progress, never asset URLs,
  local stage paths, credentials, or transport errors.

## Release discovery

Discovery uses the fixed GitHub API endpoint for the latest `ro-ag/ptrack`
release and refuses metadata redirects. The response must describe one
published, stable SemVer release newer than the running official build. The
updater selects exactly one expected package name for the running OS and CPU
exactly one `checksums.txt` asset, and exactly one 64-byte
`checksums.txt.sig` asset. A release without that signature asset is not a
candidate at all.

Accepted packages are:

| Platform | Asset |
|---|---|
| macOS | `p-track_<version>_darwin_<arch>.dmg` |
| Windows, installed with the MSI | `p-track_<version>_windows_<arch>.msi` |
| Windows, any other copy | `ptrack_<version>_windows_<arch>.zip` |
| Linux standalone archive | `ptrack_<version>_linux_<arch>.tar.gz` |
| Linux AppImage | `p-track_<version>_linux_<arch>.AppImage` |

Debian/RPM and Nix-store installations can discover releases but cannot stage
or apply them. Their package manager owns replacement. A package marker only
classifies `/usr/bin/ptrack`; it does not capture a separate user-owned archive.
AppImage classification requires the executable to match the canonical
`APPDIR/usr/bin/ptrack`, not merely the presence of an environment variable.

GitHub-generated source tarballs and zipballs are not read from the response
and cannot become candidates. Prereleases, drafts, development versions,
downgrades, equal versions, duplicates, missing assets, arbitrary hosts,
queries, fragments, ports, and unexpected paths fail closed.

## Download and staging

Release assets are streamed into a private directory under the global p-track
home. Requests start from the exact GitHub download URL and may follow only the
bounded GitHub release-asset redirect chain. Package, manifest, response,
archive entry, release-note, and progress sizes are bounded.

`checksums.txt.sig` is a raw Ed25519 signature over the exact bytes of
`checksums.txt`. After both are staged, the updater verifies the signature
against the p-track release public key compiled into `ptrack-updater`, on every
platform, before it reads any digest from the manifest; a missing, wrong-sized,
or failing signature ends the operation before the package is downloaded.
`checksums.txt` must then contain one exact SHA-256 entry for the selected
package.
The archive must have the expected single-root layout and executable; path
traversal, links, extra entries, duplicate entries, and the wrong ELF or PE
machine type are rejected. The durable stage records archive and payload
digests and sizes. Files are reopened without following links and rehashed
before use.

AppImage stages hold the entire image as their payload, with a 512 MiB bound,
an ELF architecture check and a type-2 AppImage header check. They use the same
signed manifest chain and revalidation as other packages. Installation type is
checked again during apply and startup recovery so an old standalone-executable
stage cannot replace a package-owned or mounted executable.

## Release signing

The tag-only release job signs `checksums.txt` with the Ed25519 release key
held in the `PTRACK_RELEASE_SIGNING_KEY` repository secret, verifies the
signature against the pinned public key, and fails the release when the secret
is absent, so an unsigned release is never published. The private key never
leaves that secret, and the public key compiled into the updater is checked
against it by `tools/release_contract.py public-key`.

Because the signature is checked against a key built into the running binary
rather than anything the release itself supplies, a leaked release token or a
tampered release run can publish assets but cannot get them installed. macOS
adds a separate pinned Developer ID identity and Gatekeeper check at handoff.
Rotating the release key needs an app release that carries the new public key
before any release is signed with it.

## Platform handoff

### macOS

The DMG is rehashed, checked by `hdiutil`, and required to pass strict
`codesign` verification with Developer ID team `3CAJR4ZDMQ` plus Gatekeeper's
disk-image assessment. p-track then opens the verified whole DMG for the user to
complete installation. Replacing only `Contents/MacOS/ptrack` would invalidate
the signed app bundle and is never attempted.

### Windows

A copy counts as installed only when its executable sits in the folder the
per-user MSI recorded under `HKCU\Software\ro-ag\p-track\InstallDir`. Such a
copy stages the MSI instead of the ZIP. The staged package must be an OLE
compound file whose exact bytes match the signed manifest; p-track then starts
`msiexec.exe /i` from the absolute system directory and does not wait on it.
Windows Installer performs the per-user major upgrade without elevation and
offers to close the running app; p-track replaces no file itself.

Any other copy — the portable folder or the CLI archive — revalidates the ZIP
and payload, then opens Explorer through the absolute Windows directory path
with the verified archive selected. The running executable is not
overwritten. The user closes p-track, replaces the binary from the archive,
and reopens it. Both handoffs are launched rather than awaited, because
Explorer reports failure even after it opens the folder.

### Linux

For AppImages, the installer makes the verified complete image executable and
opens its private staging folder. The user closes the running app and replaces
the original image manually. No executable inside the mount is overwritten,
and no atomic-replacement journal is created for this handoff. Debian/RPM/Nix
installations refuse downloads and apply requests in the backend as well as
presenting package-manager instructions in the UI.

The current executable and parent directory must resolve canonically, be owned
by the current user, and reject group/world-writable or set-ID modes. p-track
uses a target-scoped lock, copies the verified staged payload, rechecks its
digest, creates an inode-verified hard-link backup, persists a target-bound
recovery journal, and atomically renames the replacement. It probes
`ptrack version` for the exact candidate and rolls back on any failure. It never
uses `sudo` or updates a system-owned binary. Every helper command runs with
closed standard input and a per-command deadline, and on Unix its whole process
group is killed on timeout or cancellation. A canceled version probe rolls the
replacement back.

## Recovery and failure behavior

Startup examines a bounded number of private stage directories, validates every
candidate before publishing it, resolves Linux recovery journals against their
owning stage, keeps the newest valid upgrade, and prunes verified superseded
stages. Missing, stale, malformed, canceled, or tampered inputs never gain
installation authority. An unresolved journal, unsafe target, ambiguous backup,
or excessive saved-stage backlog enters an explicit recovery-required state and
blocks new update work.

When recovery is required, close p-track and inspect the installation together
with `$PTRACK_HOME/updates` (or `~/.ptrack/updates`). On Linux, preserve any
`.pending-apply-*.json` record and sibling `.ptrack-backup-*` executable until
the installed target is identified; deleting either first can remove the
evidence needed for a safe rollback. Reinstalling the same or a newer official
package is the conservative recovery path. Do not copy a staged payload into
place or bypass its ownership, checksum, signature, or platform checks.

Checks, downloads, and applies are single-flight and bound to app shutdown.
Canceling signals the active context and retains the operation fence until the
worker exits. Public errors are static and bounded so transport details, URLs,
paths, and credentials cannot cross into the frontend.

The Rust implementation keeps this authority in `ptrack-updater` and the
app-owned `UpdateRuntime`. The native Tauri shell receives only the existing
six typed desktop commands and the one-way `update:state-changed` event; it is
not granted an updater, HTTP, filesystem, process, or shell plugin.

## Acceptance checks

Run these when changing update discovery, staging, installation, recovery, or
the About & Updates experience. They complement automated tests; they are not
a release procedure and never publish, tag, or upload anything.

Automated gates:

```sh
cargo fmt --all -- --check
cargo test -p ptrack-updater --all-targets
cargo test -p ptrack-app --lib
cargo clippy -p ptrack-updater -p ptrack-app --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps

cd frontend
npm ci
npm test
npm run build
```

Compile the updater for every release target, then execute the OS-specific
Rust tests on native macOS, Windows, and Linux hosts. Live GitHub release and
macOS trust-chain checks are deliberate native acceptance steps, never an
unannounced default test side effect.

App behavior criteria (exercise the native Tauri app with no project open and
with an open project):

- Default startup makes no update request. About & Updates opens from both
  the version trigger and the native Check for Updates… item (the p-track app
  menu on macOS, the Help menu elsewhere) on the Projects screen.
- A manual check contacts only the p-track GitHub Release endpoint. An opt-in
  survives restart; opting out during an admitted automatic check cancels it.
- Check, download, and install are separate actions. Cancel leaves the UI in
  an actionless canceling state until the worker exits.
- Progress stays bounded and does not repeatedly announce the whole dialog.
  Release notes are plain text and the release-page action opens only the
  validated p-track GitHub URL.
- Unknown, stale, malformed, tampered, unsigned, unsupported, development,
  downgrade, and recovery-required states expose no new update authority. A
  release whose `checksums.txt.sig` is missing or does not verify against the
  pinned release key is refused before its package downloads.
- Tab and Shift+Tab remain inside the dialog without focusing the backdrop;
  Escape and the backdrop close it; focus returns to the invoker. VoiceOver,
  NVDA, or Orca announces phase changes without reading asset paths or URLs.

Native handoff acceptance is the platform handoff contract above: record the
exact native OS and architecture exercised in the pull request. A cross
compile is useful but does not count as execution of OS-tagged tests.
