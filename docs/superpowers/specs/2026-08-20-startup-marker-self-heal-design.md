# Startup marker self-heal

## Problem

`~/.ptrack/runtime/active-generation.json` lists every bound project. At startup,
`validate_active_generation` (crates/ptrack-store/src/runtime_binding.rs) canonicalizes
every listed project root and fails closed on the first error. When a project root
directory has been deleted (observed 2026-08-19: session scratchpad test projects under
`/private/tmp/claude-501/...`), `fs::canonicalize` returns ENOENT and the whole app and
CLI refuse to start with "runtime recovery is required: No such file or directory
(os error 2)". One dead directory bricks everything; there is no recovery command.

## Decision

Silent self-heal at startup (user-selected over a GUI recovery prompt or a
CLI-only `ptrack recover` command).

## Design

**Where.** `ActiveRuntime::load` in crates/ptrack-app/src/production.rs — the single
chokepoint used by the GUI desktop authority, the routed CLI application, bootstrap,
and target validation. All callers heal for free.

**Flow.**
1. Attempt the normal load (shared cutover lease, marker load, validation).
2. On error: drop the shared lease, acquire the **exclusive** cutover lock, reload the
   marker, and partition `marker.projects` by `path_is_present(root)`
   (`fs::symlink_metadata`, no symlink following).
3. Nothing missing → return the original fail-closed error unchanged.
4. Some missing → back up the current marker bytes to
   `runtime/active-generation.json.pruned-<unix-epoch>` (0600), then publish the pruned
   marker via the existing `install_active_generation` (which re-validates the
   remainder under the exclusive lease). Drop the exclusive lease and retry the normal
   load once.
5. If the pruned marker still fails validation, propagate that error (no marker
   rewrite happens — `install_active_generation` validates before publishing).

**Fail-closed preserved.** The heal only acts on direct evidence: a listed project root
whose directory is absent. Marker corruption, non-canonical paths, writer-version
mismatches, and global-store failures error exactly as today. The bootstrap-plan
recovery gate ("bootstrap recovery must complete before runtime load") is checked
before validation and is untouched.

**Backup rationale.** The pruned entries carry `database_id` bindings. The backup file
preserves them for manual re-adoption if a pruned root reappears (e.g. remounted
volume). Automatic re-adoption is out of scope; today an existing
`.ptrack/ptrack.redb` under a new init target already routes to "an unmapped Rust
project database requires recovery", and that path is unchanged.

## Out of scope

- GUI notification of the prune.
- Auto-forgetting stale entries in the recents registry (recents legitimately outlive
  unmounts and already render as unavailable).
- Re-adopt flow for pruned projects whose storage returns.

## Testing

In crates/ptrack-app/src/production_test.rs, against a temp global home:

1. Delete one bound project root → `ActiveRuntime::load` succeeds, the marker no longer
   lists it, the remaining projects still load, and a `.pruned-*` backup exists.
2. Unrelated marker damage (e.g. non-canonical marker JSON) with all roots present →
   load still fails closed and the marker is not rewritten.
3. All roots present → load succeeds without rewriting the marker (no backup created).
