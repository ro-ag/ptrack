# Inventory: Domain model (Go v0.21.0)

Scope: `internal/model` (`model.go`, `model_test.go`). This package defines
the persistent data types only; IDs, transitions, and invariants are enforced
in `internal/store` (covered by the STORE inventory) — noted here where the
model type itself constrains the contract.

## Contracts

### Serialization & storage representation

- MODEL-001: All persistent entities are serialized with Go `encoding/gob`
  into bbolt buckets; the structs carry no behavior beyond their fields. A
  Rust rewrite must either reproduce gob wire format byte-for-byte to read
  existing `.ptrack/ptrack.db` files, or ship a migration. — src:
  internal/model/model.go:1-4 — pinned by: internal/model/model_test.go
  TestGobRoundTrip — verify: fixture (decode an existing DB) / automated test
- MODEL-002: Gob round-trip must preserve these entities losslessly: `Meta`,
  `Plan`, `Task`, `Note`, `ProjectRef`, `Capability` (including nested
  `CapabilityLimits`, `CapabilityAuditPolicy`, and pointer scope `GitScope`).
  — src: internal/model/model_test.go:24-63 — pinned by: TestGobRoundTrip —
  verify: automated test
- MODEL-003: `time.Time` fields are gob-encoded (GobEncode binary format);
  zero `time.Time` is meaningful (e.g. `Milestone.Due` zero = no due date).
  — src: internal/model/model.go:119 — pinned by: none (zero-value semantics
  exercised implicitly in store tests) — verify: fixture
- MODEL-004: `Meta.FormatVersion` zero means a pre-versioning (v0.1.0)
  database, adopted as version 1 on first open. — src:
  internal/model/model.go:104-107 — pinned by: none in this package (see
  internal/store/version_test.go) — verify: fixture
- MODEL-005: `Meta.LastWriteVersion` is the ptrack semver that last wrote the
  DB; recorded for diagnostics only, never gates behavior. — src:
  internal/model/model.go:108-110 — pinned by: none — verify: manual

### Meta (singleton per-project record)

- MODEL-010: `Meta` fields: `Goal string`, `Summary string`,
  `ActivePlan uint64`, `CreatedAt time.Time`, `UpdatedAt time.Time`,
  `FormatVersion uint`, `LastWriteVersion string`. — src:
  internal/model/model.go:98-111 — pinned by: TestGobRoundTrip — verify:
  automated test
- MODEL-011: `Meta` is a singleton per project: north-star goal, rolling
  context summary, and currently active plan pointer. `ActivePlan == 0` means
  no active plan (uint64 zero value). — src: internal/model/model.go:96-97 —
  pinned by: none in this package (store: internal/store/store.go:191-197,
  TestActivePlanRequiresExisting internal/store/store_test.go:71) — verify:
  automated test

### Milestone

- MODEL-020: `Milestone` fields: `ID uint64`, `Title string`,
  `Status MilestoneStatus`, `Due time.Time` (zero = no due date),
  `Order int`, `CreatedAt time.Time`, `UpdatedAt time.Time`. — src:
  internal/model/model.go:115-123 — pinned by: none in this package — verify:
  automated test
- MODEL-021: `MilestoneStatus` values: `"open"` (`MilestoneOpen`), `"done"`
  (`MilestoneDone`). Exactly two. — src: internal/model/model.go:67-70 —
  pinned by: none — verify: automated test

### Plan

- MODEL-030: `Plan` fields: `ID uint64`, `Title string`,
  `Status PlanStatus`, `MilestoneID uint64` (0 = unassigned), `Order int`,
  `CreatedAt time.Time`, `UpdatedAt time.Time`. — src:
  internal/model/model.go:127-135 — pinned by: TestGobRoundTrip — verify:
  automated test
- MODEL-031: `PlanStatus` values: `"active"` (`PlanActive`), `"done"`
  (`PlanDone`), `"archived"` (`PlanArchived`). Exactly three. — src:
  internal/model/model.go:34-39 — pinned by: TestGobRoundTrip (uses
  PlanActive) — verify: automated test
- MODEL-032: Invariant — at most one plan is "currently being worked on";
  this is enforced via `Meta.ActivePlan` (single uint64 slot), not via the
  PlanStatus value: the store validates the plan exists before pointing at it.
  — src: internal/model/model.go:96-101; internal/store/store.go:191-197 —
  pinned by: internal/store/store_test.go TestActivePlanRequiresExisting —
  verify: automated test

### Task

- MODEL-040: `Task` fields: `ID uint64`, `PlanID uint64` (owning plan,
  required — no "0 = unassigned" documented), `Title string`,
  `Status TaskStatus`, `Order int`, `CreatedAt time.Time`,
  `UpdatedAt time.Time`. — src: internal/model/model.go:160-169 — pinned by:
  TestGobRoundTrip — verify: automated test
- MODEL-041: `TaskStatus` values: `"todo"`, `"doing"`, `"done"`, `"blocked"`.
  Exactly four. — src: internal/model/model.go:41-48 — pinned by:
  model_test.go TestTaskStatusOpen (enumerates all four) — verify: automated
  test
- MODEL-042: `TaskStatus.Open()` returns `true` for every status except
  `TaskDone` (todo/doing/blocked are "open"); used by the restore digest and
  `Counts.TasksOpen`. — src: internal/model/model.go:314-318 — pinned by:
  TestTaskStatusOpen — verify: automated test
- MODEL-043: New tasks are created with status `TaskTodo`; done-task guards
  and blocked/done handling live in the store (e.g. refusing to re-finish a
  done task). — src: internal/store/store.go:294, 435, 634-636 — pinned by:
  store tests — verify: automated test

### Issue

- MODEL-050: `Issue` fields: `ID uint64`, `Title string`, `Body string`,
  `Status IssueStatus`, `Severity Severity`, `TaskID uint64` (0 = not linked
  to a task), `CreatedAt time.Time`, `UpdatedAt time.Time`. — src:
  internal/model/model.go:148-158 — pinned by: none in this package — verify:
  automated test
- MODEL-051: `IssueStatus` values: `"open"`, `"closed"`. Exactly two. — src:
  internal/model/model.go:72-75 — pinned by: internal/store/milestones_test.go
  TestIssueCRUD (:80-87 status mutation) — verify: automated test
- MODEL-052: `Severity` values: `"low"`, `"medium"`, `"high"`, `"critical"`.
  Exactly four. — src: internal/model/model.go:77-81 — pinned by:
  internal/store/milestones_test.go:80-87 (SeverityCritical) — verify:
  automated test

### Note / memory kinds

- MODEL-060: `Note` fields: `ID uint64`, `Target NoteTarget`,
  `TargetID uint64`, `Kind MemoryKind`, `Body string`, `CreatedAt time.Time`.
  No `UpdatedAt`. — src: internal/model/model.go:173-180 — pinned by:
  TestGobRoundTrip — verify: automated test
- MODEL-061: `NoteTarget` values: `"project"`, `"plan"`, `"task"`. Exactly
  three. `TargetID` is 0 for project-targeted notes (by convention; store
  passes 0). — src: internal/model/model.go:50-55;
  internal/store/memory_writeback.go:172 — pinned by: store tests
  (memory_writeback_test.go) — verify: automated test
- MODEL-062: `MemoryKind` values: `"decision"`, `"blocker"`, `"handoff"`,
  `"summary"`. `"summary"` is a write-back command kind and is NEVER stored
  as a `Note.Kind` (it replaces `Meta.Summary` instead). — src:
  internal/model/model.go:57-65; internal/store/memory_writeback.go:84, 102,
  163 — pinned by: internal/store/memory_writeback_test.go — verify:
  automated test
- MODEL-063: `MemoryKind` zero value (`""`) is retained for legacy notes —
  notes written before the Kind field existed decode with empty Kind and must
  remain readable. — src: internal/model/model.go:17-19 — pinned by:
  internal/store/memory_writeback_test.go:175 ("legacy"), version_test.go:86
  ("legacy v4 note") — verify: fixture

### Commit

- MODEL-070: `Commit` fields: `ID uint64`, `SHA string`, `Subject string`,
  `PlanID uint64`, `TaskID uint64` (parsed from a `#<id>` reference in the
  subject; 0 = unlinked), `CreatedAt time.Time`. No `UpdatedAt`. — src:
  internal/model/model.go:139-146 — pinned by: none in this package — verify:
  automated test

### Capability (JSON-tagged contract record)

- MODEL-080: `CapabilityModelVersion` is `uint = 1`; independent of the
  project DB format version so records can be rejected/migrated individually.
  — src: internal/model/model.go:91-94 — pinned by: TestGobRoundTrip (uses
  the constant) — verify: automated test
- MODEL-081: `Capability` JSON field names (these are the IPC/DTO contract):
  `id`, `model_version`, `revision`, `name`, `kind`, `agent_profile`,
  `enabled`, `approval_duration_seconds`, `approved_at`, `expires_at`,
  `scope_digest`, `limits`, `audit`, `http` (omitempty), `git` (omitempty),
  `ssh` (omitempty), `created_at`, `updated_at`. — src:
  internal/model/model.go:250-269 — pinned by: none in this package — verify:
  automated test (JSON shape assertion)
- MODEL-082: `CapabilityKind` values: `"http"`, `"git"`, `"ssh"`. Exactly
  three. — src: internal/model/model.go:83-89 — pinned by: TestGobRoundTrip
  (CapabilityGit) — verify: automated test
- MODEL-083: Usability invariant: a `Capability` is usable only when
  `Enabled == true`, approval not expired, `AgentProfile` exactly matches the
  caller, and the kind-specific scope authorizes the operation. — src:
  internal/model/model.go:246-249 (doc comment stating the contract) —
  pinned by: capability package tests (out of scope here) — verify: automated
  test
- MODEL-084: `CapabilityLimits` JSON names: `timeout_seconds`,
  `max_request_bytes`, `max_response_bytes`, `max_output_bytes`,
  `max_redirects`, `max_concurrent`. Missing byte/time/concurrency values get
  safe defaults during normalization; zero `max_redirects` is an explicit
  deny-all-redirects policy (NOT "use default"). — src:
  internal/model/model.go:182-193 — pinned by: none in this package — verify:
  automated test
- MODEL-085: `CapabilityAuditPolicy` JSON names: `enabled`, `retain_last`.
  Audit records never contain request/response bodies, headers, credentials,
  terminal contents, or raw secret-bearing arguments (security property).
  — src: internal/model/model.go:195-201 — pinned by: none in this package —
  verify: manual / security review
- MODEL-086: `HTTPScope` JSON names: `base_url`, `methods`, `path_prefixes`;
  grants methods and path prefixes beneath one normalized base URL. — src:
  internal/model/model.go:203-208 — pinned by: none in this package — verify:
  automated test
- MODEL-087: `GitScope` JSON names: `remote_name`, `remote_url`,
  `operations`, `branches`, `refspecs`, `allow_tags`, `allow_force_push`,
  `allow_delete_refs`; scoped to one exact normalized remote URL. — src:
  internal/model/model.go:210-220 — pinned by: TestGobRoundTrip (partial) —
  verify: automated test
- MODEL-088: `SSHScope` JSON names: `alias`, `host`, `port` (uint16), `user`,
  `host_key`, `allow_git`, `remote_commands`, `allow_upload`,
  `allow_download`, `upload_roots`, `download_roots`, `upload_remote_roots`,
  `download_remote_roots`, `allow_interactive_shell`,
  `local_forward_targets`, `remote_forward_targets`. `HostKey` is a
  known_hosts-style public key used with strict checking. High-risk
  operations default false. `allow_interactive_shell` is reserved: current
  normalization REJECTS it. — src: internal/model/model.go:222-244 — pinned
  by: none in this package — verify: automated test
- MODEL-089: `CapabilityAudit` is metadata-only; JSON names: `id`,
  `capability_id`, `agent_profile`, `kind`, `operation`, `target`, `success`,
  `error_class`, `duration_millis`, `request_bytes`, `response_bytes`,
  `redirects`, `created_at`. `Target` is the normalized non-secret scope
  target; only an allowlisted error class may be persisted in `error_class`.
  — src: internal/model/model.go:271-288 — pinned by: none in this package —
  verify: automated test / security review

### ProjectRef (global registry)

- MODEL-090: `ProjectRef` fields: `Name string`, `Path string`,
  `LastSeen time.Time`; gob-serialized, no JSON tags. Entry in the global
  project registry pointing at a known project directory. — src:
  internal/model/model.go:290-295 — pinned by: TestGobRoundTrip — verify:
  automated test

### Counts (derived summary DTO)

- MODEL-100: `Counts` fields: `Milestones`, `MilestonesDone`, `Plans`,
  `PlansDone`, `Tasks`, `TasksDone`, `TasksBlocked`, `TasksOpen`,
  `Issues`, `IssuesOpen`, `Commits`, `Notes` (all `int`). Used for the
  bounded context footer. — src: internal/model/model.go:297-312 — pinned by:
  none — verify: automated test
- MODEL-101: `Counts.TasksOpen` is defined as "not done (todo/doing/blocked)"
  — consistent with `TaskStatus.Open()`. — src:
  internal/model/model.go:307, 314-318 — pinned by: TestTaskStatusOpen —
  verify: automated test

### Ordering

- MODEL-110: `Plan`, `Task`, and `Milestone` expose `Ord() int` returning
  their `Order` field for generic sorting; display order is by `Order`
  ascending. — src: internal/model/model.go:320-327 — pinned by: none —
  verify: automated test

### ID allocation (observable shape, enforced in store)

- MODEL-120: All entity IDs are `uint64` allocated via bbolt
  `Bucket.NextSequence()` — monotonically increasing from 1 per bucket,
  never reused within a bucket, gaps possible after failed txns. Buckets with
  independent sequences: capabilities (×2: capabilities + audit),
  memory replays, notes, milestones (×2), commits, plans, tasks, issues,
  meta-adjacent stores. — src: internal/store/capabilities.go:15,127;
  internal/store/memory_writeback.go:100,117; internal/store/milestones.go:17,144;
  internal/store/commits.go:34; internal/store/store.go:207,288,432,539 —
  pinned by: store tests; note body "use NextSequence" in
  model_test.go:42 documents the convention — verify: automated test
- MODEL-121: Foreign-key zero convention: `0` means "none" for
  `Meta.ActivePlan`, `Plan.MilestoneID`, `Issue.TaskID`, `Commit.PlanID`,
  `Commit.TaskID`, `Note.TargetID` (project target). Because NextSequence
  starts at 1, ID 0 is never a real entity. — src:
  internal/model/model.go:125-126, 155, 143-144 — pinned by: none — verify:
  automated test

## Notes / surprises

- The status enums are plain `type X string` with no validation method in
  this package — any string round-trips through gob. All transition/validity
  enforcement lives in `internal/store` and the CLI layer; a Rust rewrite must
  reproduce those rules there, not in the model. The model itself accepts
  unknown enum strings silently (relevant for forward/backward compat).
- `MemorySummary` exists in the same enum as stored note kinds but must never
  appear in a stored `Note` — it's a control-plane value. Rust should model
  this as separate types or enforce the exclusion at the write path
  (enforcement: internal/store/memory_writeback.go:84-117).
- Legacy notes have `Kind == ""` (gob zero value). Any migration or Rust
  decoder must treat empty Kind as valid, not an error.
- `CapabilityLimits` zero-value asymmetry: zero `max_redirects` means
  "deny all redirects" while other zero limits mean "apply safe default".
  Easy to get wrong in a rewrite.
- `Counts` is computed, not stored — no persistence contract, but its exact
  field set and `TasksOpen` semantics feed the CLI/TUI context footer output,
  so the derived values are observable.
- `SSHScope.AllowInteractiveShell` is part of the serialized shape but is
  rejected by normalization today — the Rust side must keep rejecting it
  (reserved field) while still round-tripping the JSON key.
- `Capability` has both gob persistence (store) and JSON tags (DTO/IPC);
  both shapes must be preserved. The JSON `omitempty` on the three scope
  pointers means absent scope keys, not `null`, in serialized DTOs.
- `Note` and `Commit` have no `UpdatedAt`; `Meta`/`Milestone`/`Plan`/`Task`/
  `Issue`/`Capability` do. `CapabilityAudit` has only `CreatedAt`.
