# Updater security model

p-track updates only from stable GitHub Releases published at
`ro-ag/ptrack`. The updater is an app-owned facility: it does not use agent
network capabilities, accept project configuration, store credentials, build
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
and exactly one `checksums.txt` asset.

Accepted packages are:

| Platform | Asset |
|---|---|
| macOS | `p-track_<version>_darwin_<arch>.dmg` |
| Windows | `ptrack_<version>_windows_<arch>.zip` |
| Linux | `ptrack_<version>_linux_<arch>.tar.gz` |

GitHub-generated source tarballs and zipballs are not read from the response
and cannot become candidates. Prereleases, drafts, development versions,
downgrades, equal versions, duplicates, missing assets, arbitrary hosts,
queries, fragments, ports, and unexpected paths fail closed.

## Download and staging

Release assets are streamed into a private directory under the global p-track
home. Requests start from the exact GitHub download URL and may follow only the
bounded GitHub release-asset redirect chain. Package, manifest, response,
archive entry, release-note, and progress sizes are bounded.

`checksums.txt` must contain one exact SHA-256 entry for the selected package.
The archive must have the expected single-root layout and executable; path
traversal, links, extra entries, duplicate entries, and the wrong ELF or PE
machine type are rejected. The durable stage records archive and payload
digests and sizes. Files are reopened without following links and rehashed
before use.

The checksum and package are co-hosted in the same GitHub Release. SHA-256
therefore detects corruption and mismatched assets, but it is not an
independent publisher signature if the repository's release authority is
compromised. macOS adds a separate pinned Developer ID identity and Gatekeeper
check at handoff; the current Windows and Linux archives rely on the release
account plus the co-hosted checksum.

## Platform handoff

### macOS

The DMG is rehashed, checked by `hdiutil`, and required to pass strict
`codesign` verification with Developer ID team `3CAJR4ZDMQ` plus Gatekeeper's
disk-image assessment. p-track then opens the verified whole DMG for the user to
complete installation. Replacing only `Contents/MacOS/ptrack` would invalidate
the signed app bundle and is never attempted.

### Windows

The ZIP and payload are revalidated, then Explorer is opened through the
absolute Windows directory path with the verified archive selected. The
running executable is not overwritten. The user closes p-track, replaces the
binary from the archive, and reopens it.

### Linux

The current executable and parent directory must resolve canonically, be owned
by the current user, and reject group/world-writable or set-ID modes. p-track
uses a target-scoped lock, copies the verified staged payload, rechecks its
digest, creates an inode-verified hard-link backup, persists a target-bound
recovery journal, and atomically renames the replacement. It probes
`ptrack version` for the exact candidate and rolls back on any failure. It never
uses `sudo` or updates a system-owned binary.

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
