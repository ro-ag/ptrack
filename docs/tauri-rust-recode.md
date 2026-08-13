# Rust and Tauri runtime architecture

Status: approved target architecture for Plan 8. The Go/Wails application is
the production implementation only until the parity and cutover gates pass.

## Runtime invariant

The shipped p-track CLI, TUI, desktop application, background services,
terminal host, updater, and capability broker are one all-Rust runtime. The
runtime must not spawn, link, or call a Go sidecar.

Go may remain after cutover only in a separately invoked, offline migration
helper that reads a named legacy bbolt database. The helper has no application
IPC surface, is never started automatically, never receives credentials, and
cannot modify a legacy database. Rust validates its bounded output before
creating a distinct redb candidate.

The existing HTML/CSS/JavaScript frontend stays in `frontend/`. Tauri is the
native window and IPC boundary, not an authority shortcut: frontend commands
reach application services, and those services alone reach storage, process,
network, terminal, and updater boundaries.

## Process topology

```text
ptrack CLI -----------+
ptrack TUI -----------+--> ptrack-app services --> ptrack-store --> redb
Tauri WebView --> IPC-+          |       |
                                 |       +--> bounded git / agent / PTY
                                 |
                                 +--> capability broker --> HTTP / Git / SSH
                                 +--> updater verifier --> platform handoff

offline operator only:
legacy bbolt --read-only--> Go exporter --> bounded stage --> Rust importer
```

There is one policy path for an operation regardless of whether it begins in
the CLI, TUI, or GUI. Tauri commands remain thin, typed adapters. They do not
open databases, launch arbitrary processes, read arbitrary files, or perform
network requests.

## Workspace and dependency direction

The target workspace has these ownership boundaries:

```text
frontend/                  existing workspace UI and backend adapter
src-tauri/                 Tauri shell, menus, lifecycle, command/event adapter
crates/ptrack-core/        models, validation, search, reports, pure services
crates/ptrack-store/       redb schema, typed transactions, paths, backups
crates/ptrack-app/         use cases, workspace generations, authorization seams
crates/ptrack-git/         bounded repository and worktree inspection
crates/ptrack-agent/       run evidence, associations, handoffs, drift, proposals
crates/ptrack-capability/  normalization, authorization, broker, audits, executors
crates/ptrack-terminal/    PTYs, profiles, streams, shell integration, cleanup
crates/ptrack-updater/     discovery, verified staging, recovery, native handoff
crates/ptrack-cli/         command parsing, output and exit compatibility
crates/ptrack-tui/         terminal UI presentation and input flows
crates/ptrack-db-import/   offline batch validation and redb candidate creation
tools/ptrack-db-export/    retained read-only Go migration helper
```

Dependencies point inward. `ptrack-core` has no platform or storage authority.
`ptrack-store` owns database handles and depends on the native record contract.
The bounded Git, agent, capability, terminal, and updater crates expose narrow
services to `ptrack-app`; they do not depend on a UI. CLI, TUI, and Tauri depend
on `ptrack-app`, never directly on redb or an executor. Cross-cutting DTOs live
at the narrowest owning boundary rather than in the desktop shell.

Unsafe native integrations and helper runners are not required for cutover.
If one is added later, it must be disposable, versioned, authenticated, and
purpose-built. It receives only the minimum declared data and capabilities and
never inherits ambient p-track authority. `native-ipc` and Ghostty are possible
future adapters, not Plan 8 runtime foundations.

## Database paths and ownership

Legacy Go paths are immutable migration sources:

- project: `<project>/.ptrack/ptrack.db`;
- global: `$PTRACK_HOME/global.db`, with the existing platform default when
  `PTRACK_HOME` is unset.

Rust application paths are distinct and fixed:

- project: `<project>/.ptrack/ptrack.redb`;
- global: `$PTRACK_HOME/global.redb`.

The storage layer refuses the legacy filenames for redb creation. It never
discovers a user database speculatively: the CLI/application service resolves
the project and global roots, then supplies an explicit path to the storage
owner. New Rust-era projects create only the Rust paths.

Only p-track storage code may hold a writable redb handle. Migration helpers
may create new candidates through that API, but a staged import remains
immutable and cannot be opened for application writes until activation has
replaced its staging provenance in one verified storage transaction.

## Migration batch and activation contract

Every migration is an explicit batch rooted at
`$PTRACK_HOME/migrations/<batch-id>/`. The directory is private to the user and
contains versioned, bounded records with canonical JSON and SHA-256 digests:

- `plan.json`: immutable source identities, destination paths, expected store
  kinds, source format versions, and the proposed activation generation;
- `journal.json`: resumable state transitions and per-source snapshot, export,
  import, reopen, hash, count, sequence, quarantine, and backup receipts;
- `handoff.json`: published only after every destination verifies, with state
  `READY_FOR_CUTOVER` and the digest of both preceding records;
- `receipt.json`: published last after activation, binding the installed
  runtime, activation generation, exact active database identities, retained
  legacy sources, and rollback material.

Records are rewritten with create-new temporary files, file and parent
directory synchronization, and atomic rename. A state transition names its
unique predecessor and monotonically advances; recovery rejects a gap,
rollback, duplicate terminal state, unknown field/version, path substitution,
or digest mismatch. Logs and receipts contain no credentials, database
payloads, terminal content, or authority-bearing URLs.

The single routing authority is
`$PTRACK_HOME/runtime/active-generation.json`. It identifies one generation,
the global redb, and the canonical project-root-to-redb mapping. It is replaced
atomically only after the batch handoff and every installed destination have
been reopened and reverified. Runtime discovery may find a project directory,
but it must fail closed unless the marker binds that canonical root, path,
store identity, schema, and generation. New projects are added through the same
storage-owned marker update.

The marker is not user-editable configuration. Unknown versions, partial
files, unsafe modes, symlinks, missing entries, identity changes, or a database
generation mismatch produce a recovery-required state and no writable open.

## Writer fencing and source retention

The standalone Rust activation tool owns the cross-runtime writer fence. It
must hold all of these from the final source snapshot through marker commit:

1. an exclusive private activation lock at
   `$PTRACK_HOME/runtime/cutover.lock`;
2. a bbolt-compatible read lock on every legacy source, proving no Go writer is
   open and preventing one from entering during the final export;
3. exclusive redb ownership of every candidate or installed destination; and
4. pinned parent/file identities around every publication step.

Failure to acquire every fence aborts before activation. The Go/Wails runtime
does not participate in activation and must be stopped; the tool never guesses
that a process is harmless from its PID or name.

Legacy databases are never deleted, renamed, truncated, chmodded, or reused.
The batch keeps a verified transaction-consistent backup receipt as well as
the original source identity. Automated cleanup is forbidden. A later explicit
retention command may be designed only after cutover evidence exists; it is not
part of application startup or update.

## Startup, rollback, and recovery

Startup reads and validates the active-generation marker before opening any
store. It then reopens every required store, verifies its manifest, generation,
kind, path identity, permissions, and capability state, and only then publishes
the workspace generation to callers. Validation failure opens no store for
writes and reports a bounded recovery-required error.

Activation retains the previous marker and exact database identities in the
batch receipt. Before the first committed Rust application write, a failed
startup may atomically restore the previous marker while the activation tool
holds the same fence. Once any store records a write in the new generation,
automatic rollback to a legacy source or older database is forbidden because
that would discard acknowledged state. Recovery then means repairing or
restoring the bound Rust generation from a verified backup, or installing a
compatible newer application; all alternatives remain explicit and offline.

Application code never silently falls back from redb to bbolt, from one
generation to another, or from a failed project store to a different path.
Updater rollback may restore application bytes only when the retained binary
supports the active storage schema; it never changes the database marker as a
side effect.

## Capability and IPC boundaries

All network and remote-process authority remains deny by default. A decoded or
migrated capability is inert until Rust policy code normalizes its complete
scope, recomputes the approval digest byte-for-byte, verifies profile,
generation, revision, expiry, operation, target, and limits at the point of
use, and obtains explicit approval. A mismatch disables the grant and produces
only bounded audit metadata.

The initial Tauri shell enables only the main application window and the IPC
needed for its explicitly registered commands. It starts with no shell,
filesystem, HTTP, process, dialog, or updater plugin authority. Each future
command and plugin requires a reviewed permission entry and still calls the
same Rust application service used by the CLI and TUI.

Loopback servers bind only to the loopback interface, use one-shot bounded
tokens tied to the active workspace generation, and publish private descriptor
files. Subprocesses receive a minimal environment, explicit argv, time and
output limits, process-tree cleanup, and no credential values in diagnostics.

## Delivery sequence and cutover gate

1. Freeze the Go compatibility contract and all-Rust boundaries.
2. Scaffold the Tauri shell around the unchanged frontend with no ambient
   authority.
3. Complete core services and typed application storage.
4. Port CLI, TUI, Git, agent, capability, PTY, GUI, updater, and packaging
   behavior behind the shared service boundaries.
5. Produce complete automated, fixture, and native manual evidence for the
   parity matrix and its current-feature extension on every supported target.
6. Exercise migration, activation, application writes, recovery, and rollback
   only with disposable copies.
7. Remove the Wails and Go runtime after every gate passes, retaining only the
   isolated read-only exporter.

The Go/Wails build remains the production fallback until the exact Rust
candidate passes the complete gate. Compilation or cross-compilation alone is
not parity, and an unavailable native result is not a pass.
