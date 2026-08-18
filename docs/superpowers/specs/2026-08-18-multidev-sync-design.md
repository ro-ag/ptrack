# Multi-developer sync — design

Date: 2026-08-18
Status: approved

## Problem

ptrack is single-player today: `.ptrack/` is git-ignored, so every clone has a
private database. Two developers tracking the same repo see disjoint plan sets,
the active plan is a project-wide singleton, and no mutation records who made
it. This design makes multiple developers (and agents) coordinate planning
state through git — without ever putting the database in git, without a server,
and without destroying anyone's work on conflict.

## Goals

- Multiple developers share one logical set of plans/tasks per repo, synced
  through git alone (local-first, offline-capable).
- One developer per plan, enforced (hard claim, explicit release).
- Every mutation attributed to a stable identity (humans and agents).
- Git pushes/pulls can never corrupt a database.
- All machines deterministically converge to the same state.
- Existing 0.25/0.26 databases upgrade smoothly and automatically.

## Non-goals

- Real-time sync (freshness is "as of last git pull", like code).
- A server or daemon of any kind.
- Sharing the global `~/.ptrack` database — strictly project-DB scope.
- Journal compaction (append-only forever; years of headroom at planning
  volume; a rewrite-shared-history feature is explicitly deferred).
- Hard deletes in the shared vocabulary. `ConvertTaskToPlan` stays the only
  delete, handled by aliasing (below).

## Architecture

Local redb per machine remains the storage engine and the only thing any
surface (CLI/TUI/GUI/agent) reads or writes. New committed directory
`.ptrack-shared/` is the transport:

- One append-only JSONL journal per (identity, machine):
  `<identity-id>.<machine-id>.jsonl`. Each file has exactly one writer, so git
  merges never conflict by construction. `.ptrack-shared/.gitattributes` ships
  `*.jsonl merge=union` as belt-and-braces.
- A journal line is one event: `{ulid, actor, hlc, op, payload…}` carrying
  every minted or derived value (entity ULID, display number, timestamps,
  order, and for convert: the minted plan ULID and mapped status).
- Journal files start with a format-version header line.
- `git pull` only drops text lines in the worktree; nothing touches the DB at
  git time. The next ptrack command replays unseen lines into the local DB
  under the normal single-writer commit.

## Identity

- `ptrack config set user <name>` mints a stable random identity ID once per
  user; the display name is mutable metadata (a profile event). Renames and
  name collisions cannot break attribution or filenames.
- A machine ID is minted at first use per host. Same human on two machines =
  two journal files, still single-writer each.
- Agents may hold their own identities.
- Every mutation is stamped with the acting identity (`actor`), including in
  single-player mode once P1 ships (`legacy` sentinel for pre-existing
  records).

## Ordering and convergence

- Events are stamped with a hybrid logical clock (HLC): UTC-based,
  `max(last_hlc + 1, utc_now)`, persisted in the local DB, advanced on every
  ingested event. Wrong wall clocks cannot win races or reorder cause before
  effect.
- Total order across all journals: `(hlc, actor_id, ulid)` — ULID strictly as
  final tiebreak, never a causality key. Identical on every machine.
- Replay goes through a dedicated deterministic applier, separate from the
  interactive mutation path. The applier takes all values from the event,
  calls no clock, reads no table counts, and is total over valid history
  (never errors on a valid event). The interactive path becomes: validate
  against local state → mint values → append event → apply via the same
  applier. One code path applies events everywhere.
- `Set*` mutations (title, status, severity, due, hold…) apply
  last-writer-wins per field by HLC. Transition guards run at append time on
  the author's machine only — never at replay (replay-time guards are a
  divergence engine).
- Replay is idempotent by ULID; applying the same event twice is a no-op.

## Claims — one developer per plan

- `plan use <id>` claims the plan for your identity and sets *your* active
  plan (active plan becomes per-actor, not a project singleton).
- Content mutations to a plan claimed by someone else are refused locally.
  Holds, notes, and issue links stay open to everyone — they are the
  cross-developer communication channel. The Mutation surface is explicitly
  classified claim-gated vs open.
- `plan release` frees a claim. `plan use --steal` takes over, incrementing
  the claim epoch.
- Claims carry an integer epoch. Two claims at the same epoch (both devs
  claimed before seeing each other's event) = deterministic `claim-conflict`
  state on every machine: nobody's work is reverted or destroyed, the plan
  refuses further content mutations everywhere, and `ptrack plan resolve`
  (human decision) picks the owner going forward. Enforcement is prospective,
  never retroactive.
- Terminal plan status (done/archived) auto-releases the claim — the
  interactive actor emits the release event alongside the status event
  (replay never emits).
- `ConvertTaskToPlan` on a claimed plan births the new plan already claimed
  by the converting actor.

## IDs

- Every shared entity gets a ULID at creation. Journals and all
  cross-references (milestone→plan, plan→task, issue↔task, notes, commits)
  use ULIDs only.
- Display numbers are actor-prefixed and minted by the creator, carried in
  the event: `r12`, `a7` (short actor handle + per-actor counter). Stable and
  identical on every machine — "task r12" means the same entity in commit
  messages, agent contexts, and on every developer's screen. No bare-number
  per-machine aliases.
- Pre-share entities keep their existing numbers under the share-initiating
  actor's prefix via the genesis snapshot.

## Failure handling

- Fail-closed ingest per event: schema-validated, size-capped, actor must
  match the journal file's identity (no impersonation). Same-ULID-different-
  bytes (equivocation) is a journal integrity error.
- Prefix-blocking, never skip-and-continue: an unknown or invalid event halts
  that journal's cursor at that line, and the global replay frontier clamps
  to the earliest halt point. Machines fall behind ("upgrade required to see
  events after X") but never diverge. True garbage halts the whole journal
  loudly; the DB is untouched.
- Cursor = (byte offset, ULID of last applied line), advanced in the same
  redb transaction as the application. On mismatch (git reset/rewind), rescan
  the file from zero — idempotent, cheap — and re-append own locally-applied
  events missing from the file.
- Tombstone-with-alias for convert: ops targeting a converted task retarget
  through the alias (task→plan), exactly as convert itself retargets notes;
  non-retargetable ops become deterministic recorded no-ops with a visible
  notice.
- Same-machine concurrency (GUI + CLI + TUI): redb single-writer + idempotent
  applier + in-transaction cursors; the existing per-host cutover flock
  already covers bootstrap. No new mechanism.

## Migration and compatibility

Smooth path from any 0.25/0.26 database, no manual steps:

- One consolidated payload-schema bump (schema 3) designed up front, carrying
  all P1–P3 fields: `actor` (optional; `legacy` sentinel for old records),
  optional entity ULID (minted at share init or on demand), per-actor active
  plan map (falls back to the old singleton value), claim/epoch storage.
  Ranged acceptance becomes 1..=3 with the existing lazy per-record upgrade
  on write — a 0.25 or 0.26 DB opens as-is and upgrades automatically as
  records are touched. No fleet-wide cutover per phase; this is the only
  payload bump the feature ever ships.
- After first write by a schema-3 binary, older binaries refuse the database
  fail-closed (existing gate behavior, documented in Help Center
  troubleshooting exactly as the 0.26 schema-2 note is).
- `ptrack share init` (opt-in, per project) additionally: bumps
  STORE_SCHEMA_VERSION so journal-unaware binaries physically cannot open a
  share-enabled DB; mints ULIDs for all existing entities; writes a genesis
  snapshot event into the initiator's journal so a fresh clone reconstructs
  the entire pre-existing state; records share-enabled in the DB.
- Non-sharing projects never see the store-schema bump and keep working
  single-player, unchanged.

## Phasing

- **P1 — identity + schema.** `ptrack config set user`, actor stamping on all
  mutations, the full consolidated schema-3 record shapes (fields present,
  mostly unpopulated). Ships alone; single-player behavior unchanged.
- **P2 — per-dev active plan + claims (shared-host).** Active plan becomes
  per-actor; hard claims with epochs on a single shared DB (SSH/team-box
  case). No distributed machinery yet.
- **P3a — journals + replay, advisory cross-machine claims.** HLC,
  deterministic applier, genesis, store-schema lockout, cursors, prefix
  blocking. Cross-machine claims are visible and warn-on-mutate while the
  replay engine soaks in real use. Claims remain hard on shared-host DBs.
- **P3b — hard cross-machine claims.** Flip enforcement on: refusal +
  `claim-conflict` + `plan resolve`, once P3a has proven convergent.

## Testing

- Applier determinism: property test — random event sets applied in journal
  order on N simulated machines fold to byte-identical state.
- Idempotency: every event applied twice = no-op.
- Convergence with interleaved replay orders (different pull timings).
- Claim races: same-epoch conflict detection, steal chains, terminal
  auto-release, convert-inherits-claim.
- Migration: 0.25-era and 0.26-era fixture DBs open, read, lazily upgrade on
  write (extends the existing schema-1 fixture e2e test).
- Ingest hostile input: oversize lines, bad JSON, actor mismatch,
  equivocation, future format versions — all halt fail-closed, DB untouched.
- Cursor rewind: git reset scenarios re-scan and re-emit own events.
